//! The startup autoload: the tape of recent closed trades the close-time capture missed,
//! fetched on the terminal's own initiative once the cores are up.
//!
//! The close-time capture (`session::lifecycle::capture_closed_trade`) files a trade's prints
//! out of the core's ring the moment it closes — but only while the terminal is running. What
//! closed in between is gone from the ring by the next launch; the core's chart archive still
//! holds the last few thousand prints of each market (minutes on a hot coin, a day on a quiet
//! one), and the venue serves the rest where it has a route. The tuner's axis would otherwise
//! show those rows as missing until someone pressed "Fetch trades". Behind two switches —
//! `[trade_replay] autoload_cores` (on by default: it costs the cores' traffic only) and
//! `autoload_missing` (the venues; off by default: it spends their public budget unasked) —
//! this runs ONCE per process, from the coordination tick:
//!
//! 1. read every closed trade with millisecond stamps of the last [`HORIZON_MS`] across every
//!    core, under the axis' own filter — strategy trades the tuner can be run on;
//! 2. resolve each through the live source; drop the rows whose tape `trades.sqlite` already
//!    holds ([`drop_held`], answered off the span table's bounds, one read per market); with a
//!    station set up, ask it first for what the rest lack of their windows
//!    ([`super::station::fill`]) and drop what it covered — the station records the live stream
//!    around every trade while the terminal is off, and costs neither a core nor a venue;
//! 3. split what is left:
//!    - with the venue switch on, the ones the venue still serves by the worker's own retention
//!      rule (`inside_retention`: the exit inside the route's retention) go to the fetch job
//!      ([`super::job::enqueue`]), whose tape stage asks the core's archive first and the venue
//!      for the rest;
//!    - with the core switch on, every other row that closed within [`CORE_HORIZON_MS`] — no
//!      route (Bybit, Hyperliquid), past the venue's retention, or the venue switch off — is
//!      filed straight from the core's archive ([`file_from_cores`]): no venue request at all;
//! 4. a trade whose core is not connected yet, or whose catalog is not in, is kept and tried
//!    again every [`RETRY`] for up to [`MAX_ATTEMPTS`]: the cores come up one by one after the
//!    terminal, and the catalog a little after each core.
//!
//! The first pass yields to the startup cleanup of the trade tape
//! (`settings::trades_cleanup_startup`, behind `[trade_replay] cleanup_at_startup`): the
//! cleanup cuts the file to what the tuner's rows claim, this then fetches what they still
//! lack — the other order would fetch first and cut second.
//!
//! The read, the resolution and the station's round trips run on a thread of the pass's own; the
//! tick only decides whether one is due. "Stop" on the axis' button cancels the whole batch and the core filing
//! ([`cancel`]). The venue switch going off re-arms the pass and drops every row this autoload
//! queued for the venues ([`switched_off`]); rows the user queued with "Fetch trades" in the
//! same batch stay. The core switch going off stops the core filing between markets and
//! touches nothing else ([`cores_switched_off`]). Either switch going on re-arms the pass
//! ([`switched_on`]), which splits the rows anew by the switches as they then stand.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use std::collections::HashMap;

use super::job::{self, QueuedRow};
use super::{FetchResolver, strategy_field_defaults};
use crate::Backend;
use moon_core::db::tuner::ticks::{Deal, model_window, required_spans};
use moon_core::market::trade_replay::venue_caps::trade_route;
use moon_core::market::trade_replay::worker::{self as replay_worker, inside_retention};
use moon_core::market::trade_replay::{
    Coverage, ReplayWindow, long_position_ms, margin_ms, tape_autoload, tape_autoload_cores,
    trade_cache,
};

/// How far back the autoload looks, whatever the venue documents: the longest retention a
/// route names is 90 days, and a month of rows is already thousands of walks.
const HORIZON_MS: i64 = 30 * 24 * 3_600_000;

