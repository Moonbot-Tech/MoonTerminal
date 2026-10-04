//! Deal charts: a picture of a closed trade's tape, sent to a chat whose chart rule its result
//! passes (`ChartRule`, LinKvo 04.10).
//!
//! The trade read of [`super::tick`] decides which trades are due ([`super::trades::decide_charts`])
//! once their dollar value is known, and hands them here in memory. One drawing job at a time then
//! asks the tape recorder for each trade's prints — at least ten seconds before the entry and
//! three after the exit ([`window`]) — waits while the recorder has not seen the stream pass that
//! end (or is busy,
//! or has not seen the close yet) and, for a shorter while, for the core's answer on the trade's
//! order lines, and draws the picture ([`crate::deal_chart`]) into
//! the charts folder beside the notifications file. The owner thread then queues one photo row per
//! chat; the sender uploads it with the trade's card as the caption.
//!
//! The tape is the recorder's (`moon_core::market::tape_recorder`), which the station always runs
//! and the terminal only with `channels.tape_recorder` on. A trade the recorder does not remember —
//! it was off, or the process restarted after the close — gets no picture. So does a trade whose
//! picture was decided but not drawn when the process stopped: the queue lives in memory.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono_tz::Tz;
use moon_core::db::CoreNames;
use moon_core::db::order_traces::{self, TraceEntry};
use moon_core::feed::ArchivedLineKind;
use moon_core::market::tape_recorder::{self, TapeMiss, TradeId, TradeTape};
use moon_core::telegram::runtime::{NotifyStore, chart_spool_dir, push_photo};

use crate::deal_chart::{DealChart, render};
use crate::notify::render::trade_card;
use crate::notify::tick::lock_store;
use crate::notify::trades::ClosedTrade;
use crate::{Finish, Job, TgHost};

/// Minimum gap between two drawing jobs.
const CHART_INTERVAL: Duration = Duration::from_secs(1);
/// The least of the run-up the picture shows before the entry (LinKvo 04.10). A long trade shows
/// a third of its length, up to the recorder's margin — it keeps no more before an entry.
const LEAD_MIN_MS: i64 = 10_000;
/// The most run-up any picture shows, whatever the margin: the recorder reads the asked stretch on
/// its own thread, and an hour of a busy market is not a picture.
const LEAD_CAP_MS: i64 = 600_000;
/// How much the picture shows after the exit (LinKvo 04.10).
const TAIL_MS: i64 = 3_000;
/// How long a picture waits for the stream to pass its last second before it is drawn with what
/// there is, and how long a recorder that does not answer for the trade is asked again before the
/// picture is given up.
const WAIT: Duration = Duration::from_secs(90);
/// How long the order lines are waited for: a core that never answers for them — too old for the
/// request, or its archive failed — must not hold every picture for the whole [`WAIT`].
const LINES_WAIT: Duration = Duration::from_secs(20);
/// How long the recorder may take to answer.
const PEEK_TIMEOUT: Duration = Duration::from_secs(3);

/// A trade whose picture a chat is due.
#[derive(Clone, Debug)]
pub(crate) struct Due {
    pub chat: i64,
    pub trade: ClosedTrade,
    /// When the trade read decided it.
    pub decided: Instant,
}

/// A picture drawn, waiting for the owner thread to queue it.
struct Drawn {
    chats: Vec<i64>,
    core: u64,
    file: String,
    caption: String,
}

/// What a drawing job hands back.
struct Outcome {
    drawn: Vec<Drawn>,
    /// Not ready yet: queued again.
    waiting: Vec<Due>,
}

