//! The startup autoload: the tape of recent closed trades the close-time capture missed,
//! fetched on the terminal's own initiative once the cores are up.
//!
//! The close-time capture (`session::lifecycle::capture_closed_trade`) files a trade's prints
//! out of the core's ring the moment it closes — but only while the terminal is running. What
//! closed in between is gone from the ring by the next launch and can only come from the venue,
//! and the tuner's axis would otherwise show those rows as missing until someone pressed
//! "Fetch trades". Behind `[trade_replay] autoload_missing` (off by default: it spends the
//! venues' public budget unasked), this runs ONCE per process, from the coordination tick:
//!
//! 1. read every closed trade with millisecond stamps of the last [`HORIZON_MS`] across every
//!    core, under the axis' own filter — strategy trades the tuner can be run on;
//! 2. resolve each through the live source, keep the ones the venue still serves by the
//!    worker's own retention rule (`inside_retention`: the exit inside the route's retention; a
//!    venue with no route is skipped — nothing to ask), and hand them to the fetch job
//!    ([`super::job::enqueue`]) — the job asks the
//!    worker, and the worker serves what the tiles and `trades.sqlite` already hold without a
//!    request, so a trade the capture DID file costs nothing here;
//! 3. a trade whose core is not connected yet, or whose catalog is not in, is kept and tried
//!    again every [`RETRY`] for up to [`MAX_ATTEMPTS`]: the cores come up one by one after the
//!    terminal, and the catalog a little after each core.
//!
//! The first pass yields to the startup cleanup of the trade tape
//! (`settings::trades_cleanup_startup`, behind `[trade_replay] cleanup_at_startup`): the
//! cleanup cuts the file to what the tuner's rows claim, this then fetches what they still
//! lack — the other order would fetch first and cut second.
//!
//! The read and the resolution run on the background executor; the tick only decides whether
//! one is due. "Stop" on the axis' button cancels what remains ([`cancel`]); flipping the
//! switch off and on again re-arms it.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::App;

use super::job;
use super::{FetchResolver, strategy_field_defaults};
use crate::Backend;
use moon_core::db::tuner::ticks::{Deal, model_window};
use moon_core::market::trade_replay::venue_caps::trade_route;
use moon_core::market::trade_replay::worker::inside_retention;
use moon_core::market::trade_replay::{long_position_ms, model_margin_ms};

/// How far back the autoload looks, whatever the venue documents: the longest retention a
/// route names is 90 days, and a month of rows is already thousands of walks.
const HORIZON_MS: i64 = 30 * 24 * 3_600_000;

/// How long the first pass waits after the tick first finds the switch on — for the cores to
/// come up and report their catalogs, so the first pass resolves most rows at once.
const FIRST_DELAY: Duration = Duration::from_secs(20);

/// Between passes over the rows still unresolved.
const RETRY: Duration = Duration::from_secs(30);

/// Passes before the rows still unresolved are given up on: ten minutes of cores not coming.
const MAX_ATTEMPTS: u32 = 20;

/// Where the autoload stands.
#[derive(Default)]
enum Phase {
    /// The switch has not been seen on yet, or was seen off since.
    #[default]
    Armed,
    /// Waiting for the next pass, with what is left to resolve (`None` before the first read).
    Waiting {
        due: Instant,
        left: Option<Vec<Deal>>,
        attempts: u32,
    },
    /// A pass is on the background executor.
    Running,
    /// Every row was handed over or given up on, or the user stopped it.
    Done,
}

#[derive(Default)]
struct Autoload {
    phase: Phase,
    /// What the running pass hands back: the rows still unresolved, and the pass count.
    result: Option<(Vec<Deal>, u32)>,
    /// Bumped by every start of a pass, by a cancel and by the switch going off: a pass carries
    /// the generation it started under and is heard only while it is still the current one —
    /// a pass the user stopped, or that the switch outlived, neither enqueues nor reports.
    generation: u64,
}

static AUTOLOAD: OnceLock<Mutex<Autoload>> = OnceLock::new();

