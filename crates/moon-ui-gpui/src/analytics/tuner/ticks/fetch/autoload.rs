//! Startup and window-growth tape filling, independent of an open tuner.
//!
//! Startup reads thirty days; growth invalidates the running pass and reads the full history
//! once. A configured station is the sole source, including while it is unreachable. Without
//! one, the existing core and venue switches choose the fallback legs. Successful incomplete
//! station answers are remembered until their window or source policy changes; transport
//! failures remain retryable. All report reads, resolution and transport run off the UI thread.
//!
//! Core autoload defaults on and venue autoload defaults off; neither explicit choice is
//! overridden by growth. The first pass waits for startup cleanup. Unresolved core addresses
//! retry within a bounded budget, and station outages use the shared reachability backoff.
//! Cancellation and switch changes invalidate old work before downstream handover.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use std::collections::HashMap;

use super::job::{self, QueuedRow};
use super::{FetchResolver, strategy_field_defaults};
use crate::Backend;
use moon_core::db::tuner::ticks::{Deal, model_window};
use moon_core::market::CORE_ARCHIVE_RETENTION_MS;
use moon_core::market::trade_replay::venue_caps::{Fetchable, fetchable};
use moon_core::market::trade_replay::worker::{self as replay_worker};
use moon_core::market::trade_replay::{
    long_position_ms, margin_ms, tape_autoload, tape_autoload_cores,
};

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

/// Scheduler, cancellation epochs and progress owned by the process rather than a tuner view.
#[derive(Default)]
struct Autoload {
    phase: Phase,
    /// A newly discovered station invalidates already admitted fallback work.
    station_known: bool,
    /// Growth reads all history once; ordinary startup retains its bounded horizon.
    whole_history: bool,
    /// Rows the current pass has not yet handed over or classified.
    pending: usize,
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
/// Core filing outlives the resolving pass but still owns pending deals.
static CORE_PENDING: AtomicUsize = AtomicUsize::new(0);

/// Release unfiled progress on cancellation, thread-spawn failure and normal completion.
struct CoreWork {
    left: usize,
}

impl CoreWork {
    /// Register all rows before launching their detached filing thread.
    fn new(left: usize) -> Self {
        CORE_PENDING.fetch_add(left, Ordering::Relaxed);
        Self { left }
    }