/// Start a drawing job when pictures are queued, none is in flight, and the interval is open.
///
/// Args:
///     host: The queue, the busy flag and the spawn hook.
///     store: Notifications file the finished job queues into.
///     now_utc: Current UTC Unix seconds, stored on the queued rows.
pub(crate) fn run(host: &mut dyn TgHost, store: &Arc<Mutex<NotifyStore>>, now_utc: i64) {
    let state = host.state();
    if state.charts_busy
        || state.chart_queue.is_empty()
        || state
            .last_chart_run
            .is_some_and(|at| at.elapsed() < CHART_INTERVAL)
    {
        return;
    }
    let zone = host.report_zone();
    let names = CoreNames::from_servers(&host.config().servers);
    let spool = chart_spool_dir(&host.notifications_path());
    let store = Arc::clone(store);
    let state = host.state_mut();
    let due = std::mem::take(&mut state.chart_queue);
    state.charts_busy = true;
    state.last_chart_run = Some(Instant::now());
    let job: Job = Box::new(move || {
        // A picture that panics costs this job's pictures, never the busy flag: the finish
        // always comes back.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            draw_all(due, zone, names, &spool)
        }))
        .unwrap_or_else(|_| {
            log::error!("telegram charts: drawing panicked, this job's pictures are lost");
            Outcome {
                drawn: Vec::new(),
                waiting: Vec::new(),
            }
        });
        let finish: Finish = Box::new(move |host| finish(host, &store, outcome, now_utc));
        finish
    });
    host.spawn(job);
}

/// Queue the drawn pictures, put back the ones still waiting, and allow the next job. A picture
/// no chat took — every chat switched its charts off meanwhile, or the save failed — is deleted
/// at once.
fn finish(host: &mut dyn TgHost, store: &Mutex<NotifyStore>, outcome: Outcome, now_utc: i64) {
    let mut guard = lock_store(store);
    let queued = guard.update(|file| {
        let mut taken = Vec::new();
        for drawn in &outcome.drawn {
            for &chat in &drawn.chats {
                // A chat that switched its charts off meanwhile does not get them.
                if !file.chats.get(&chat).is_some_and(|c| c.settings.charts.on) {
                    continue;
                }
                push_photo(
                    file,
                    chat,
                    drawn.caption.clone(),
                    Some(vec![drawn.core]),
                    drawn.file.clone(),
                    now_utc,
                );
                taken.push(drawn.file.as_str());
            }
        }
        taken.len()
    });
    let queued = queued.unwrap_or_else(|error| {
        log::warn!("telegram charts not queued: {error}");
        0
    });
    for drawn in &outcome.drawn {
        guard.drop_chart(&drawn.file);
    }
    drop(guard);
    log::debug!(
        "telegram charts: {queued} queued, {} still waiting",
        outcome.waiting.len()
    );
    let state = host.state_mut();
    state.chart_queue.extend(outcome.waiting);
    state.charts_busy = false;
}

/// Draw every picture that is ready; one trade due in several chats is drawn once.
fn draw_all(due: Vec<Due>, zone: Tz, names: CoreNames, spool: &Path) -> Outcome {
    let mut by_trade: BTreeMap<(u64, i64), Vec<Due>> = BTreeMap::new();
    for entry in due {
        by_trade
            .entry((entry.trade.core, entry.trade.rec_id))
            .or_default()
            .push(entry);
    }
    let mut outcome = Outcome {
        drawn: Vec::new(),
        waiting: Vec::new(),
    };
    let mut day = DaySums::new(zone, names);
    for ((core, rec_id), entries) in by_trade {
        let trade = entries[0].trade.clone();
        let decided = entries
            .iter()
            .map(|e| e.decided)
            .min()
            .unwrap_or_else(Instant::now);
        let waited = decided.elapsed();
        let id = TradeId { core, rec_id };
        // Asked for the longest run-up a picture may show; [`window`] cuts it to this trade's.
        let lead = run_up_ms();
        let tape = match tape_recorder::trade_tape(id, lead, TAIL_MS, PEEK_TIMEOUT) {
            Ok(tape) => tape,
            // The recorder may not have its close yet — the replica can land first — or be busy:
            // asked again until the wait runs out.
            Err(_) if waited < WAIT => {
                outcome.waiting.extend(entries);
                continue;
            }
            Err(miss) => {
                let why = match miss {
                    TapeMiss::Unknown => "the tape recorder holds nothing of it",
                    TapeMiss::Busy => "the tape recorder did not answer",
                };
                log::info!(
                    "telegram chart {} core {core} trade {rec_id}: {why}, no picture",
                    trade.coin
                );
                continue;
            }
        };
        let lines = trade_lines(core, trade.report_uid);
        let lines_due = lines.is_some() || waited >= LINES_WAIT;
        if waited < WAIT && !(tape.settled && lines_due) {
            outcome.waiting.extend(entries);
            continue;
        }
        let lines = lines.unwrap_or_default();
        if lines.unreadable {
            log::warn!(
                "telegram chart {} core {core} trade {rec_id}: order traces unreadable, drawn without order lines",
                trade.coin
            );
        }
        let day_usd = day.of(core, trade.close_utc);
        let Some(file) = draw_one(&trade, &tape, &lines, day_usd, zone, spool) else {
            continue;
        };
        outcome.drawn.push(Drawn {
            chats: entries.iter().map(|e| e.chat).collect(),
            core,
            file,
            caption: trade_card(&trade, false),
        });
    }
    outcome
}

