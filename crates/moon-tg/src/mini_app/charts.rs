//! Bounded chart reads; blocking tape and report work stays outside the host owner loop.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    Arc, Mutex, PoisonError,
    mpsc::{self, Receiver, SyncSender},
};
use std::time::{Duration, Instant};

use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::db::CoreNames;
use moon_core::feed::{OrderRow, Side, Tick};
use moon_core::market::MarketDataSource;
use moon_core::market::trade_replay::venue_caps::TickValue;
use moon_core::market::trade_replay::worker::TradeReplayRequest;
use moon_core::market::trade_replay::{
    Coverage, ReplayIntent, ReplayWindow, TickStatus, TickThinning, TradeReplayEmpty,
    TradeReplayOutcome, TradeReplaySource,
};
use moon_core::market::{SIDE_BUCKET_MS, side_slots_of_ticks};
use moon_core::market::{tape_recorder, trade_replay};
use moon_core::telegram::web::MiniAppApiError;
use moon_core::telegram::web::dto::{
    ChartRangeDto, ChartStateDto, LevelDto, LevelKindDto, LiveTicksDto, MAX_CHART_TICKS,
    MAX_LIVE_TICKS, MAX_VOLUME_BUCKETS, TradeTicksDto,
};
use moon_core::util::fmt;

use super::dto::{order_at_mark, order_quote, pnl_sign, pnl_text, trade_dto};
use super::{mini_access, visible_cores};
use crate::report::MiniTrade;
use crate::{TelegramState, TgHost};

/// A replay remains owned until its receiver is evicted or the service restarts.
pub(crate) struct ChartJob {
    pub(super) receiver: Receiver<TradeReplayOutcome>,
    pub(super) at: Instant,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) latest: Option<TradeReplayOutcome>,
}

impl Drop for ChartJob {
    /// Tell the replay worker to abandon pages when this consumer forgets its receiver.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// At most four replay channels, with a five-minute lifetime independent of polling.
#[derive(Default)]
pub(crate) struct ChartJobs(pub(super) BTreeMap<(u64, i64), ChartJob>);

impl ChartJobs {
    /// Drop expired receivers before inserting; cancellation also covers oldest-entry eviction.
    pub(super) fn insert(&mut self, key: (u64, i64), job: ChartJob) {
        self.prune();
        if !self.0.contains_key(&key)
            && self.0.len() >= 4
            && let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, job)| job.at)
                .map(|(key, _)| *key)
        {
            self.0.remove(&oldest);
        }
        self.0.insert(key, job);
    }

    /// Expire jobs even when a poll hits another chart's fast path.
    fn prune(&mut self) {
        self.0
            .retain(|_, job| job.at.elapsed() < Duration::from_secs(300));
    }
}

/// A previously listed order supplies identity after the session removes the row.
#[derive(Clone)]
pub(crate) struct OrderSnapshot {
    pub(super) coin: String,
    market: String,
    entry_ms: i64,
    buy_price: f64,
    levels: Vec<LevelDto>,
    pub(super) at: Instant,
}

/// Preserve at most 64 recently observed orders, expiring identity after one day.
pub(super) fn remember_order(state: &mut TelegramState, core: u64, order: &OrderRow) {
    state
        .mini_order_snapshots
        .retain(|_, row| row.at.elapsed() < Duration::from_secs(86400));
    let key = (core, order.uid);
    if !state.mini_order_snapshots.contains_key(&key)
        && state.mini_order_snapshots.len() >= 64
        && let Some(oldest) = state
            .mini_order_snapshots
            .iter()
            .min_by_key(|(_, row)| row.at)
            .map(|(key, _)| *key)
    {
        state.mini_order_snapshots.remove(&oldest);
    }
    state.mini_order_snapshots.insert(
        key,
        OrderSnapshot {
            coin: order.coin.clone(),
            market: order.market.clone(),
            entry_ms: order.entry_fill_time_ms as i64,
            buy_price: order.buy_price,
            levels: live_levels(order),
            at: Instant::now(),
        },
    );
}

/// Admission and Finish both require the current grant and visible core.
fn authorize(
    host: &dyn TgHost,
    chat: i64,
    core: u64,
) -> Result<TelegramReportAccess, MiniAppApiError> {
    let access = mini_access(host, chat).ok_or(MiniAppApiError::Rejected)?;
    if !visible_cores(host, &access)
        .iter()
        .any(|(id, _, _)| *id == core)
    {
        return Err(MiniAppApiError::NotFound);
    }
    Ok(access)
}

