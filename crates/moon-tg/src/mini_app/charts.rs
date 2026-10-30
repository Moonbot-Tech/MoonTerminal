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
    TradeReplayFailure, TradeReplayOutcome, TradeReplaySource,
};
use moon_core::market::{SIDE_BUCKET_MS, side_slots_of_ticks};
use moon_core::market::{tape_recorder, trade_replay};
use moon_core::telegram::web::MiniAppApiError;
use moon_core::telegram::web::dto::{
    ChartRangeDto, ChartStateDto, CoinDealDto, CoinLevelDto, CoinPositionDto, CoinTfDto,
    CoinTicksDto, LevelDto, LevelKindDto, LiveTicksDto, MAX_CHART_TICKS, MAX_LIVE_TICKS,
    MAX_VOLUME_BUCKETS, TradeTicksDto,
};
use moon_core::util::fmt;

use super::dto::{core_quotes, order_at_mark, order_quote, pnl_sign, pnl_text, trade_dto};
use super::order_handoff::{
    begin_order_handoff, finish_order_handoff, order_snapshot, remember_order,
};
use super::{mini_access, visible_cores};
use crate::TgHost;
use crate::report::MiniTrade;

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

/// A replay job's identity: core, displayed id and recorder id. Station rows can all carry
/// displayed id 0, so the recorder id keeps two of their trades apart.
pub(super) type JobKey = (u64, i64, i64);

pub(super) fn job_key(trade: &MiniTrade) -> JobKey {
    (trade.core_uid, trade.rec_id, trade.new_rec_id)
}

/// At most four replay channels, with a five-minute lifetime independent of polling.
#[derive(Default)]
pub(crate) struct ChartJobs(pub(super) BTreeMap<JobKey, ChartJob>);