/// How far back a row is worth the core's archive: it holds a few thousand prints per market,
/// minutes deep on a hot coin and a day or somewhat more on the quietest measured (27.09,
/// `STATION.md` §7.1), so two days covers what any archive can still hold. A market is one
/// archive request of up to a few megabytes, so rows past this are not offered.
const CORE_HORIZON_MS: i64 = 48 * 3_600_000;

/// How long the first pass waits after the tick first finds the switch on — for the cores to
/// come up and report their catalogs, so the first pass resolves most rows at once.
const FIRST_DELAY: Duration = Duration::from_secs(20);

/// Between passes over the rows still unresolved.
const RETRY: Duration = Duration::from_secs(30);

/// Passes before the rows still unresolved are given up on: ten minutes of cores not coming.
const MAX_ATTEMPTS: u32 = 20;

/// How long a station that could not be reached is left out of the passes.
const STATION_BACKOFF: Duration = Duration::from_secs(600);

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
    /// Bumped by every start of a pass, by a cancel, by the venue switch going off and by either
    /// switch going on: a pass carries the generation it started under and is heard only while
    /// it is still the current one — a pass the user stopped, or that a flip outlived, neither
    /// enqueues nor reports.
    generation: u64,
    /// The core filing's own generation ([`file_from_cores`]): a pass reads it when it STARTS
    /// and the filing thread stops between markets once it moved. Bumped by a Stop, by the core
    /// switch going off, and by every re-arm — a re-armed pass files what the disk still lacks,
    /// so the filing it replaces stops rather than run beside it.
    cores_generation: u64,
    /// The station could not be reached: passes before this instant leave it out rather than
    /// wait for it again on every retry.
    station_back_at: Option<Instant>,
}

static AUTOLOAD: OnceLock<Mutex<Autoload>> = OnceLock::new();