fn lock() -> std::sync::MutexGuard<'static, Autoload> {
    AUTOLOAD
        .get_or_init(|| Mutex::new(Autoload::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Stop adding rows: what the job already has stays the job's to finish or to drop, and a pass
/// still running adds nothing when it comes back.
pub(crate) fn cancel() {
    let mut st = lock();
    if !matches!(st.phase, Phase::Armed) {
        st.phase = Phase::Done;
        st.generation += 1;
        st.result = None;
    }
}

/// The coordination tick's call: start, continue or finish the autoload. Cheap when nothing is
/// due — a switch read and a clock compare.
pub(crate) fn tick(backend: &Backend, cx: &App) {
    let on = moon_core::market::trade_replay::tape_autoload();
    let mut st = lock();
    if !on {
        // Off re-arms: switched on again later, it starts over from a fresh read; a pass still
        // running under the old generation is not heard.
        if !matches!(st.phase, Phase::Armed) {
            st.generation += 1;
        }
        st.phase = Phase::Armed;
        st.result = None;
        return;
    }
    let now = Instant::now();
    match std::mem::take(&mut st.phase) {
        Phase::Armed => {
            st.phase = Phase::Waiting {
                due: now + FIRST_DELAY,
                left: None,
                attempts: 0,
            };
        }
        // The first pass also waits for the startup cleanup
        // (`settings::trades_cleanup_startup`): what it removes must not be what this pass
        // just fetched. A retry pass has the same guard for free — the cleanup is done by then.
        Phase::Waiting {
            due,
            left,
            attempts,
        } if due <= now && crate::settings::trades_cleanup_startup::clear_for_autoload() => {
            st.phase = Phase::Running;
            st.generation += 1;
            let generation = st.generation;
            start_pass(backend, left, attempts, generation, cx);
        }
        waiting @ Phase::Waiting { .. } => st.phase = waiting,
        Phase::Running => match st.result.take() {
            None => st.phase = Phase::Running,
            Some((left, attempts)) => {
                st.phase = match (left.is_empty(), attempts) {
                    (true, _) => Phase::Done,
                    (false, attempts) if attempts >= MAX_ATTEMPTS => {
                        log::info!(
                            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                            "[x] ticks autoload: {} row(s) never resolved after {attempts} passes — cores not connected or catalogs without the coin; given up",
                            left.len()
                        );
                        Phase::Done
                    }
                    (false, attempts) => Phase::Waiting {
                        due: now + RETRY,
                        left: Some(left),
                        attempts,
                    },
                };
            }
        },
        Phase::Done => st.phase = Phase::Done,
    }
}

/// One pass on the background executor: read the deals (first pass only), resolve, hand over,
/// report what is left — unless the pass was cancelled or outlived meanwhile.
fn start_pass(
    backend: &Backend,
    left: Option<Vec<Deal>>,
    attempts: u32,
    generation: u64,
    cx: &App,
) {
    let resolver = FetchResolver::of(backend);
    let defaults = strategy_field_defaults(backend);
    let axis = backend.report_axis(chrono_tz::UTC);
    let attempt = attempts + 1;
    cx.background_executor()
        .spawn(async move {
            let left = run_pass(resolver, defaults, axis, left, attempt, generation);
            let mut st = lock();
            if st.generation == generation {
                st.result = Some((left, attempt));
            }
        })
        .detach();
}

/// The pass itself, off the UI thread.
///
/// Returns:
///     The deals still unresolved — to try again — or nothing when every one was handed over,
///     skipped, or the read failed (a failed read is logged and not retried: the replica is
///     not going to change its mind in thirty seconds, and the axis' own load will say why).
fn run_pass(
    mut resolver: FetchResolver,
    defaults: std::collections::HashMap<String, f64>,
    axis: moon_core::db::ReportAxis,
    left: Option<Vec<Deal>>,
    attempt: u32,
    generation: u64,
) -> Vec<Deal> {
    let now_ms = moon_core::util::time::now_unix_ms_i64();
    let deals = match left {
        Some(left) => left,
        None => match read_recent(axis, now_ms) {
            Ok(deals) => deals,
            Err(error) => {
                log::info!(
                    target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                    "[x] ticks autoload: read failed, not retried: {error:?}"
                );
                return Vec::new();
            }
        },
    };
    let total = deals.len();
    let mut rows = Vec::new();
    let mut unresolved = Vec::new();
    let mut no_route = 0usize;
    let mut out_of_retention = 0usize;
    let mut degenerate = 0usize;
    for deal in deals {
        // Stamps that describe no window are the row's own fault, final: not a core that is
        // still coming, so never retried.
        if model_window(&deal, model_margin_ms(), long_position_ms()).is_none() {
            degenerate += 1;
            continue;
        }
        // Either `None` is the core not connected yet, or its catalog not spelling the coin
        // yet: the row waits for the next pass.
        let Some(address) = resolver.address(&deal) else {
            unresolved.push(deal);
            continue;
        };
        let Some(row) = resolver.queued_row(deal.clone(), address) else {
            unresolved.push(deal);
            continue;
        };
        let Some(route) = trade_route(row.replay_address.venue) else {
            no_route += 1;
            continue;
        };
        // The worker's own rule, asked here only to spare the candle page a refused row would
        // pay first: the exit inside the route's retention. Not the entry — a trade held across
        // the retention edge still gets its exit's tape, and what the model then lacks is the
        // model's own verdict, the same as through the button.
        if !inside_retention(route, row.window, now_ms) {
            out_of_retention += 1;
            continue;
        }
        rows.push(row);
    }
    // Newest-first is the job's queue order: it pops from the end, oldest first.
    rows.sort_by_key(|row| std::cmp::Reverse(row.deal.close_ms));
    let offered = rows.len();
    // Checked right before the hand-over, not at the start: the resolution above can take a
    // while, and a Stop pressed during it means these rows are not wanted. The autoload's lock
    // is HELD across the hand-over, so a Stop cannot slip between the check and the queue:
    // `cancel` waits for it, then bumps the generation, and the job it then stops already
    // holds these rows. The nesting is one-way (autoload, then job) — `ticks_fetch_stop`
    // takes them one after the other, never the job's inside the autoload's.
    let queued = {
        let st = lock();
        if st.generation != generation {
            drop(st);
            log::info!(
                target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                "[x] ticks autoload pass {attempt}: stopped before the hand-over, {offered} row(s) not queued"
            );
            return Vec::new();
        }
        job::enqueue(rows, defaults)
    };
    log::info!(
        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks autoload pass {attempt}: {total} deal(s) considered, {queued} queued ({} already in the batch), {no_route} with no route, {out_of_retention} past the venue's retention, {degenerate} with no window, {} unresolved (core not connected or catalog without the coin)",
        offered - queued,
        unresolved.len()
    );
    unresolved
}

/// The candidates: every closed trade with millisecond stamps of the last [`HORIZON_MS`], on
/// every core, under the axis' own filters.
fn read_recent(
    axis: moon_core::db::ReportAxis,
    now_ms: i64,
) -> Result<Vec<Deal>, moon_core::db::ReadFail> {
    let now_s = now_ms.div_euclid(1_000);
    let q = moon_core::db::analytics::Query {
        axis,
        from: now_s - HORIZON_MS.div_euclid(1_000),
        // Exclusive, and a day ahead: a core's clock a little ahead of this machine's must not
        // hide the trade that closed a minute ago.
        to: now_s + 86_400,
        // Percent needs no quote projection, so a fleet of mixed quotes reads in one pass.
        metric: moon_core::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let read = moon_core::db::tuner::ticks::read_deals(&q)?;
    log::info!(
        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks autoload read: {} deal(s) with ms stamps in the last {} days, {} service, {} not tunable",
        read.deals.len(),
        HORIZON_MS / 86_400_000,
        read.service,
        read.untunable
    );
    Ok(read.deals)
}