/// Revoke an answer whenever its captured grant changed, including a narrowed viewer grant.
pub(super) fn finish_grant(
    captured: &TelegramReportAccess,
    current: Option<TelegramReportAccess>,
    visible: bool,
) -> Result<(), MiniAppApiError> {
    if current.as_ref() != Some(captured) {
        Err(MiniAppApiError::Rejected)
    } else if !visible {
        Err(MiniAppApiError::NotFound)
    } else {
        Ok(())
    }
}

/// Claim a bounded reader slot before enqueuing a background job.
pub(super) fn claim_pending(count: &mut usize, limit: usize) -> bool {
    if *count >= limit {
        false
    } else {
        *count += 1;
        true
    }
}

/// Keep Finish reachable on unwind so it can release admission and recheck the grant.
pub(super) fn chart_read<T>(
    read: impl FnOnce() -> Result<T, MiniAppApiError>,
) -> Result<T, MiniAppApiError> {
    catch_unwind(AssertUnwindSafe(read)).unwrap_or(Err(MiniAppApiError::ReadFailed))
}

/// Preserve the recorder's lifecycle identity rather than the displayed report id.
pub(super) fn recorder_id(trade: &MiniTrade) -> tape_recorder::TradeId {
    tape_recorder::TradeId {
        core: trade.core_uid,
        rec_id: trade.new_rec_id,
    }
}

/// Admit a closed chart without blocking the host; range changes only the page viewport.
pub(super) fn mini_trade_ticks(
    host: &mut dyn TgHost,
    chat: i64,
    core: u64,
    rec_id: i64,
    _range: ChartRangeDto,
    reply: SyncSender<Result<TradeTicksDto, MiniAppApiError>>,
) {
    let access = match authorize(host, chat, core) {
        Ok(access) => access,
        Err(error) => {
            let _ = reply.try_send(Err(error));
            return;
        }
    };
    if !claim_pending(&mut host.state_mut().mini_chart_pending, 2) {
        let _ = reply.try_send(Err(MiniAppApiError::Busy));
        return;
    }
    let cached = host
        .state()
        .mini_trades_last
        .as_ref()
        .and_then(|cache| {
            cache
                .trades
                .iter()
                .find(|trade| trade.core_uid == core && trade.rec_id == rec_id)
        })
        .cloned();
    let names = CoreNames::from_servers(&host.config().servers);
    let zone = host.report_zone();
    let source = host.session().market_source();
    let quote = host
        .config()
        .servers
        .iter()
        .find(|server| server.uid == core)
        .map(|server| server.market.clone())
        .unwrap_or_default();
    let exchange = visible_cores(host, &access)
        .into_iter()
        .find(|(id, _, _)| *id == core)
        .map(|(_, _, exchange)| exchange)
        .unwrap_or_default();
    let jobs = host.state().mini_chart_jobs.clone();
    host.spawn(Box::new(move || {
        let deadline = Instant::now() + Duration::from_secs(4);
        let result = chart_read(|| {
            cached
                .map(Ok)
                .unwrap_or_else(|| {
                    crate::report::read_mini_chart_trade(zone, names, core, Ok(rec_id))
                        .map_err(|_| MiniAppApiError::ReadFailed)
                        .and_then(|trade| trade.ok_or(MiniAppApiError::NotFound))
                })
                .map(|trade| read_trade(&source, &jobs, &quote, exchange, trade, deadline))
        });
        Box::new(move |host: &mut dyn TgHost| {
            host.state_mut().mini_chart_pending = host.state().mini_chart_pending.saturating_sub(1);
            let current = mini_access(host, chat);
            let visible = current.as_ref().is_some_and(|grant| {
                visible_cores(host, grant)
                    .iter()
                    .any(|(id, _, _)| *id == core)
            });
            let permitted = finish_grant(&access, current, visible);
            let _ = reply.try_send(permitted.and(result));
        })
    }));
}

/// Remaining wait shared by recorder and tile reads rather than restarted per stage.
fn remaining(deadline: Instant, cap: Duration) -> Duration {
    deadline.saturating_duration_since(Instant::now()).min(cap)
}