fn lock() -> std::sync::MutexGuard<'static, Autoload> {
    AUTOLOAD
        .get_or_init(|| Mutex::new(Autoload::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The switch went off: re-arm so a later on starts from a fresh read, and drop every row this
/// autoload queued. A pass still running adds nothing when it comes back. Rows the user queued
/// in the same batch stay.
///
/// The autoload lock is taken and released before the job lock, the same order a pass uses
/// when it hands rows over, so the two cannot deadlock. A pass that already passed its
/// generation check and is inside the hand-over finishes that enqueue, then this drop removes
/// what it just added.
pub(crate) fn switched_off() {
    {
        let mut st = lock();
        if !matches!(st.phase, Phase::Armed) {
            st.generation += 1;
        }
        st.cores_generation += 1;
        st.phase = Phase::Armed;
        st.result = None;
    }
    job::stop_autoload();
}

/// The core switch went off: the core filing still running stops between markets. The venue
/// path and the pass are left alone — the venue switch may still be on.
pub(crate) fn cores_switched_off() {
    lock().cores_generation += 1;
}

/// A switch went ON: re-arm, so the next tick starts a pass that splits the rows by the switches
/// as they stand now. Nothing queued is dropped — the job skips a row it already holds, the
/// pass skips a row the disk holds — and a pass still running is not heard when it comes back.
pub(crate) fn switched_on() {
    let mut st = lock();
    if !matches!(st.phase, Phase::Armed) {
        st.generation += 1;
    }
    st.cores_generation += 1;
    st.phase = Phase::Armed;
    st.result = None;
}

/// Stop adding rows: what the job already has stays the job's to finish or to drop, a pass
/// still running adds nothing when it comes back, and the core filing stops between markets.
pub(crate) fn cancel() {
    let mut st = lock();
    st.cores_generation += 1;
    if !matches!(st.phase, Phase::Armed) {
        st.phase = Phase::Done;
        st.generation += 1;
        st.result = None;
    }
}

/// The coordination tick's call: start, continue or finish the autoload. Cheap when nothing is
/// due — a switch read and a clock compare.
pub(crate) fn tick(backend: &Backend) {
    let on = tape_autoload() || tape_autoload_cores();
    if !on {
        // Off re-arms and drops what this autoload queued. Already armed: nothing to do, and
        // the job is not locked on every tick while the switch stays off.
        let leave = { !matches!(lock().phase, Phase::Armed) };
        if leave {
            switched_off();
            cores_switched_off();
        }
        return;
    }
    let mut st = lock();
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
            start_pass(backend, left, attempts, generation);
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

/// One pass on a thread of its own: read the deals (first pass only), resolve, ask the station,
/// hand over, report what is left — unless the pass was cancelled or outlived meanwhile.
fn start_pass(backend: &Backend, left: Option<Vec<Deal>>, attempts: u32, generation: u64) {
    let resolver = FetchResolver::of(backend);
    let defaults = strategy_field_defaults(backend);
    let axis = backend.report_axis(chrono_tz::UTC);
    let attempt = attempts + 1;
    // Read when the pass starts: a server set up or forgotten meanwhile counts from the next one.
    let station = crate::backend::station::known_target();
    // A thread of its own, not the background executor: the station's round trips are blocking
    // SSH and may take minutes over a month of trades.
    let spawned = std::thread::Builder::new()
        .name("tape-autoload".into())
        .spawn(move || {
            let left = run_pass(resolver, defaults, axis, left, attempt, generation, station);
            let mut st = lock();
            if st.generation == generation {
                st.result = Some((left, attempt));
            }
        });
    if let Err(e) = spawned {
        log::warn!("[x] ticks autoload: pass thread did not start: {e}");
        let mut st = lock();
        if st.generation == generation {
            st.result = Some((Vec::new(), attempt));
        }
    }
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
    station: Option<moon_remote::ssh::Target>,
) -> Vec<Deal> {
    // Taken at the start, like the switches below: a core switch that goes off while this pass
    // resolves has moved it by the hand-over, and the filing then stops before its first market.
    let cores_generation = lock().cores_generation;
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
    // Read once per pass. A flip mid-pass is not re-read: a switch going on re-arms the next
    // pass, the venue switch going off drops what this one queued, and the core switch going
    // off moves the core generation this pass took at its start.
    let (venues_on, cores_on) = (tape_autoload(), tape_autoload_cores());
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    let mut no_route = 0usize;
    let mut out_of_retention = 0usize;
    let mut venue_off = 0usize;
    let mut too_old_for_core = 0usize;
    let mut degenerate = 0usize;
    for deal in deals {
        // Stamps that describe no window are the row's own fault, final: not a core that is
        // still coming, so never retried.
        if model_window(&deal, margin_ms(), long_position_ms()).is_none() {
            degenerate += 1;
            continue;
        }
        // Either `None` is the core not connected yet, or its catalog not spelling the coin
        // yet: the row waits for the next pass.
        let Some(address) = resolver.address(&deal) else {
            unresolved.push(deal);
            continue;
        };
        let Some(row) = resolver.queued_row(deal.clone(), address, job::RowOrigin::Autoload) else {
            unresolved.push(deal);
            continue;
        };
        resolved.push(row);
    }
    let place = |row: &QueuedRow| {
        (
            row.address.exchange_key.clone(),
            row.address.market.clone(),
            row.window,
        )
    };
    let held_spans = |exchange: &str, market: &str, from_ms, to_ms| {
        trade_cache::handle()?.held_spans(exchange, market, from_ms, to_ms)
    };
    let (mut resolved, mut held) = drop_held(resolved, place, held_spans);
    let mut from_station = 0usize;
    let station = station.filter(|_| {
        !resolved.is_empty() && lock().station_back_at.is_none_or(|at| Instant::now() >= at)
    });
    if let Some(target) = station {
        // A Stop, a flip that outlived this pass, or both switches going off end the station's
        // round trips between two requests.
        let stop =
            || lock().generation != generation || !(tape_autoload() || tape_autoload_cores());
        let unreachable = super::station::fill(&resolved, place, &target, stop).unreachable;
        lock().station_back_at = unreachable.then(|| Instant::now() + STATION_BACKOFF);
        let before = resolved.len();
        let (rest, _) = drop_held(resolved, place, held_spans);
        from_station = before - rest.len();
        held += from_station;
        resolved = rest;
    }
    let mut rows = Vec::new();
    let mut core_rows = Vec::new();
    for row in resolved {
        // The worker's own rule, asked here only to spare the candle page a refused row would
        // pay first: the exit inside the route's retention. Not the entry — a trade held across
        // the retention edge still gets its exit's tape, and what the model then lacks is the
        // model's own verdict, the same as through the button.
        let route = trade_route(row.replay_address.venue);
        let venue_serves = route.is_some_and(|route| inside_retention(route, row.window, now_ms));
        match row_path(
            (venues_on, cores_on),
            route.is_some(),
            venue_serves,
            row.window.close_ms,
            now_ms,
        ) {
            RowPath::Venue => rows.push(row),
            RowPath::Cores => core_rows.push(row),
            RowPath::NoRoute => no_route += 1,
            RowPath::PastRetention => out_of_retention += 1,
            RowPath::VenueOff => venue_off += 1,
            RowPath::TooOldForCores => too_old_for_core += 1,
        }
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
    let to_cores = core_rows.len();
    if !core_rows.is_empty() {
        file_from_cores(core_rows, cores_generation);
    }
    log::info!(
        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks autoload pass {attempt}: {total} deal(s) considered, {queued} queued for the venues ({} already in the batch), {to_cores} filed from the cores' archives, {held} held on disk ({from_station} of them just from the station), {no_route} with no route, {out_of_retention} past the venue's retention, {venue_off} with the venue switch off, {too_old_for_core} older than the cores' archives, {degenerate} with no window, {} unresolved (core not connected or catalog without the coin)",
        offered - queued,
        unresolved.len()
    );
    unresolved
}

/// Which path a resolved row takes, by the switches and what the venue still serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowPath {
    /// The fetch job: the core's archive first, the venue for the rest.
    Venue,
    /// Straight from the core's archive, no venue request.
    Cores,
    /// Neither: the venue has no route and the core switch is off.
    NoRoute,
    /// Neither: past the venue's retention and the core switch is off.
    PastRetention,
    /// Neither: the venue would serve it but its switch is off, and so is the core switch.
    VenueOff,
    /// Neither: the core switch is on, but the row closed before what an archive still holds.
    TooOldForCores,
}

/// The split of [`run_pass`]: the venue path where its switch is on and the venue serves the
/// row; otherwise the cores within [`CORE_HORIZON_MS`] of the exit; otherwise the reason neither
/// took it.
///
/// Args:
///     switches: `(venues_on, cores_on)`, read once per pass.
///     has_route: Whether the venue has a public trade route at all.
///     venue_serves: Whether that route still serves the row (the exit inside its retention).
///     close_ms: The row's exit, true-UTC milliseconds.
///     now_ms: The clock, true-UTC milliseconds.
fn row_path(
    (venues_on, cores_on): (bool, bool),
    has_route: bool,
    venue_serves: bool,
    close_ms: i64,
    now_ms: i64,
) -> RowPath {
    if venues_on && venue_serves {
        return RowPath::Venue;
    }
    if cores_on {
        return match close_ms >= now_ms - CORE_HORIZON_MS {
            true => RowPath::Cores,
            false => RowPath::TooOldForCores,
        };
    }
    match (has_route, venue_serves) {
        (false, _) => RowPath::NoRoute,
        (true, false) => RowPath::PastRetention,
        (true, true) => RowPath::VenueOff,
    }
}

/// File what the cores' chart archives hold of `rows` — no venue request.
///
/// Per market, one wait for the archive (`ReplayAddress::await_core_archive`: the first core of
/// the venue that answers, a few seconds at most), then the close-time capture of every row of
/// that market ([`replay_worker::capture`]), which copies the now-merged ring into the tiles and
/// `trades.sqlite` over what it actually holds. The capture runs on the replay worker's
/// coordinator, which answers the tuner's held-data queries and must not wait; the wait is here,
/// on a thread of its own, since a batch of markets is tens of seconds of them. A Stop or the
/// core switch going off (a new core generation) end it between markets.
fn file_from_cores(rows: Vec<QueuedRow>, cores_generation: u64) {
    let mut by_market: HashMap<(String, String), Vec<QueuedRow>> = HashMap::new();
    for row in rows {
        by_market
            .entry((row.address.exchange_key.clone(), row.address.market.clone()))
            .or_default()
            .push(row);
    }
    let spawned = std::thread::Builder::new()
        .name("tape-autoload-cores".into())
        .spawn(move || {
            let (markets, mut filed) = (by_market.len(), 0usize);
            for ((_, market), rows) in by_market {
                if lock().cores_generation != cores_generation || !tape_autoload_cores() {
                    log::info!(
                        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                        "[x] ticks autoload: core filing stopped after {filed} of {markets} market(s)"
                    );
                    return;
                }
                rows[0].replay_address.await_core_archive(&market);
                for row in rows {
                    replay_worker::capture(replay_worker::CaptureRequest {
                        address: row.replay_address,
                        market: market.clone(),
                        open_ms: row.window.open_ms,
                        close_ms: row.window.close_ms,
                        // The shape the row was built and judged held by, not the tab's
                        // setting now: the batch runs for tens of seconds.
                        margin_ms: row.window.margin_ms,
                        long_position_ms: row.window.long_position_ms,
                    });
                }
                filed += 1;
            }
            log::info!(
                target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                "[x] ticks autoload: filed {markets} market(s) from the cores' archives"
            );
        });
    if let Err(e) = spawned {
        log::warn!("[x] ticks autoload: core filing thread did not start: {e}");
    }
}

/// Drop the rows whose tape the disk already holds — every stretch the model needs of the
/// window (`required_spans`, the rule the axis marks a row covered by) inside the spans
/// `trades.sqlite` has filed for the market. One bounds read per market, over the stretch its
/// rows span; a market whose read did not happen keeps every row — the job then asks, as it
/// always did.
///
/// Args:
///     rows: The candidates.
///     place: A row's `(exchange key, market, window)`.
///     held_spans: `(exchange, market, from_ms, to_ms)` → the stored spans' bounds, `None` when
///         the read did not happen.
///
/// Returns:
///     The rows still worth the job, and how many were dropped as held.
fn drop_held<T>(
    rows: Vec<T>,
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    held_spans: impl Fn(&str, &str, i64, i64) -> Option<Vec<(i64, i64)>>,
) -> (Vec<T>, usize) {
    let mut by_market: HashMap<(String, String), Vec<(T, Coverage)>> = HashMap::new();
    for row in rows {
        let (exchange, market, window) = place(&row);
        by_market
            .entry((exchange, market))
            .or_default()
            .push((row, required_spans(&window)));
    }
    let mut kept = Vec::new();
    let mut held = 0usize;
    for ((exchange, market), group) in by_market {
        let bounds = group
            .iter()
            .filter_map(|(_, need)| need.hull())
            .reduce(|(a_from, a_to), (b_from, b_to)| (a_from.min(b_from), a_to.max(b_to)));
        let stored = bounds
            .and_then(|(from_ms, to_ms)| held_spans(&exchange, &market, from_ms, to_ms))
            .map(Coverage::from_spans);
        for (row, need) in group {
            match &stored {
                Some(stored) if !need.is_empty() && stored.covers(&need) => held += 1,
                _ => kept.push(row),
            }
        }
    }
    (kept, held)
}

#[cfg(test)]
mod tests;

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