impl ChartJobs {
    /// Drop expired receivers before inserting; cancellation also covers oldest-entry eviction.
    pub(super) fn insert(&mut self, key: JobKey, job: ChartJob) {
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

/// Finish one live reader whose admission count is preserved across service replacement.
pub(super) fn finish_live_read(
    state: &mut crate::TelegramState,
    service: &Arc<Mutex<ChartJobs>>,
) -> Result<(), MiniAppApiError> {
    // start_saved transfers in-flight counts, so a retired reader still owns one slot.
    state.mini_live_pending = state.mini_live_pending.saturating_sub(1);
    if !Arc::ptr_eq(service, &state.mini_chart_jobs) {
        return Err(MiniAppApiError::Rejected);
    }
    Ok(())
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
    key: crate::report::TradeKey,
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
        log::info!("[mini-chart] trade ticks state=Busy source=admission elapsed_ms=0");
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
                .find(|trade| trade.core_uid == core && key.matches(trade))
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
        let started = Instant::now();
        let deadline = started + Duration::from_secs(4);
        let mut coin = String::new();
        let mut stage = "report";
        let result = chart_read(|| {
            cached
                .map(Ok)
                .unwrap_or_else(|| {
                    crate::report::read_mini_chart_trade(zone, names, core, Ok(key))
                        .map_err(|_| MiniAppApiError::ReadFailed)
                        .and_then(|trade| trade.ok_or(MiniAppApiError::NotFound))
                })
                .map(|trade| {
                    coin = trade.coin.clone();
                    let (dto, used) =
                        read_trade(&source, &jobs, &quote, exchange.clone(), trade, deadline);
                    stage = used;
                    dto
                })
        });
        log::info!(
            "[mini-chart] trade ticks exchange={exchange} coin={coin} state={} source={stage} elapsed_ms={}",
            result
                .as_ref()
                .map_or_else(|error| format!("{error:?}"), |dto| format!("{:?}", dto.state)),
            started.elapsed().as_millis()
        );
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

/// How long a replay may stay queued before the sheet is told the source is catching up.
pub(super) const LOADING_PATIENCE: Duration = Duration::from_secs(20);

/// Fast poll while a fresh replay job is still within [`LOADING_PATIENCE`].
pub(super) const LOADING_RETRY_MS: u64 = 1500;

/// Slow background poll while the source is catching up.
pub(super) const CATCHUP_RETRY_MS: u64 = 10_000;

/// One UTC day in milliseconds.
const DAY_MS: i64 = 86_400_000;

/// Recorder, held terminal tiles, then an existing bounded replay job, in that order.
///
/// Returns:
///     The answer and the name of the stage that decided it, for the request's log line.
fn read_trade(
    source: &MarketDataSource,
    jobs: &Mutex<ChartJobs>,
    quote: &str,
    exchange: String,
    trade: MiniTrade,
    deadline: Instant,
) -> (TradeTicksDto, &'static str) {
    let mut dto = trade_base(&trade);
    let open = dto.open_ms;
    let close = dto.close_ms;
    let from = open.saturating_sub(300_000);
    let to = close.saturating_add(300_000);
    if close <= open || open <= 0 {
        dto.state = ChartStateDto::NoTape;
        return (dto, "window");
    }
    let recorder = tape_recorder::trade_tape(
        recorder_id(&trade),
        300_000,
        300_000,
        remaining(deadline, Duration::from_millis(1500)),
    );
    // A busy recorder answers neither question in time; asking twice only burns the budget.
    let recorder_busy = matches!(recorder, Err(tape_recorder::TapeMiss::Busy));
    // The hold itself recorded is enough: a fresh trade's tail margin is still being written,
    // and waiting for it to settle sent every fresh trade to the slower venue walk.
    if let Ok(tape) = recorder
        && !tape.ticks.is_empty()
        && covers_trade(&tape.covered, open, close)
    {
        let value = source.replay_tick_value(trade.core_uid, &tape.market);
        if let Some(answer) = stored_trade(
            &trade,
            &exchange,
            value,
            Ok(tape_recorder::RecorderSpan {
                ticks: tape.ticks,
                settled: tape.settled,
                covered: tape.covered,
            }),
            || None,
        ) {
            return answer;
        }
    }
    let now = moon_core::util::now_unix_ms_i64();
    // The core is not connected yet (a restart) or its catalog is not loaded: both recover on
    // their own, so the sheet keeps asking slowly instead of showing a hard error.
    let Ok(address) = source.replay_address(trade.core_uid) else {
        settle_trade(&mut dto, ChartStateDto::CatchingUp, None, None, None, now);
        return (dto, "no_address");
    };
    let Some(market) = source.resolve_market(trade.core_uid, quote, &trade.coin) else {
        settle_trade(&mut dto, ChartStateDto::CatchingUp, None, None, None, now);
        return (dto, "no_market");
    };
    let tick_value = source.replay_tick_value(trade.core_uid, &market);
    let archive_lag = address
        .venue
        .and_then(|venue| trade_replay::venue_caps::archive_route(venue, &market))
        .map(|route| route.publish_lag_ms());
    let recorder = if !recorder_busy {
        tape_recorder::market_tape(
            &address.exchange_key,
            &market,
            from,
            to,
            remaining(deadline, Duration::from_millis(1000)),
        )
    } else {
        Err(tape_recorder::TapeMiss::Busy)
    };
    if let Some(answer) = stored_trade(&trade, &exchange, tick_value, recorder, || {
        let (reply, answer) = mpsc::channel();
        trade_replay::query_held(trade_replay::TickQuery {
            exchange_key: address.exchange_key.clone(),
            market: market.clone(),
            spans: Coverage::one((from, to)),
            reply,
        });
        answer
            .recv_timeout(remaining(deadline, Duration::from_millis(500)))
            .ok()
    }) {
        return answer;
    }
    let key = job_key(&trade);
    let mut jobs = jobs.lock().unwrap_or_else(PoisonError::into_inner);
    jobs.prune();
    let retry = jobs
        .0
        .get(&key)
        .and_then(|job| job.latest.as_ref())
        .is_some_and(|outcome| {
            let (state, retry_at) = replay_state(Some(outcome));
            matches!(state, ChartStateDto::Error | ChartStateDto::CatchingUp)
                || retry_at.is_some_and(|at| at <= now)
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
    let age = job.at.elapsed();
    drop(jobs);
    let (state, retry_at) = replay_state(outcome.as_ref());
    if let Some(TradeReplayOutcome::Ready(series)) = outcome
        && state == ChartStateDto::Ok
    {
        let origin = if series.source == TradeReplaySource::CoreTicks {
            "archive"
        } else {
            "exchange"
        };
        let dto = trade_ok(
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
        return (dto, origin);
    }
    settle_trade(&mut dto, state, retry_at, Some(age), archive_lag, now);
    if dto.state == ChartStateDto::NoTape {
        dto.levels = Some(trade_levels(&trade));
    }
    (dto, "replay")
}

/// Read already-resolved stored tape in priority order without requiring a live catalog.
pub(super) fn stored_trade(
    trade: &MiniTrade,
    exchange: &str,
    value: TickValue,
    recorder: Result<tape_recorder::RecorderSpan, tape_recorder::TapeMiss>,
    terminal: impl FnOnce() -> Option<trade_replay::TickAnswer>,
) -> Option<(TradeTicksDto, &'static str)> {
    let dto = trade_base(trade);
    let (ticks, stage) = if let Ok(span) = recorder
        && !span.ticks.is_empty()
        && covers_trade(&span.covered, dto.open_ms, dto.close_ms)
    {
        (span.ticks, "recorder")
    } else {
        let held = terminal()?;
        if held.ticks.is_empty() || !held.covered.contains((dto.open_ms, dto.close_ms)) {
            return None;
        }
        (held.ticks, "terminal")
    };
    let slots = quote_slots(&ticks, value);
    Some((
        trade_ok(
            dto,
            trade,
            ticks,
            Some(slots),
            stage,
            exchange.to_owned(),
            TickThinning::Raw,
        ),
        stage,
    ))
}

/// Turn a replay state into the bounded answer the sheet acts on.
///
/// A job still loading past [`LOADING_PATIENCE`] waits behind other venue work (the recorder's
/// catch-up shares the host gate), so the sheet is moved to the slow catching-up cadence rather
/// than counting toward a hard error. An empty answer for a market whose venue publishes a daily
/// archive, while the trade's day is not published yet, is the file not being out: it reads as
/// archive-pending with the publication time, never as «no tape».
///
/// Args:
///     dto: Answer to fill; only `state`, `retry_in_ms` and `retry_at_ms` change.
///     state: State derived from the replay outcome or the pre-replay stage.
///     retry_at: Publication time the replay reported, when it did.
///     age: How long the replay job has been running; `None` before a job exists.
///     archive_lag_ms: The venue's daily-archive publication lag for this market, or `None`
///         when the venue publishes no daily archive for it.
///     now_ms: Current UTC Unix milliseconds.
pub(super) fn settle_trade(
    dto: &mut TradeTicksDto,
    state: ChartStateDto,
    retry_at: Option<i64>,
    age: Option<Duration>,
    archive_lag_ms: Option<i64>,
    now_ms: i64,
) {
    use ChartStateDto as State;
    let published = archive_lag_ms.map(|lag| archive_expected(dto.close_ms, lag));
    let (state, retry_at) = match state {
        State::Loading if age.is_some_and(|age| age >= LOADING_PATIENCE) => {
            (State::CatchingUp, None)
        }
        State::NoTape if published.is_some_and(|at| at > now_ms) => {
            (State::ArchivePending, published)
        }
        other => (other, retry_at),
    };
    dto.state = state;
    dto.retry_at_ms = retry_at;
    dto.retry_in_ms = match state {
        State::Loading => Some(LOADING_RETRY_MS),
        State::CatchingUp => Some(CATCHUP_RETRY_MS),
        _ => None,
    };
}

/// When the daily file holding `close_ms` is published: the next UTC midnight plus the venue's
/// lag, the same schedule the archive reader retries on.
pub(super) fn archive_expected(close_ms: i64, lag_ms: i64) -> i64 {
    close_ms
        .div_euclid(DAY_MS)
        .saturating_add(1)
        .saturating_mul(DAY_MS)
        .saturating_add(lag_ms)
}

/// Interpret tick availability explicitly; candle-only success is never a tick chart.
///
/// A refused host (rate limit, usually the recorder's catch-up spending the same budget) and a
/// core that is not connected yet are transient: both read as catching up, never as an error.
pub(super) fn replay_state(outcome: Option<&TradeReplayOutcome>) -> (ChartStateDto, Option<i64>) {
    use ChartStateDto as State;
    match outcome {
        None => (State::Loading, None),
        Some(TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited { .. }))
        | Some(TradeReplayOutcome::Empty(TradeReplayEmpty::CoreNotConnected)) => {
            (State::CatchingUp, None)
        }
        Some(TradeReplayOutcome::Failed(_)) => (State::Error, None),
        Some(TradeReplayOutcome::Empty(_)) => (State::NoTape, None),
        Some(TradeReplayOutcome::Ready(series)) => match series.tick_status {
            TickStatus::Pending | TickStatus::Streaming | TickStatus::AwaitingCore => {
                (State::Loading, None)
            }
            TickStatus::ArchivePending { retry_at_ms } => {
                (State::ArchivePending, Some(retry_at_ms))
            }
            TickStatus::RateLimited { .. } => (State::CatchingUp, None),
            TickStatus::Failed => (State::Error, None),
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
        log::info!("[mini-chart] order ticks state=Busy source=admission elapsed_ms=0");
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
    let snapshot = order_snapshot(host.state(), core, uid).cloned();
    begin_order_handoff(host.state_mut(), core, uid);
    let closing = order_closing(live.as_ref());
    let source = host.session().market_source();
    let zone = host.report_zone();
    let names = CoreNames::from_servers(&host.config().servers);
    let exchange = visible_cores(host, &access)
        .into_iter()
        .find(|(id, _, _)| *id == core)
        .map(|(_, _, exchange)| exchange)
        .unwrap_or_default();
    let service = Arc::clone(&host.state().mini_chart_jobs);
    host.spawn(Box::new(move || {
        let started = Instant::now();
        let now = moon_core::util::now_unix_ms_i64();
        let mut stage = "handoff";
        let coin = live
            .as_ref()
            .map(|row| row.coin.clone())
            .or_else(|| snapshot.as_ref().map(|snapshot| snapshot.coin.clone()))
            .unwrap_or_default();
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
                stage = read_live(&source, core, row, since_ms, &mut dto);
            }
            trade.map(|trade| (dto, trade))
        });
        log::info!(
            "[mini-chart] order ticks exchange={exchange} coin={coin} state={} source={stage} elapsed_ms={}",
            result
                .as_ref()
                .map_or_else(|error| format!("{error:?}"), |(dto, _)| format!("{:?}", dto.state)),
            started.elapsed().as_millis()
        );
        Box::new(move |host: &mut dyn TgHost| {
            if let Err(error) = finish_live_read(host.state_mut(), &service) {
                let _ = reply.try_send(Err(error));
                return;
            }
            let current = mini_access(host, chat);
            let visible = current.as_ref().is_some_and(|grant| {
                visible_cores(host, grant)
                    .iter()
                    .any(|(id, _, _)| *id == core)
            });
            let permitted = finish_grant(&access, current, visible);
            let result = permitted.and_then(|()| {
                result.map(|(mut dto, trade)| {
                    if let Some(snapshot) = &snapshot
                        && (trade.is_some() || (closing && snapshot.entry_ms <= 0)) {
                        finish_order_handoff(host.state_mut(), core, uid, snapshot.entry_ms);
                    }
                    dto.trade = trade
                        .as_ref()
                        .map(|trade| trade_dto(host, &core_quotes(&host.config().servers), &mut Default::default(), zone, now / 1000, trade));
                    dto
                })
            });
            let _ = reply.try_send(result);
        })
    }));
}

/// Admit shared core revisions; matching overlays are projected on the executor.
pub(super) fn mini_coin_ticks(
    host: &mut dyn TgHost,
    chat: i64,
    core: u64,
    market: String,
    tf: CoinTfDto,
    since_ms: Option<i64>,
    reply: SyncSender<Result<CoinTicksDto, MiniAppApiError>>,
) {
    let access = match authorize(host, chat, core) {
        Ok(access) => access,
        Err(error) => {
            let _ = reply.try_send(Err(error));
            return;
        }
    };
    if !claim_pending(&mut host.state_mut().mini_live_pending, 4) {
        log::info!("[mini-chart] coin ticks state=Busy source=admission elapsed_ms=0");
        let _ = reply.try_send(Err(MiniAppApiError::Busy));
        return;
    }
    let source = host.session().market_source();
    let listed = visible_cores(host, &access);
    let exchange = listed
        .iter()
        .find(|(id, _, _)| *id == core)
        .map(|(_, _, exchange)| exchange.clone())
        .unwrap_or_default();
    let mut captures = std::mem::take(&mut host.state_mut().mini_order_captures);
    let captured = coin_snapshot(
        &mut captures,
        host.session().store(),
        &listed,
        host.session().core_venues(),
        core,
    );
    host.state_mut().mini_order_captures = captures;
    let deal_ids = coin_deal_cores(
        &captured.ids,
        &core_quotes(&host.config().servers),
        &order_quote(&source.market_label(core, &market).quote, &market),
    );
    let zone = host.report_zone();
    let names = CoreNames::from_servers(&host.config().servers);
    let service = Arc::clone(&host.state().mini_chart_jobs);
    host.spawn(Box::new(move || {
        let started = Instant::now();
        let CoinSnapshot { ids, orders } = captured.project(&market);
        let market_display = orders.first().map(|(_, _, row)| row.market_display.clone())
            .filter(|name| !name.is_empty()).unwrap_or_else(|| market.clone());
        let coin = orders.first().map(|(_, _, row)| row.coin.clone())
            .unwrap_or_else(|| source.market_label(core, &market).coin);
        let now = moon_core::util::now_unix_ms_i64();
        let mut stage = "handoff";
        let result = chart_read(|| {
            let market_read = read_coin_market(tf, since_ms, |since| read_market(&source, core, &market, since, now));
            let data = market_read.data;
            stage = market_read.source;
            let ticks = data.as_ref().map_or(&[][..], |data| data.ticks.as_slice());
            let series = coin_series(&source, core, &market, tf, now, since_ms, ticks);
            let mark = source.latest_price(core, &market).ok().map(f64::from)
                .filter(|price| price.is_finite() && *price > 0.0)
                .or_else(|| ticks.last().map(|tick| f64::from(tick.price)))
                .or_else(|| orders.first().map(|(_, _, row)| f64::from(row.price)))
                .unwrap_or(0.0);
            let mut dto = CoinTicksDto {
                state: live_state(now, data.as_ref().and_then(|data| data.last_tick_ms)),
                now_ms: now,
                last_tick_ms: data.as_ref().and_then(|data| data.last_tick_ms),
                mark_price: mark,
                price_decimals: price_decimals(mark),
                truncated: data.as_ref().is_some_and(|data| data.truncated),
                exchange: exchange.clone(),
                market_display,
                open_price: series.candles.first().map(|candle| candle[1])
                    .or(market_read.open_price),
                ticks: series.ticks,
                candles: series.candles,
                ..Default::default()
            };
            for (id, name, row) in &orders {
                coin_overlays(&source, *id, name, row, mark, &mut dto);
            }
            let window_from = coin_deal_from(tf, now);
            let trades = crate::report::read_mini_trades(
                zone, TelegramReportAccess::Viewer(deal_ids), names, 50,
                Some(crate::report::MiniTradeWindow {
                    coin: coin.clone(), from: window_from.saturating_add(999).div_euclid(1000),
                    to: now.div_euclid(1000),
                }),
            ).or_else(|error| match error {
                moon_core::db::ReadFail::NotReady => Ok(Vec::new()),
                _ => Err(MiniAppApiError::ReadFailed),
            })?;
            dto.deals = trades.into_iter().map(|trade| CoinDealDto {
                core_name: trade.core_name,
                is_short: trade.is_short,
                profit_usd: trade.profit_usdt,
                profit_pct: trade.pct,
                open_ms: trade.buy_utc.map(|time| time.saturating_mul(1000)),
                close_ms: trade.close_utc.saturating_mul(1000),
                open_price: trade.buy_price,
                close_price: trade.sell_price,
            }).collect();
            Ok(dto)
        });
        log::info!("[mini-chart] coin ticks exchange={exchange} coin={market} tf={tf:?} state={} source={stage} elapsed_ms={}",
            result.as_ref().map_or_else(|error| format!("{error:?}"), |dto| format!("{:?}", dto.state)),
            started.elapsed().as_millis());
        Box::new(move |host: &mut dyn TgHost| {
            if let Err(error) = finish_live_read(host.state_mut(), &service) {
                let _ = reply.try_send(Err(error));
                return;
            }
            let current = mini_access(host, chat);
            let visible = current.as_ref().is_some_and(|grant| {
                let visible = visible_cores(host, grant);
                ids.iter().all(|id| visible.iter().any(|(current, _, _)| current == id))
            });
            let result = finish_grant(&access, current, visible).and(result);
            let _ = reply.try_send(result);
        })
    }));
}

/// Restrict history to granted cores whose configured quote matches this chart before SQL LIMIT.
pub(super) fn coin_deal_cores(
    ids: &[u64],
    quotes: &std::collections::HashMap<u64, String>,
    quote: &str,
) -> Vec<u64> {
    let quote = quote.trim();
    ids.iter()
        .copied()
        .filter(|id| {
            !quote.is_empty()
                && quotes
                    .get(id)
                    .is_some_and(|configured| configured.trim().eq_ignore_ascii_case(quote))
        })
        .collect()
}

/// Owned overlay rows and the exact venue-scoped history grant captured at admission.
pub(super) struct CoinSnapshot {
    pub ids: Vec<u64>,
    pub orders: Vec<(u64, String, CoinOrder)>,
}

/// Cheap venue-scoped handles captured by the owner, without any order-level projection.
pub(super) struct CoinCapture {
    pub ids: Vec<u64>,
    cores: Vec<super::order_capture::CoreOrders>,
}

impl CoinCapture {
    /// Build matching overlays only after the immutable handles have reached the executor.
    pub(super) fn project(self, market: &str) -> CoinSnapshot {
        let orders = self
            .cores
            .iter()
            .flat_map(|core| {
                core.rows
                    .iter()
                    .filter(|row| !row.job_is_done && row.market == market)
                    .map(|row| (core.core, core.name.clone(), CoinOrder::new(row)))
            })
            .collect();
        CoinSnapshot {
            ids: self.ids,
            orders,
        }
    }
}

/// Only the fields consumed by a coin overlay; order traces never leave the owner with it.
pub(super) struct CoinOrder {
    coin: String,
    market: String,
    market_display: String,
    quote: String,
    price: f32,
    buy_price: f64,
    size: f64,
    quantity: Option<f64>,
    pending: bool,
    is_short: bool,
    strat_name: String,
    take_profit: Option<f64>,
    stop_loss: Option<f64>,
    levels: Vec<LevelDto>,
}

impl CoinOrder {
    /// Project the canonical position gate and levels from a frozen row on the executor.
    pub(super) fn new(row: &OrderRow) -> Self {
        Self {
            coin: row.coin.clone(),
            market: row.market.clone(),
            market_display: row.market_display.clone(),
            quote: row.quote.clone(),
            price: row.price,
            buy_price: row.buy_price,
            size: row.size,
            quantity: moon_core::feed::order_math::position_qty(row),
            pending: row.pending,
            is_short: row.is_short,
            strat_name: row.strat_name.clone(),
            take_profit: row.take_profit,
            stop_loss: row.stop_loss,
            levels: live_levels(row),
        }
    }
}

/// Capture visible core revisions on the source venue; the executor selects the market.
pub(super) fn coin_snapshot(
    captures: &mut super::order_capture::OrderCaptures,
    store: &moon_core::session::CoreStore,
    listed: &[(u64, String, String)],
    venues: &std::collections::HashMap<u64, moon_core::venue::CoreVenue>,
    core: u64,
) -> CoinCapture {
    let venue = venues.get(&core);
    let peers: Vec<_> = listed
        .iter()
        .filter(|(id, _, _)| {
            *id == core
                || venue.is_some_and(|venue| venues.get(id).is_some_and(|peer| peer.id == venue.id))
        })
        .cloned()
        .collect();
    let ids = peers.iter().map(|(id, _, _)| *id).collect();
    let cores = captures.capture(store, &peers);
    CoinCapture { ids, cores }
}

/// Tag existing level prices and position arithmetic with their originating core.
pub(super) fn coin_overlays(
    source: &MarketDataSource,
    core: u64,
    name: &str,
    row: &CoinOrder,
    mark: f64,
    dto: &mut CoinTicksDto,
) {
    dto.levels
        .extend(row.levels.iter().map(|level| CoinLevelDto {
            kind: if row.pending && level.kind == LevelKindDto::Entry {
                LevelKindDto::Buy
            } else {
                level.kind
            },
            price: level.price,
            core: core.to_string(),
            core_name: name.to_string(),
            is_short: row.is_short,
            pending: row.pending,
        }));
    let volume = source
        .currency_usd_rate(core, &order_quote(&row.quote, &row.market))
        .map(|rate| row.size * row.buy_price * rate)
        .filter(|value| value.is_finite() && *value > 0.0);
    let pnl_pct = if row.pending {
        moon_core::feed::order_math::entry_distance_pct(row.buy_price, mark, row.is_short)
    } else {
        moon_core::feed::order_math::pnl_at_mark(
            row.buy_price,
            mark as f32,
            row.quantity,
            row.is_short,
        )
        .1
        .filter(|value| value.is_finite())
    };
    dto.positions.push(CoinPositionDto {
        core: core.to_string(),
        core_name: name.to_string(),
        is_short: row.is_short,
        pending: row.pending,
        entry: row.buy_price,
        take: row.take_profit,
        stop: row.stop_loss,
        strategy: row.strat_name.clone(),
        volume_usd: volume,
        pnl_pct,
    });
}

/// Wire series keep incremental prints separate from replacement candle history.
pub(super) struct CoinSeries {
    pub ticks: Vec<(i64, f64, f64, u8)>,
    pub candles: Vec<[f64; 6]>,
}

/// Apply the client cursor only to tick payloads, never to candle input history.
pub(super) fn coin_series(
    source: &MarketDataSource,
    core: u64,
    market: &str,
    tf: CoinTfDto,
    now: i64,
    since: Option<i64>,
    ticks: &[Tick],
) -> CoinSeries {
    let candles = coin_candles(source, core, market, tf, now, ticks);
    let from = live_from(now, since);
    let ticks = wire_ticks(
        ticks
            .iter()
            .filter(|tick| tick.time_ms >= from as f64)
            .copied()
            .collect(),
    );
    CoinSeries { ticks, candles }
}

/// Maximum raw prints retained while rebuilding candle buckets beyond the cached prefix.
pub(super) const MAX_COIN_CANDLE_TICKS: usize = 200_000;

/// Read cached candle history first, then a bounded raw tail using the shared builders.
pub(super) fn coin_candles(
    source: &MarketDataSource,
    core: u64,
    market: &str,
    tf: CoinTfDto,
    now: i64,
    ticks: &[Tick],
) -> Vec<[f64; 6]> {
    if tf == CoinTfDto::Ticks {
        return Vec::new();
    }
    let window = coin_candle_window(tf, now);
    let (from, _) = window;
    let cached = source
        .kline_cache()
        .and_then(|cache| {
            let address = source.replay_address(core).ok()?;
            cache.read_range(
                &address.exchange_key,
                market,
                if tf == CoinTfDto::M1 { 1 } else { 5 },
                from,
                now,
            )
        })
        .unwrap_or_default();
    let width = window.1;
    coin_candle_tail(&cached, ticks, window, now, |(from, to, limit)| {
        // One bucket of lookback lets a ring read prove it reaches back to the bucket open.
        let (data, _, covered) = read_market_span(
            source,
            core,
            market,
            None,
            (from.saturating_sub(width), to, limit),
            tape_recorder::watch_market,
        );
        let data = data?;
        let covered = covered.or_else(|| data.ticks.first().map(|tick| tick.time_ms as i64));
        Some((data, covered))
    })
}

/// Rebuild only the last cached bucket and its successors, retaining at most the newest raw cap.
pub(super) fn coin_candle_tail(
    cached: &[moon_core::market::candles::ChartCandle],
    fallback: &[Tick],
    window: (i64, i64),
    now: i64,
    read: impl FnOnce((i64, i64, usize)) -> Option<(moon_core::market::LiveTicks, Option<i64>)>,
) -> Vec<[f64; 6]> {
    let (from, width) = window;
    // Include the final cached bucket because it may still be forming.
    let tail_from = cached.last().map_or(from, |bar| {
        (bar.t_open_ms as i64)
            .div_euclid(width)
            .saturating_mul(width)
            .max(from)
    });
    let history = read((tail_from, now, MAX_COIN_CANDLE_TICKS));
    let ticks = history
        .as_ref()
        .map_or(fallback, |(history, _)| history.ticks.as_slice());
    let first = ticks.partition_point(|tick| tick.time_ms < tail_from as f64);
    let end = ticks.partition_point(|tick| tick.time_ms <= now as f64);
    let capped = end.saturating_sub(MAX_COIN_CANDLE_TICKS);
    let covered = match &history {
        // A capped read only covers from its oldest retained print.
        Some((history, _)) if history.truncated || capped > first => {
            ticks.get(capped).map(|tick| tick.time_ms as i64)
        }
        Some((_, covered)) => *covered,
        None => fallback.first().map(|tick| tick.time_ms as i64),
    };
    let first = first.max(capped);
    merge_coin_candles(cached, &ticks[first..end], covered, window, now)
}

/// Preserve complete cached buckets when the source's coverage starts partway through their bar.
pub(super) fn merge_coin_candles(
    cached: &[moon_core::market::candles::ChartCandle],
    ticks: &[Tick],
    covered_from: Option<i64>,
    window: (i64, i64),
    now: i64,
) -> Vec<[f64; 6]> {
    use moon_core::market::candles::{aggregate_trades, resample};
    let (from, width) = window;
    let mut base = Vec::new();
    resample(cached, width, &mut base);
    let mut tail = Vec::new();
    aggregate_trades(ticks, 60_000, &mut tail);
    let mut resampled = Vec::new();
    resample(&tail, width, &mut resampled);
    // Both builders return sorted lists; only fully observed live buckets replace cache.
    let mut cached = base.into_iter().peekable();
    let mut live = resampled.into_iter().peekable();
    let mut merged = Vec::new();
    while let (Some(left), Some(right)) = (cached.peek(), live.peek()) {
        if left.t_open_ms < right.t_open_ms {
            merged.extend(cached.next());
        } else {
            if left.t_open_ms == right.t_open_ms {
                if covered_from.is_none_or(|from| from as f64 > right.t_open_ms) {
                    merged.extend(cached.next());
                    let _ = live.next();
                    continue;
                }
                let _ = cached.next();
            }
            merged.extend(live.next());
        }
    }
    merged.extend(cached);
    merged.extend(live);
    merged
        .into_iter()
        .filter(|row| row.t_open_ms >= from as f64 && row.t_open_ms <= now as f64)
        .map(|row| {
            [
                row.t_open_ms,
                f64::from(row.open),
                f64::from(row.high),
                f64::from(row.low),
                f64::from(row.close),
                f64::from(row.volume),
            ]
        })
        .collect()
}

/// Market delta and its independently sampled window opening, with streaming source provenance.
pub(super) struct CoinMarketRead {
    pub data: Option<moon_core::market::LiveTicks>,
    pub open_price: Option<f64>,
    pub source: &'static str,
}

/// Apply tick cursors before source caps; only the first read carries the window's opening.
pub(super) fn read_coin_market(
    tf: CoinTfDto,
    since: Option<i64>,
    read: impl FnOnce(Option<i64>) -> (Option<moon_core::market::LiveTicks>, &'static str),
) -> CoinMarketRead {
    let cursor = if tf == CoinTfDto::Ticks { since } else { None };
    let (data, source) = read(cursor);
    // The sheet keeps the first reply's opening, so a cursor read never re-scans the window.
    let open_price = if cursor.is_some() {
        None
    } else {
        data.as_ref()
            .and_then(|window| window.ticks.first())
            .map(|tick| f64::from(tick.price))
    };
    CoinMarketRead {
        data,
        open_price,
        source,
    }
}

/// Give tick charts one hour of closed deals while candle charts retain their selected history.
pub(super) fn coin_deal_from(tf: CoinTfDto, now: i64) -> i64 {
    if tf == CoinTfDto::Ticks {
        now.saturating_sub(3_600_000)
    } else {
        coin_candle_window(tf, now).0
    }
}

/// Align a candle window to full bucket boundaries, including the current forming bucket.
pub(super) fn coin_candle_window(tf: CoinTfDto, now: i64) -> (i64, i64) {
    let (width, bars) = match tf {
        CoinTfDto::Ticks => (1, 0),
        CoinTfDto::M1 => (60_000, 120),
        CoinTfDto::M5 => (300_000, 100),
        CoinTfDto::M15 => (900_000, 100),
    };
    (
        now.div_euclid(width)
            .saturating_sub(bars - 1)
            .saturating_mul(width),
        width,
    )
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
///
/// A ring that holds no print of the market (the core never subscribed it, as on a station
/// right after a restart) counts as absent, so the recorder is armed and asked instead of the
/// sheet staying on an empty chart.
///
/// Returns:
///     The source that answered (`ring`, `recorder` or `none`), for the request's log line.
fn read_live(
    source: &MarketDataSource,
    core: u64,
    row: &OrderRow,
    since: Option<i64>,
    dto: &mut LiveTicksDto,
) -> &'static str {
    let (data, used) = read_market(source, core, &row.market, since, dto.now_ms);
    if let Some(data) = data {
        dto.last_tick_ms = data.last_tick_ms;
        dto.truncated = data.truncated;
        dto.ticks = wire_ticks(data.ticks);
    }
    dto.state = live_state(dto.now_ms, dto.last_tick_ms);
    fill_mark(source, core, row, dto);
    used
}

/// Read a bounded market delta from the ring, then renew the recorder's expiring demand.
pub(super) fn read_market(
    source: &MarketDataSource,
    core: u64,
    market: &str,
    since: Option<i64>,
    now: i64,
) -> (Option<moon_core::market::LiveTicks>, &'static str) {
    read_market_with_watch(
        source,
        core,
        market,
        since,
        now,
        tape_recorder::watch_market,
    )
}

/// Share source selection while allowing a fixture to observe the recorder demand request.
pub(super) fn read_market_with_watch(
    source: &MarketDataSource,
    core: u64,
    market: &str,
    since: Option<i64>,
    now: i64,
    watch: impl FnOnce(&str, &str, i64),
) -> (Option<moon_core::market::LiveTicks>, &'static str) {
    read_market_window(
        source,
        core,
        market,
        since,
        (live_from(now, since), now, MAX_LIVE_TICKS),
        watch,
    )
}

/// Share ring and recorder selection while allowing candle history its own time and row bounds.
fn read_market_window(
    source: &MarketDataSource,
    core: u64,
    market: &str,
    since: Option<i64>,
    window: (i64, i64, usize),
    watch: impl FnOnce(&str, &str, i64),
) -> (Option<moon_core::market::LiveTicks>, &'static str) {
    let (data, used, _) = read_market_span(source, core, market, since, window, watch);
    (data, used)
}

/// Read a market window and, from the recorder, the start of its newest contiguous coverage.
fn read_market_span(
    source: &MarketDataSource,
    core: u64,
    market: &str,
    since: Option<i64>,
    window: (i64, i64, usize),
    watch: impl FnOnce(&str, &str, i64),
) -> (
    Option<moon_core::market::LiveTicks>,
    &'static str,
    Option<i64>,
) {
    let (from, now, limit) = window;
    let mut used = "ring";
    let mut covered = None;
    let data = source
        .live_core_ticks(core, market, from, now, limit)
        .filter(|ring| ring.last_tick_ms.is_some())
        .or_else(|| {
            used = "recorder";
            let address = source.replay_address(core).ok()?;
            watch(&address.exchange_key, market, now.saturating_add(30_000));
            let span = tape_recorder::market_tape(
                &address.exchange_key,
                market,
                from,
                now,
                Duration::from_millis(1500),
            )
            .ok()?;
            covered = span.covered.last().map(|&(start, _)| start);
            let start = recorder_start(since, &span.covered, from);
            let mut delta = recorder_delta(span.ticks, start, now, limit);
            delta.last_tick_ms = delta.last_tick_ms.or(since);
            Some(delta)
        });
    if data.is_none() {
        used = "none";
    }
    (data, used, covered)
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

/// First millisecond a recorder read hands the sheet.
///
/// The sheet's first read (`since` absent) seeds the chart with every span the recorder already
/// holds in the window, so it is not empty while a fresh watch catches up. Later deltas stay on
/// the newest covered span, so a hole is never filled in behind the cursor.
///
/// Args:
///     since: The sheet's inclusive cursor; `None` on its first read.
///     covered: Recorder spans, oldest first.
///     from: Oldest millisecond of the live window.
///
/// Returns:
///     The start of the delta.
pub(super) fn recorder_start(since: Option<i64>, covered: &[(i64, i64)], from: i64) -> i64 {
    if since.is_none() {
        return from;
    }
    covered.last().map_or(from, |&(start, _)| start.max(from))
}

/// Retain freshness before excluding an acknowledged delta and dropping its older overflow.
pub(super) fn recorder_delta(
    mut ticks: Vec<Tick>,
    from: i64,
    now: i64,
    limit: usize,
) -> moon_core::market::LiveTicks {
    debug_assert!(
        ticks
            .windows(2)
            .all(|pair| pair[0].time_ms <= pair[1].time_ms)
    );
    let last_tick_ms = ticks.last().map(|tick| tick.time_ms as i64);
    ticks.retain(|tick| tick.time_ms >= from as f64 && tick.time_ms <= now as f64);
    let truncated = ticks.len() > limit;
    let older = ticks.len().saturating_sub(limit);
    ticks.drain(..older);
    moon_core::market::LiveTicks {
        ticks,
        truncated,
        last_tick_ms,
    }
}