/// The order lines and the stop the core archived for the trade. `None` while the store has no
/// answer for it yet; empty lines for a trade without a `ReportUID`, an empty answer, or a store
/// that cannot be read. The stop is drawn across the window, as on the reference picture.
fn trade_lines(core: u64, report_uid: Option<i64>) -> Option<Lines> {
    let Some(uid) = report_uid else {
        return Some(Lines::default());
    };
    match order_traces::read_many(core, &[uid]) {
        Ok(mut found) => match found.remove(&uid)? {
            TraceEntry::Lines(traces) => Some(Lines::of(&traces)),
            TraceEntry::Empty { .. } => Some(Lines::default()),
        },
        Err(error) => {
            // Asked again every second while the picture waits: said once, when it is drawn.
            log::debug!("telegram chart: order traces of {uid} unreadable: {error:?}");
            Some(Lines {
                unreadable: true,
                ..Lines::default()
            })
        }
    }
}

/// What the picture draws of the trade's orders.
#[derive(Default)]
struct Lines {
    entry: Vec<(i64, f64)>,
    exit: Vec<(i64, f64)>,
    stop: Option<f64>,
    /// The store could not be read: the picture goes without lines, and says so in the log.
    unreadable: bool,
}

impl Lines {
    /// The trade's own entry and exit lines — not an ancestor's, inherited through a join — and
    /// the stop, the exit's before the entry's.
    fn of(traces: &[moon_core::feed::ArchivedOrderTrace]) -> Self {
        let own = |kind: ArchivedLineKind| {
            traces
                .iter()
                .find(|t| t.own && t.kind == kind)
                .map(|t| {
                    t.points
                        .iter()
                        .filter(|(ms, _)| ms.is_finite())
                        .map(|&(ms, price)| (ms as i64, price))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let stop = [ArchivedLineKind::Exit, ArchivedLineKind::Entry]
            .into_iter()
            .find_map(|kind| {
                traces
                    .iter()
                    .filter(|t| t.own && t.kind == kind)
                    .find_map(|t| t.stop_price)
            })
            .filter(|price| price.is_finite() && *price > 0.0);
        Self {
            entry: own(ArchivedLineKind::Entry),
            exit: own(ArchivedLineKind::Exit),
            stop,
            unreadable: false,
        }
    }
}

/// Draw one trade and write its PNG; the file's name, or `None` when nothing was drawn or written.
fn draw_one(
    trade: &ClosedTrade,
    tape: &TradeTape,
    lines: &Lines,
    day_usd: Option<f64>,
    zone: Tz,
    spool: &Path,
) -> Option<String> {
    let profit = trade.rule_profit_usd()?;
    let caption = crate::deal_chart::caption(profit, trade.profit_pct, &trade.strategy);
    let chart = DealChart {
        market: &tape.market,
        base: &trade.coin,
        short: trade.short,
        entry: (tape.open_ms, trade.buy_price.unwrap_or(0.0)),
        exit: (tape.close_ms, trade.sell_price.unwrap_or(0.0)),
        stop: lines.stop,
        entry_line: &lines.entry,
        exit_line: &lines.exit,
        caption: &caption,
        won: crate::deal_chart::won(profit),
        spent_usd: trade.rule_volume_usd(),
        day_usd,
        ticks: &tape.ticks,
        window: window(tape.open_ms, tape.close_ms, run_up_ms()),
        zone,
    };
    let started = Instant::now();
    let Some(png) = render(&chart) else {
        log::info!(
            "telegram chart {}: {} print(s) draw no picture",
            tape.market,
            tape.ticks.len()
        );
        return None;
    };
    let name = format!(
        "{}-{}-{}.png",
        trade.core,
        trade.rec_id,
        moon_core::util::now_unix_ms_i64()
    );
    if let Err(error) =
        moon_core::config::write_file_atomic(&spool.join(&name), &png, "telegram chart")
    {
        log::warn!("telegram chart {} not written: {error:#}", tape.market);
        return None;
    }
    log::info!(
        "telegram chart {}: {} print(s), {} KB in {} ms",
        tape.market,
        tape.ticks.len(),
        png.len() / 1024,
        started.elapsed().as_millis()
    );
    Some(name)
}

/// The most run-up a picture may show: the recorder's margin before an entry, at least
/// [`LEAD_MIN_MS`].
fn run_up_ms() -> i64 {
    moon_core::market::trade_replay::margin_ms().clamp(LEAD_MIN_MS, LEAD_CAP_MS)
}

/// The stretch a trade's picture shows: a third of the trade's length before the entry — at least
/// [`LEAD_MIN_MS`], at most `max_lead_ms` (what the recorder keeps) — to [`TAIL_MS`] after the
/// exit. The frame is the trade's, not its tape's: a quiet market's few prints must not squeeze
/// the trade to an edge.
fn window(open_ms: i64, close_ms: i64, max_lead_ms: i64) -> (i64, i64) {
    let lead =
        (close_ms.saturating_sub(open_ms) / 3).clamp(LEAD_MIN_MS, max_lead_ms.max(LEAD_MIN_MS));
    (
        open_ms.saturating_sub(lead),
        close_ms.max(open_ms).saturating_add(TAIL_MS),
    )
}

/// Each core's dollar result on the day a trade closed, up to that trade: the replica is read
/// once per job, from the earliest midnight a picture asks for, and only when one is drawn. A read
/// that failed is not tried again in the same job.
struct DaySums {
    zone: Tz,
    names: CoreNames,
    /// The midnight the read started from, and the closed trades since.
    read: Option<(i64, Vec<ClosedTrade>)>,
    failed: bool,
}

impl DaySums {
    fn new(zone: Tz, names: CoreNames) -> Self {
        Self {
            zone,
            names,
            read: None,
            failed: false,
        }
    }

    /// `core`'s day as of a trade closed at `close_utc`: the sum of its trades' dollar profit
    /// closed from that day's midnight in the zone up to `close_utc`. `None` when one of them has
    /// no dollar value yet or the report cannot be read — a sum missing a trade would be a wrong
    /// number on the picture.
    fn of(&mut self, core: u64, close_utc: i64) -> Option<f64> {
        let midnight = midnight_utc(self.zone, close_utc)?;
        if self.failed {
            return None;
        }
        if self.read.as_ref().is_none_or(|(from, _)| *from > midnight) {
            match crate::report::read_closed_since(self.zone, self.names.clone(), midnight) {
                Ok(trades) => self.read = Some((midnight, trades)),
                Err(error) => {
                    log::debug!("telegram chart: the day's trades unreadable: {error:?}");
                    self.failed = true;
                    return None;
                }
            }
        }
        let (_, trades) = self.read.as_ref()?;
        day_sum(trades, core, midnight, close_utc)
    }
}

/// The start of the day `at_utc` falls on in `zone`, UTC Unix seconds.
fn midnight_utc(zone: Tz, at_utc: i64) -> Option<i64> {
    let day = chrono::DateTime::from_timestamp(at_utc, 0)?
        .with_timezone(&zone)
        .date_naive();
    moon_core::util::display_time::day_start(day, zone)
}

/// The dollar profit of `core`'s trades closed in `[from_utc, to_utc]`, or `None` when one of
/// them is unvalued.
fn day_sum(trades: &[ClosedTrade], core: u64, from_utc: i64, to_utc: i64) -> Option<f64> {
    trades
        .iter()
        .filter(|t| t.core == core && (from_utc..=to_utc).contains(&t.close_utc))
        .map(ClosedTrade::rule_profit_usd)
        .sum()
}

#[cfg(test)]
mod tests;