/// Keep row stamps on unavailable responses while omitting data whose provenance is unknown.
fn trade_base(trade: &MiniTrade) -> TradeTicksDto {
    let close_ms = trade.close_utc.saturating_mul(1000);
    TradeTicksDto {
        open_ms: trade
            .buy_utc
            .map(|time| time.saturating_mul(1000))
            .unwrap_or_else(|| close_ms.saturating_sub(1000)),
        close_ms,
        ..Default::default()
    }
}

/// Recorder, held terminal tiles, then an existing bounded replay job, in that order.
fn read_trade(
    source: &MarketDataSource,
    jobs: &Mutex<ChartJobs>,
    quote: &str,
    exchange: String,
    trade: MiniTrade,
    deadline: Instant,
) -> TradeTicksDto {
    let mut dto = trade_base(&trade);
    let open = dto.open_ms;
    let close = dto.close_ms;
    let from = open.saturating_sub(300_000);
    let to = close.saturating_add(300_000);
    if close <= open || open <= 0 {
        dto.state = ChartStateDto::NoTape;
        return dto;
    }
    let recorder = tape_recorder::trade_tape(
        recorder_id(&trade),
        300_000,
        300_000,
        remaining(deadline, Duration::from_millis(1500)),
    );
    if let Ok(tape) = recorder
        && tape.settled
        && !tape.ticks.is_empty()
        && covers_trade(&tape.covered, open, close)
    {
        let value = tick_value(source, trade.core_uid, &tape.market);
        let slots = quote_slots(&tape.ticks, value);
        return trade_ok(
            dto,
            &trade,
            tape.ticks,
            Some(slots),
            "recorder",
            exchange,
            TickThinning::Raw,
        );
    }
    let Ok(address) = source.replay_address(trade.core_uid) else {
        return dto;
    };
    let Some(market) = source.resolve_market(trade.core_uid, quote, &trade.coin) else {
        return dto;
    };
    let tick_value = tick_value(source, trade.core_uid, &market);
    if let Ok(span) = tape_recorder::market_tape(
        &address.exchange_key,
        &market,
        from,
        to,
        remaining(deadline, Duration::from_millis(1000)),
    ) && span.settled
        && !span.ticks.is_empty()
        && covers_trade(&span.covered, open, close)
    {
        let slots = quote_slots(&span.ticks, tick_value);
        return trade_ok(
            dto,
            &trade,
            span.ticks,
            Some(slots),
            "recorder",
            exchange,
            TickThinning::Raw,
        );
    }
    let (reply, answer) = mpsc::channel();
    trade_replay::query_held(trade_replay::TickQuery {
        exchange_key: address.exchange_key.clone(),
        market: market.clone(),
        spans: Coverage::one((from, to)),
        reply,
    });
    if let Ok(held) = answer.recv_timeout(remaining(deadline, Duration::from_millis(500)))
        && !held.ticks.is_empty()
        && held.covered.contains((open, close))
    {
        let slots = quote_slots(&held.ticks, tick_value);
        return trade_ok(
            dto,
            &trade,
            held.ticks,
            Some(slots),
            "terminal",
            exchange,
            TickThinning::Raw,
        );
    }
    let key = (trade.core_uid, trade.rec_id);
    let mut jobs = jobs.lock().unwrap_or_else(PoisonError::into_inner);
    jobs.prune();
    let retry = jobs
        .0
        .get(&key)
        .and_then(|job| job.latest.as_ref())
        .is_some_and(|outcome| {
            let (state, retry_at) = replay_state(Some(outcome));
            state == ChartStateDto::Error
                || retry_at.is_some_and(|at| at <= moon_core::util::now_unix_ms_i64())
        });
    if retry {
        jobs.0.remove(&key);
    }
    if !jobs.0.contains_key(&key) {
        let (reply, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        trade_replay::worker::request(TradeReplayRequest {
            address,
            market,
            window: ReplayWindow {
                from_ms: from,
                to_ms: to,
                open_ms: open,
                close_ms: close,
                margin_ms: 300_000,
                long_position_ms: trade_replay::long_position_ms(),
                over_budget: false,
            },
            identity: trade.rec_id as u64 ^ trade.core_uid.rotate_left(32),
            tick_value,
            ticks: true,
            intent: ReplayIntent::Chart,
            cancel: cancel.clone(),
            reply,
        });
        jobs.insert(
            key,
            ChartJob {
                receiver,
                at: Instant::now(),
                cancel,
                latest: None,
            },
        );
    }
    let job = jobs.0.get_mut(&key).expect("inserted chart job");
    while let Ok(outcome) = job.receiver.try_recv() {
        job.latest = Some(outcome);
    }
    let outcome = job.latest.clone();
    drop(jobs);
    let (state, retry) = replay_state(outcome.as_ref());
    dto.state = state;
    if state == ChartStateDto::Loading {
        dto.retry_in_ms = Some(1500);
    }
    dto.retry_at_ms = retry;
    if let Some(TradeReplayOutcome::Ready(series)) = outcome
        && state == ChartStateDto::Ok
    {
        let origin = if series.source == TradeReplaySource::CoreTicks {
            "archive"
        } else {
            "exchange"
        };
        return trade_ok(
            dto,
            &trade,
            series.ticks,
            Some(
                series
                    .side_slots
                    .into_iter()
                    .map(|slot| (slot.id.saturating_mul(SIDE_BUCKET_MS), slot.buy, slot.sell))
                    .collect(),
            ),
            origin,
            exchange,
            series.thinning,
        );
    }
    if state == ChartStateDto::NoTape {
        dto.levels = Some(trade_levels(&trade));
    }
    dto
}

/// Interpret tick availability explicitly; candle-only success is never a tick chart.
pub(super) fn replay_state(outcome: Option<&TradeReplayOutcome>) -> (ChartStateDto, Option<i64>) {
    use ChartStateDto as State;
    match outcome {
        None => (State::Loading, None),
        Some(TradeReplayOutcome::Failed(_))
        | Some(TradeReplayOutcome::Empty(TradeReplayEmpty::CoreNotConnected)) => {
            (State::Error, None)
        }
        Some(TradeReplayOutcome::Empty(_)) => (State::NoTape, None),
        Some(TradeReplayOutcome::Ready(series)) => match series.tick_status {
            TickStatus::Pending | TickStatus::Streaming | TickStatus::AwaitingCore => {
                (State::Loading, None)
            }
            TickStatus::ArchivePending { retry_at_ms } => {
                (State::ArchivePending, Some(retry_at_ms))
            }
            TickStatus::Failed | TickStatus::RateLimited { .. } => (State::Error, None),
            TickStatus::Served | TickStatus::ContextUnavailable
                if series.source.is_ticks() && !series.ticks.is_empty() =>
            {
                (State::Ok, None)
            }
            _ => (State::NoTape, None),
        },
    }
}

/// Bound archived levels and retain the recorded entry when traces have not arrived.
fn trade_levels(trade: &MiniTrade) -> Vec<LevelDto> {
    let mut levels = crate::notify::charts::mini_trade_levels(trade.core_uid, trade.report_uid);
    if !levels.iter().any(|(kind, _)| *kind == LevelKindDto::Buy) {
        levels.push((LevelKindDto::Buy, trade.buy_price));
    }
    levels
        .into_iter()
        .filter(|(_, price)| price.is_finite() && *price > 0.0)
        .take(3)
        .map(|(kind, price)| LevelDto { kind, price })
        .collect()
}

/// Accept a fully recorded position even when a later tail is separated by a gap.
pub(super) fn covers_trade(covered: &[(i64, i64)], open: i64, close: i64) -> bool {
    covered
        .iter()
        .any(|&(from, to)| from <= open && to >= close)
}

/// Resolve the same quantity interpretation used by the replay worker.
fn tick_value(source: &MarketDataSource, core: u64, market: &str) -> TickValue {
    let terms = source.market_contract_terms(core, market);
    source
        .replay_address(core)
        .ok()
        .and_then(|address| address.venue)
        .map(|venue| {
            trade_replay::venue_caps::tick_value(
                venue,
                terms.as_ref().map(|(quote, size)| (quote.as_str(), *size)),
            )
        })
        .unwrap_or(TickValue::Unknown)
}

/// Sum raw quote turnover before thinning, using replay's contract terms and bucket clock.
pub(super) fn quote_slots(ticks: &[Tick], value: TickValue) -> Vec<(i64, f64, f64)> {
    side_slots_of_ticks(ticks, value)
        .into_iter()
        .map(|slot| (slot.id.saturating_mul(SIDE_BUCKET_MS), slot.buy, slot.sell))
        .collect()
}

/// Aggregate pre-thinning quote turnover; absent slots mean volume is unavailable.
pub(super) fn volume(slots: Option<&[(i64, f64, f64)]>) -> (i64, Vec<(i64, f64, f64)>) {
    let values = slots.unwrap_or_default();
    let first = values.iter().map(|(time, _, _)| *time).min().unwrap_or(0);
    let last = values
        .iter()
        .map(|(time, _, _)| *time)
        .max()
        .unwrap_or(first);
    let width = ((last.saturating_sub(first) / MAX_VOLUME_BUCKETS as i64) + 1).max(1000);
    let mut buckets = BTreeMap::<i64, (f64, f64)>::new();
    for &(time, buy, sell) in values {
        let start = first + (time.saturating_sub(first) / width) * width;
        let row = buckets.entry(start).or_default();
        row.0 += buy;
        row.1 += sell;
    }
    (
        width,
        buckets
            .into_iter()
            .map(|(time, (buy, sell))| (time, buy, sell))
            .collect(),
    )
}

/// Convert real prints to the frozen array wire format.
fn wire_ticks(ticks: Vec<Tick>) -> Vec<(i64, f64, f64, u8)> {
    ticks
        .into_iter()
        .map(|tick| {
            (
                tick.time_ms as i64,
                f64::from(tick.price),
                f64::from(tick.qty),
                u8::from(tick.side == Side::Buy),
            )
        })
        .collect()
}

/// Keep the same magnitude precision as the Mini App's adaptive price text.
pub(super) fn price_decimals(price: f64) -> u32 {
    if !price.is_finite() || price <= 0.0 || price >= 1000.0 {
        0
    } else {
        (4 - price.log10().floor() as i32).clamp(0, 18) as u32
    }
}

/// Populate successful data together so no failed response leaks a partial ok payload.
pub(super) fn trade_ok(
    mut dto: TradeTicksDto,
    trade: &MiniTrade,
    ticks: Vec<Tick>,
    slots: Option<Vec<(i64, f64, f64)>>,
    source: &str,
    exchange: String,
    prior: TickThinning,
) -> TradeTicksDto {
    // Unknown quantity terms cannot truthfully reconstruct quote turnover.
    let raw_volume = slots
        .as_ref()
        .filter(|slots| !slots.is_empty())
        .map(|slots| volume(Some(slots)));
    let (ticks, thinning) =
        trade_replay::fit_for_chart(ticks, dto.open_ms, dto.close_ms, MAX_CHART_TICKS);
    let bucket = |mode| match mode {
        TickThinning::Raw => 0,
        TickThinning::Edges { bucket_ms } | TickThinning::Whole { bucket_ms } => bucket_ms,
    };
    dto.state = ChartStateDto::Ok;
    dto.retry_in_ms = None;
    dto.retry_at_ms = None;
    dto.from_ms = Some(dto.open_ms.saturating_sub(300_000));
    dto.to_ms = Some(dto.close_ms.saturating_add(300_000));
    dto.entry_price = Some(trade.buy_price);
    dto.exit_price = Some(trade.sell_price);
    dto.levels = Some(trade_levels(trade));
    dto.ticks = Some(wire_ticks(ticks));
    dto.thinned_ms = Some(bucket(prior).max(bucket(thinning)));
    if let Some((width, buckets)) = raw_volume {
        dto.volume_bucket_ms = Some(width);
        dto.volume = Some(buckets);
    }
    dto.exit_reason = trade.sell_reason.clone();
    dto.source = Some(source.to_string());
    dto.exchange = Some(exchange);
    dto.price_decimals = Some(price_decimals(trade.buy_price));
    dto
}

/// Admit one live delta, snapshotting only the row and grant on the owner thread.
pub(super) fn mini_order_ticks(
    host: &mut dyn TgHost,
    chat: i64,
    core: u64,
    uid: u64,
    since_ms: Option<i64>,
    reply: SyncSender<Result<LiveTicksDto, MiniAppApiError>>,
) {
    let access = match authorize(host, chat, core) {
        Ok(access) => access,
        Err(error) => {
            let _ = reply.try_send(Err(error));
            return;
        }
    };
    if !claim_pending(&mut host.state_mut().mini_live_pending, 4) {
        let _ = reply.try_send(Err(MiniAppApiError::Busy));
        return;
    }
    let live = host
        .session()
        .store()
        .core(core)
        .and_then(|data| data.orders.iter().find(|row| row.uid == uid))
        .cloned();
    if let Some(row) = &live {
        remember_order(host.state_mut(), core, row);
    }
    host.state_mut()
        .mini_order_snapshots
        .retain(|_, row| row.at.elapsed() < Duration::from_secs(86400));
    let snapshot = host.state().mini_order_snapshots.get(&(core, uid)).cloned();
    let closing = order_closing(live.as_ref());
    let source = host.session().market_source();
    let zone = host.report_zone();
    let names = CoreNames::from_servers(&host.config().servers);
    host.spawn(Box::new(move || {
        let now = moon_core::util::now_unix_ms_i64();
        let result = chart_read(|| {
            let mut dto = live_base(live.as_ref(), now);
            if live.is_none()
                && let Some(snapshot) = &snapshot
            {
                dto.entry_ms = snapshot.entry_ms;
                dto.entry_price = snapshot.buy_price;
                dto.mark_price = source
                    .latest_price(core, &snapshot.market)
                    .ok()
                    .map(f64::from)
                    .unwrap_or(snapshot.buy_price);
                dto.price_decimals = price_decimals(snapshot.buy_price);
                dto.levels = snapshot.levels.clone();
            }
            let trade = if closing {
                if let Some(snapshot) = &snapshot {
                    crate::report::read_mini_chart_trade(
                        zone,
                        names,
                        core,
                        Err((snapshot.coin.clone(), snapshot.entry_ms)),
                    )
                    .or_else(|error| match error {
                        moon_core::db::ReadFail::NotReady => Ok(None),
                        _ => Err(MiniAppApiError::ReadFailed),
                    })
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            };
            if closing {
                dto.state = handoff_state(
                    snapshot.is_some(),
                    trade.as_ref().is_ok_and(Option::is_some),
                );
                if let Some(row) = &live {
                    fill_mark(&source, core, row, &mut dto);
                }
            } else if let Some(row) = &live {
                read_live(&source, core, row, since_ms, &mut dto);
            }
            trade.map(|trade| (dto, trade))
        });
        Box::new(move |host: &mut dyn TgHost| {
            host.state_mut().mini_live_pending = host.state().mini_live_pending.saturating_sub(1);
            let current = mini_access(host, chat);
            let visible = current.as_ref().is_some_and(|grant| {
                visible_cores(host, grant)
                    .iter()
                    .any(|(id, _, _)| *id == core)
            });
            let permitted = finish_grant(&access, current, visible);
            let result = permitted.and_then(|()| {
                result.map(|(mut dto, trade)| {
                    dto.trade = trade
                        .as_ref()
                        .map(|trade| trade_dto(host, zone, now / 1000, trade));
                    dto
                })
            });
            let _ = reply.try_send(result);
        })
    }));
}

/// Base fields survive a closing handoff without inventing an exit execution.
pub(super) fn live_base(row: Option<&OrderRow>, now: i64) -> LiveTicksDto {
    LiveTicksDto {
        state: ChartStateDto::NoTape,
        now_ms: now,
        last_tick_ms: None,
        entry_ms: row.map_or(0, |row| row.entry_fill_time_ms as i64),
        entry_price: row.map_or(0.0, |row| row.buy_price),
        mark_price: row.map_or(0.0, |row| f64::from(row.price)),
        levels: row.map(live_levels).unwrap_or_default(),
        ticks: Vec::new(),
        truncated: false,
        pnl_text: String::new(),
        pnl_sign: 0,
        change_text: String::new(),
        take_dist_text: String::new(),
        stop_dist_text: String::new(),
        price_decimals: row.map_or(0, |row| price_decimals(row.buy_price)),
        trade: None,
    }
}

/// A completed row starts handoff before the session removes it.
pub(super) fn order_closing(row: Option<&OrderRow>) -> bool {
    row.is_none_or(|row| row.job_is_done)
}

/// Only a matched report row permits a terminal closed handoff.
pub(super) fn handoff_state(known: bool, matched: bool) -> ChartStateDto {
    if known && matched {
        ChartStateDto::Closed
    } else {
        ChartStateDto::Closing
    }
}

/// Return current entry, take and stop only when their source prices are usable.
pub(super) fn live_levels(row: &OrderRow) -> Vec<LevelDto> {
    [
        (LevelKindDto::Take, row.take_profit),
        (LevelKindDto::Entry, Some(row.buy_price)),
        (LevelKindDto::Stop, row.stop_loss),
    ]
    .into_iter()
    .filter_map(|(kind, price)| {
        price
            .filter(|price| price.is_finite() && *price > 0.0)
            .map(|price| LevelDto { kind, price })
    })
    .collect()
}

/// Clamp the inclusive cursor so late prints in the last millisecond remain visible.
pub(super) fn live_from(now: i64, since: Option<i64>) -> i64 {
    let oldest = now.saturating_sub(900_000);
    since.map_or(oldest, |since| since.clamp(oldest, now))
}

/// Freshness follows the source's last print independently of an empty delta.
pub(super) fn live_state(now: i64, last: Option<i64>) -> ChartStateDto {
    match last {
        None => ChartStateDto::NoTape,
        Some(last) if now.saturating_sub(last) > 5000 => ChartStateDto::Stale,
        Some(_) => ChartStateDto::Live,
    }
}

/// Read terminal capability first; stations renew the recorder's expiring pair demand.
fn read_live(
    source: &MarketDataSource,
    core: u64,
    row: &OrderRow,
    since: Option<i64>,
    dto: &mut LiveTicksDto,
) {
    let from = live_from(dto.now_ms, since);
    let data = source
        .live_core_ticks(core, &row.market, from, dto.now_ms, MAX_LIVE_TICKS)
        .or_else(|| {
            let address = source.replay_address(core).ok()?;
            tape_recorder::watch_market(
                &address.exchange_key,
                &row.market,
                dto.now_ms.saturating_add(30_000),
            );
            let span = tape_recorder::market_tape(
                &address.exchange_key,
                &row.market,
                from,
                dto.now_ms,
                Duration::from_millis(1500),
            )
            .ok()?;
            let start = span
                .covered
                .last()
                .map_or(from, |&(start, _)| start.max(from));
            let mut delta = recorder_delta(span.ticks, start, dto.now_ms);
            delta.last_tick_ms = delta.last_tick_ms.or(since);
            Some(delta)
        });
    if let Some(data) = data {
        dto.last_tick_ms = data.last_tick_ms;
        dto.truncated = data.truncated;
        dto.ticks = wire_ticks(data.ticks);
    }
    dto.state = live_state(dto.now_ms, dto.last_tick_ms);
    fill_mark(source, core, row, dto);
}

/// Format live and closing rows from one fresh mark using the order table's arithmetic.
fn fill_mark(source: &MarketDataSource, core: u64, row: &OrderRow, dto: &mut LiveTicksDto) {
    let mark = source
        .latest_price(core, &row.market)
        .ok()
        .filter(|mark| mark.is_finite() && *mark > 0.0)
        .unwrap_or(row.price);
    dto.mark_price = f64::from(mark);
    let (pnl, change) = order_at_mark(row, mark);
    let quote = order_quote(&row.quote, &row.market);
    dto.pnl_text = pnl
        .and_then(|pnl| pnl_text(pnl, &quote))
        .unwrap_or_default();
    dto.pnl_sign = pnl.map_or(0, |pnl| pnl_sign(pnl, &quote));
    dto.change_text = change.and_then(signed_pct).unwrap_or_default();
    let distance = |target: Option<f64>| {
        target
            .and_then(|target| {
                let mut level = row.clone();
                level.sell_price = target;
                moon_core::feed::order_math::pct_to_exit(&level, dto.mark_price)
                    .and_then(signed_pct)
            })
            .unwrap_or_default()
    };
    dto.take_dist_text = distance(row.take_profit);
    dto.stop_dist_text = distance(row.stop_loss);
}

/// Use the order table's signed percent precision for live figures.
fn signed_pct(value: f64) -> Option<String> {
    fmt::signed_pct(value, moon_core::feed::order_math::MONEY_DECIMALS).map(|(text, _)| text)
}

/// Retain freshness before excluding an acknowledged delta and dropping its older overflow.
pub(super) fn recorder_delta(
    mut ticks: Vec<Tick>,
    from: i64,
    now: i64,
) -> moon_core::market::LiveTicks {
    debug_assert!(
        ticks
            .windows(2)
            .all(|pair| pair[0].time_ms <= pair[1].time_ms)
    );
    let last_tick_ms = ticks.last().map(|tick| tick.time_ms as i64);
    ticks.retain(|tick| tick.time_ms >= from as f64 && tick.time_ms <= now as f64);
    let truncated = ticks.len() > MAX_LIVE_TICKS;
    let older = ticks.len().saturating_sub(MAX_LIVE_TICKS);
    ticks.drain(..older);
    moon_core::market::LiveTicks {
        ticks,
        truncated,
        last_tick_ms,
    }
}