    /// Remove a row after its capture was handed to the replay worker.
    fn filed_one(&mut self) {
        self.left -= 1;
        CORE_PENDING.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Drop for CoreWork {
    /// A stopped thread must not leave an eternal loading caption.
    fn drop(&mut self) {
        CORE_PENDING.fetch_sub(self.left, Ordering::Relaxed);
    }
}

/// Source policy and cancellation snapshot shared by a background pass and its transport.
struct Pass {
    generation: u64,
    station: Option<moon_remote::ssh::Target>,
    station_only: bool,
    whole_history: bool,
}

/// Background I/O boundary; scheduling and source admission stay in the production pass.
trait Sources {
    /// Read deals under the exact report query chosen by the production pass.
    fn read(
        &mut self,
        query: &moon_core::db::analytics::Query,
    ) -> Result<Vec<Deal>, moon_core::db::ReadFail>;
    /// File station tape and return rows still missing coverage.
    fn prefill(
        &mut self,
        rows: Vec<QueuedRow>,
        target: Option<&moon_remote::ssh::Target>,
        stop: &dyn Fn() -> bool,
    ) -> (Vec<QueuedRow>, usize, usize);
    /// Hand admitted rows to the venue worker.
    fn enqueue(&mut self, rows: Vec<QueuedRow>, defaults: HashMap<String, f64>) -> usize;
    /// File admitted rows from core archives.
    fn cores(&mut self, rows: Vec<QueuedRow>, generation: u64);
}

/// The terminal's existing station, venue and core legs.
struct LiveSources {
    generation: u64,
}

impl Sources for LiveSources {
    /// Read the replica using the normal tuner filters.
    fn read(
        &mut self,
        query: &moon_core::db::analytics::Query,
    ) -> Result<Vec<Deal>, moon_core::db::ReadFail> {
        moon_core::db::tuner::ticks::read_deals(query).map(|read| read.deals)
    }
    /// Keep the station's disk-before/pull/disk-after coverage check.
    fn prefill(
        &mut self,
        rows: Vec<QueuedRow>,
        target: Option<&moon_remote::ssh::Target>,
        stop: &dyn Fn() -> bool,
    ) -> (Vec<QueuedRow>, usize, usize) {
        let offered = rows.len();
        super::station::prefill_counted(rows, target, stop, |missing| {
            held_rows_dropped(self.generation, offered, missing);
        })
    }
    /// Use the process-wide venue queue.
    fn enqueue(&mut self, rows: Vec<QueuedRow>, defaults: HashMap<String, f64>) -> usize {
        job::enqueue(rows, defaults)
    }
    /// Use the existing core-archive filing worker.
    fn cores(&mut self, rows: Vec<QueuedRow>, generation: u64) {
        file_from_cores(rows, generation);
    }
}

std::thread_local! {
    /// This thread holds the autoload lock: the coordination tick takes it on the UI thread, so a
    /// second take there would freeze the whole terminal instead of failing.
    static HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The autoload lock, released on drop; taking it twice on one thread panics.
struct Held(std::sync::MutexGuard<'static, Autoload>);

impl std::ops::Deref for Held {
    type Target = Autoload;
    fn deref(&self) -> &Autoload {
        &self.0
    }
}

impl std::ops::DerefMut for Held {
    fn deref_mut(&mut self) -> &mut Autoload {
        &mut self.0
    }
}

impl Drop for Held {
    /// Clear the per-thread mark, on a normal release and on an unwind alike.
    fn drop(&mut self) {
        HELD.with(|held| held.set(false));
    }
}

fn lock() -> Held {
    assert!(
        !HELD.with(std::cell::Cell::get),
        "autoload lock taken twice on one thread: std::sync::Mutex is not re-entrant and would hang"
    );
    let guard = AUTOLOAD
        .get_or_init(|| Mutex::new(Autoload::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    HELD.with(|held| held.set(true));
    Held(guard)
}

/// Current cancellation generation shared by station-prefill and autoload passes.
pub(super) fn generation() -> u64 {
    lock().generation
}

/// Station target only while the common reachability backoff permits another attempt.
pub(super) fn station_target() -> Option<moon_remote::ssh::Target> {
    station_target_of(&lock())
}

/// [`station_target`] for a caller already holding the lock — the tick, on the UI thread.
fn station_target_of(st: &Autoload) -> Option<moon_remote::ssh::Target> {
    if st.station_back_at.is_some_and(|at| Instant::now() < at) {
        return None;
    }
    crate::backend::station::known_target()
}

/// Share station reachability between background and table-triggered passes.
pub(super) fn station_answered(unreachable: bool) {
    lock().station_back_at = unreachable.then(|| Instant::now() + STATION_BACKOFF);
}

/// Serialize the hand-over against Stop, so an obsolete prefill cannot restart a batch.
pub(super) fn enqueue_if_current(
    generation: u64,
    rows: Vec<QueuedRow>,
    defaults: HashMap<String, f64>,
) -> usize {
    let st = lock();
    if st.generation != generation || !tape_autoload() {
        return 0;
    }
    job::enqueue(rows, defaults)
}

/// The switch went off: re-arm so a later on starts from a fresh read, and drop every row this
/// autoload queued. A pass still running adds nothing when it comes back.
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

/// The coordination tick's call: start, continue or finish the autoload. Cheap when nothing is
/// due — a switch read and a clock compare.
pub(crate) fn tick(backend: &Backend) {
    tick_with(
        crate::settings::trades_cleanup_startup::clear_for_autoload,
        crate::backend::station::known_cached,
        crate::backend::station::known_target,
        |left, attempts, generation, station, station_only, whole_history| {
            start_pass(
                backend,
                left,
                attempts,
                generation,
                station,
                station_only,
                whole_history,
            )
        },
    );
}

/// Drive the production scheduler with background launch and cleanup readiness supplied.
fn tick_with(
    ready: impl Fn() -> bool,
    known: impl Fn() -> bool,
    target: impl Fn() -> Option<moon_remote::ssh::Target>,
    start: impl FnOnce(
        Option<Vec<Deal>>,
        u32,
        u64,
        Option<moon_remote::ssh::Target>,
        bool,
        bool,
    ) -> bool,
) {
    let station_known = known();
    let arrived = {
        let mut st = lock();
        let arrived = !st.station_known
            && station_known
            && (!matches!(st.phase, Phase::Armed | Phase::Waiting { left: None, .. })
                || job::progress().active
                || CORE_PENDING.load(Ordering::Relaxed) > 0);
        st.station_known = station_known;
        arrived
    };
    if arrived {
        switched_off();
    }
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
        } if due <= now && ready() => {
            st.phase = Phase::Running;
            st.generation += 1;
            let generation = st.generation;
            let configured = target();
            st.station_known = configured.is_some();
            let station_only = configured.is_some();
            let station = configured.filter(|_| st.station_back_at.is_none_or(|at| now >= at));
            let whole_history = st.whole_history;
            if !start(
                left,
                attempts,
                generation,
                station,
                station_only,
                whole_history,
            ) {
                st.phase = Phase::Armed;
            }
        }
        waiting @ Phase::Waiting { .. } => st.phase = waiting,
        Phase::Running => match st.result.take() {
            None => st.phase = Phase::Running,
            Some((left, attempts)) => {
                // Only a current completed pass consumes a full-history growth request.
                st.whole_history = false;
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
///
/// Called by the tick under the autoload lock, so `station` is read by the caller: this body must
/// not take that lock again before its thread starts.
/// Returns false on spawn failure so the caller can rearm under its existing lock.
fn start_pass(
    backend: &Backend,
    left: Option<Vec<Deal>>,
    attempts: u32,
    generation: u64,
    station: Option<moon_remote::ssh::Target>,
    station_only: bool,
    whole_history: bool,
) -> bool {
    let resolver = FetchResolver::of(backend);
    let defaults = strategy_field_defaults(backend);
    let axis = backend.report_axis(chrono_tz::UTC);
    let attempt = attempts + 1;
    // A thread of its own, not the background executor: the station's round trips are blocking
    // SSH and may take minutes over a month of trades.
    let spawned = std::thread::Builder::new()
        .name("tape-autoload".into())
        .spawn(move || {
            let pass = Pass {
                generation,
                station,
                station_only,
                whole_history,
            };
            let left = run_pass_with(
                resolver,
                defaults,
                axis,
                left,
                attempt,
                &pass,
                &mut LiveSources { generation },
            );
            let mut st = lock();
            if st.generation == generation {
                // Offline transport is retryable, not an unresolved-core failure budget.
                let attempt = if station_only && st.station_back_at.is_some() {
                    0
                } else {
                    attempt
                };
                st.pending = left.len();
                st.result = Some((left, attempt));
            }
        });
    if let Err(e) = spawned {
        log::warn!("[x] ticks autoload: pass thread did not start: {e}");
        return false;
    }
    true
}

/// The pass itself, off the UI thread.
///
/// Returns:
///     The deals still unresolved — to try again — or nothing when every one was handed over,
///     skipped, or the read failed (a failed read is logged and not retried: the replica is
///     not going to change its mind in thirty seconds, and the axis' own load will say why).
fn run_pass_with(
    mut resolver: FetchResolver,
    defaults: std::collections::HashMap<String, f64>,
    axis: moon_core::db::ReportAxis,
    left: Option<Vec<Deal>>,
    attempt: u32,
    pass: &Pass,
    sources: &mut impl Sources,
) -> Vec<Deal> {
    let Pass {
        generation,
        station,
        station_only,
        whole_history,
    } = pass;
    let (generation, station_only, whole_history) = (*generation, *station_only, *whole_history);
    // Taken at the start, like the switches below: a core switch that goes off while this pass
    // resolves has moved it by the hand-over, and the filing then stops before its first market.
    let cores_generation = lock().cores_generation;
    let now_ms = moon_core::util::time::now_unix_ms_i64();
    let deals = match left {
        Some(left) => left,
        None => match read_recent(axis, now_ms, whole_history, sources) {
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
    {
        let mut st = lock();
        if st.generation != generation {
            return Vec::new();
        }
        st.pending = total;
    }
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
        let Some(row) = resolver.queued_row(deal.clone(), address) else {
            unresolved.push(deal);
            continue;
        };
        resolved.push(row);
    }
    let stop = || lock().generation != generation || !(tape_autoload() || tape_autoload_cores());
    let (resolved, held, from_station) = sources.prefill(resolved, station.as_ref(), &stop);
    if station_only {
        // A missing or offline station never authorizes spending venue/core traffic.
        unresolved.extend(resolved.into_iter().map(|row| row.deal));
        return unresolved;
    }
    let mut rows = Vec::new();
    let mut core_rows = Vec::new();
    for row in resolved {
        // Use the shared REST/archive reach before paying for the row's candle stage.
        let Some((_, to_ms)) = row.window.focus_spans().hull() else {
            degenerate += 1;
            continue;
        };
        let source = fetchable(row.address.venue, &row.address.market, to_ms, now_ms);
        let venue_serves = matches!(source, Fetchable::Rest | Fetchable::Archive);
        match row_path(
            (venues_on, cores_on),
            source != Fetchable::NoRoute,
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
    // while, and a switch-off during it means these rows are not wanted. The autoload lock
    // spans the hand-over, so switch-off cannot slip between the generation check and enqueue.
    // Lock nesting remains one-way: autoload, then job.
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
        sources.enqueue(rows, defaults)
    };
    let to_cores = core_rows.len();
    if !core_rows.is_empty() {
        sources.cores(core_rows, cores_generation);
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
/// row; otherwise the cores within [`CORE_ARCHIVE_RETENTION_MS`] of the exit; otherwise the reason neither
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
        return match close_ms >= now_ms - CORE_ARCHIVE_RETENTION_MS {
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
    let mut progress = CoreWork::new(rows.len());
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
                if lock().cores_generation != cores_generation
                    || !tape_autoload_cores()
                    || crate::backend::station::known_cached()
                {
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
                    progress.filed_one();
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

#[cfg(test)]
mod tests;

/// Closed candidates across every core, using the captured startup or full-history scope.
/// The axis filters remain the tuner's normal strategy-trade filters.
fn read_recent(
    axis: moon_core::db::ReportAxis,
    now_ms: i64,
    whole_history: bool,
    sources: &mut impl Sources,
) -> Result<Vec<Deal>, moon_core::db::ReadFail> {
    let now_s = now_ms.div_euclid(1_000);
    let q = moon_core::db::analytics::Query {
        axis,
        from: history_from(now_ms, whole_history),
        // Exclusive, and a day ahead: a core's clock a little ahead of this machine's must not
        // hide the trade that closed a minute ago.
        to: now_s + 86_400,
        // Percent needs no quote projection, so a fleet of mixed quotes reads in one pass.
        metric: moon_core::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let deals = sources.read(&q)?;
    log::info!(
        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks autoload read: {} deal(s) with ms stamps from {} s",
        deals.len(),
        q.from,
    );
    Ok(deals)
}

/// Re-arms the background autoload when the shared window grew.
pub(crate) fn window_grown(
    old: moon_core::station_api::TapeWindow,
    new: moon_core::station_api::TapeWindow,
) {
    if new.margin_s <= old.margin_s && new.long_position_min <= old.long_position_min {
        return;
    }
    {
        let mut st = lock();
        st.generation = st.generation.wrapping_add(1);
        st.cores_generation = st.cores_generation.wrapping_add(1);
        st.phase = Phase::Armed;
        st.whole_history = true;
        st.pending = 0;
        st.result = None;
    }
    job::stop_autoload();
}

/// Start growth reads at the beginning, retaining thirty days for ordinary startup.
fn history_from(now_ms: i64, whole_history: bool) -> i64 {
    if whole_history {
        0
    } else {
        now_ms.div_euclid(1_000) - HORIZON_MS / 1_000
    }
}

/// Deals still being filled by this pass or its worker; idle work has no loading caption.
pub(crate) fn pending_rows() -> Option<usize> {
    let st = lock();
    let pending = match &st.phase {
        Phase::Running => Some(st.pending),
        Phase::Waiting { left, .. } => Some(left.as_ref().map_or(0, Vec::len)),
        Phase::Armed | Phase::Done => None,
    };
    let job = job::progress();
    let queued = job.total.saturating_sub(job.done);
    let cores = CORE_PENDING.load(Ordering::Relaxed);
    if job.active || cores > 0 {
        Some(pending.unwrap_or(0) + queued + cores)
    } else {
        pending
    }
}

/// Remove already-held rows before station I/O, preserving unresolved rows in the same pass.
fn held_rows_dropped(generation: u64, offered: usize, missing: usize) {
    let mut st = lock();
    if st.generation == generation {
        st.pending = st.pending.saturating_sub(offered.saturating_sub(missing));
    }
}
