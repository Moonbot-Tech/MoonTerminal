//! Configured-station tape and shared coverage checks for background and tuner callers.
//! A configured station is the terminal's sole remote tape source; answered misses are retained
//! separately from transport failures so automatic retries remain both bounded and honest.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use moon_core::db::tuner::ticks::tail::{floor_ms, focus_with_tail, record_tail_ms};
use moon_core::market::trade_replay::margin_ms;
use moon_core::market::trade_replay::{Coverage, ReplayWindow, trade_cache};
use moon_core::station_api::TapeWant;
use moon_remote::ssh::Target;

/// How long the pass waits for the station's prints to reach the file before it judges which
/// rows they covered.
const FILE_SYNC: Duration = Duration::from_secs(10);

/// Disk revision lets views notice a fill even when no venue batch was needed.
static FILED: AtomicU64 = AtomicU64::new(0);
/// In-progress station work remains stoppable before a venue batch exists.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Bumped whenever a deal read lifts its stamps on different offsets than the last one
/// ([`note_axis`]).
static AXIS_EPOCH: AtomicU64 = AtomicU64::new(0);

/// What a refusal was judged under: the station's finalised-tail count and its count of trades
/// with complete tape, the tail floor, the margin and long-position window, and the report axis
/// epoch.
///
/// The tape count is what moves when the station files a refused trade's tape afterwards — its
/// hourly catch-up, or the live tail of a trade asked about too early: such a trade goes to the
/// complete set, never to the finalised tails, so the finalised count alone kept the row
/// refused until the terminal was quit. Every refusal is dropped on a new count; the next pass
/// asks only for the stretches the file still lacks ([`lacking`]).
type Generation = ((u64, u64), u32, (i64, i64), u64);

/// A report uid is unique only inside its core's report database.
type RefusalKey = (u64, i64);

/// Station refusals survive unrelated filings, but not changes to their admission inputs.
#[derive(Default)]
struct Unfillable {
    generation: Generation,
    revision: u64,
    uids: HashMap<RefusalKey, (String, String)>,
    /// Each market's revision of its last membership change, for [`refusals_since`].
    changed: HashMap<(String, String), u64>,
    snapshot_revision: u64,
    snapshot: Arc<HashSet<RefusalKey>>,
    /// Rows answered short while their post-close tail could still arrive, each with the
    /// moment it is final. Not refusals — the table goes on showing them as waiting — but not
    /// asked about again before that moment either: every pass asking meanwhile brought a
    /// little more tape, and each filing reloaded the open table and stopped its search.
    deferred: HashMap<RefusalKey, i64>,
}

impl Unfillable {
    /// Rebuild membership only after a mutation; every unchanged pass shares the same set.
    fn snapshot(&mut self) -> (u64, Arc<HashSet<RefusalKey>>) {
        if self.snapshot_revision != self.revision {
            self.snapshot = Arc::new(self.uids.keys().copied().collect());
            self.snapshot_revision = self.revision;
        }
        (self.revision, Arc::clone(&self.snapshot))
    }

    /// A changed window, floor, station finalisation or tape count, or report axis permits
    /// another attempt.
    fn refresh(&mut self, generation: Generation) {
        if self.generation != generation {
            // The waits follow what their moments were computed from — the floor, the window
            // and the axis, whose move shifts a row's close by its core's offset, hours at a
            // time. The station's counts are not among them: a status read does not change when
            // a tail is final, and dropping the waits there would ask about every such row
            // again.
            let (_, floor, window, axis) = self.generation;
            if (floor, window, axis) != (generation.1, generation.2, generation.3) {
                self.deferred.clear();
            }
            self.generation = generation;
            if !self.uids.is_empty() {
                self.revision += 1;
                let revision = self.revision;
                for (_, market) in self.uids.drain() {
                    self.changed.insert(market, revision);
                }
            }
        }
    }

    /// Remember a refusal and invalidate classifications only when membership changes.
    fn refuse(&mut self, core_uid: u64, uid: i64, exchange: &str, market: &str) {
        if self
            .uids
            .insert((core_uid, uid), (exchange.into(), market.into()))
            .is_none()
        {
            self.mark(exchange, market);
        }
    }

    /// Only a filing on this market can make its refused rows worth checking again.
    fn filed(&mut self, exchange: &str, market: &str) {
        let before = self.uids.len();
        self.uids
            .retain(|_, key| key.0 != exchange || key.1 != market);
        if self.uids.len() != before {
            self.mark(exchange, market);
        }
    }

    /// Drop refusals whose rows left the published sample, and market history nothing references.
    fn retain(&mut self, live: &HashSet<RefusalKey>, markets: &HashSet<(String, String)>) {
        let removed: Vec<_> = self
            .uids
            .iter()
            .filter(|(key, _)| !live.contains(*key))
            .map(|(key, market)| (*key, market.clone()))
            .collect();
        if !removed.is_empty() {
            self.revision += 1;
            let revision = self.revision;
            for (key, market) in removed {
                self.uids.remove(&key);
                self.changed.insert(market, revision);
            }
        }
        self.changed.retain(|market, _| markets.contains(market));
        self.deferred.retain(|key, _| live.contains(key));
    }

    /// Whether a row is waiting out its post-close tail at `now_ms`; a wait that has run out
    /// is forgotten here, so the row is asked about once more.
    fn waiting_tail(&mut self, core_uid: u64, uid: i64, now_ms: i64) -> bool {
        let key = (core_uid, uid);
        match self.deferred.get(&key) {
            Some(&until) if now_ms < until => true,
            Some(_) => {
                self.deferred.remove(&key);
                false
            }
            None => false,
        }
    }

    /// Bump the revision for a membership change on one market.
    fn mark(&mut self, exchange: &str, market: &str) {
        self.revision += 1;
        self.changed
            .insert((exchange.into(), market.into()), self.revision);
    }

    /// The revision now and every market whose membership changed after `since`.
    fn since(&self, since: u64) -> (u64, HashSet<(String, String)>) {
        let markets = self
            .changed
            .iter()
            .filter(|&(_, &rev)| rev > since)
            .map(|(market, _)| market.clone())
            .collect();
        (self.revision, markets)
    }
}

/// Share station refusals across windows and startup autoload passes.
fn unfillable() -> MutexGuard<'static, Unfillable> {
    static ROWS: OnceLock<Mutex<Unfillable>> = OnceLock::new();
    let mut rows = ROWS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rows.refresh((
        (
            crate::backend::station::tail_final_cached(),
            crate::backend::station::tape_trades_cached(),
        ),
        super::super::tail::effective_s(),
        (
            margin_ms(),
            moon_core::market::trade_replay::long_position_ms(),
        ),
        AXIS_EPOCH.load(Ordering::Acquire),
    ));
    rows
}

/// Record the axis a deal read just lifted its stamps on.
///
/// A deal's tape window is its stamps lifted onto true UTC by its core's measured offset, so a
/// refusal judged before an offset was measured, or before it changed, asked the station for a
/// different stretch than the row now needs: a read on other offsets drops every remembered
/// refusal. The read's own axis is the authority, not the session's — the session adopts an
/// offset before the replica that the read resolves it from has stored it. Only the offsets
/// count: the Analytics window and the startup pass read in different display zones.
///
/// Args:
///     axis: `DealsRead::axis` of the read whose deals become rows.
pub(in crate::analytics::tuner) fn note_axis(axis: &moon_core::db::ReportAxis) {
    static LAST: Mutex<Option<moon_core::db::ReportAxis>> = Mutex::new(None);
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !last.as_ref().is_some_and(|last| last.same_offsets(axis)) {
        *last = Some(axis.clone());
        AXIS_EPOCH.fetch_add(1, Ordering::AcqRel);
    }
}

/// Whether any station miss is remembered: only then is the station's status worth reading
/// again (`Backend::station_tape_tick`).
pub(super) fn refusals_held() -> bool {
    !unfillable().uids.is_empty()
}

/// The earliest moment a row left unrefused because its tail could still arrive
/// ([`tail_may_follow`]) is worth asking about again; `i64::MAX` when there is none.
static RETRY_AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MAX);
/// The refusal revision the last wake was given for.
static WOKEN_REVISION: AtomicU64 = AtomicU64::new(0);

/// Whether an open Entry/Exit table should ask the station again now, true once per cause.
///
/// The table hears the station only while a fetch batch of its own runs, and asks for missing
/// rows only when it reloads. Two changes reach it no other way: a refusal dropped because the
/// station's status moved (the refusal revision), and a row left unrefused whose post-close
/// tail has now had time to be final ([`RETRY_AT`]).
///
/// Args:
///     now_ms: The clock, true UTC milliseconds.
pub(super) fn wake_due(now_ms: i64) -> bool {
    let retry_at = RETRY_AT.swap(i64::MAX, Ordering::AcqRel);
    let settled = retry_at <= now_ms;
    if !settled {
        // Not yet: put it back, unless a pass armed an earlier one meanwhile.
        RETRY_AT.fetch_min(retry_at, Ordering::AcqRel);
    }
    let revision = unfillable().revision;
    let moved = WOKEN_REVISION.swap(revision, Ordering::AcqRel) != revision;
    settled || moved
}

/// Whether the station may still be writing a row's tape: before the recorder treats the
/// trade's post-close tail as final (`tail_settles_at_ms`), a short answer says "asked too
/// early", not "the station has none". Such a row is not remembered as refused; it is left
/// alone until that moment (`Unfillable::deferred`) and the open table is then woken to ask
/// once more ([`wake_due`]) — one more request, instead of a row that reads "not on the
/// station" until the terminal is quit, or one asked about on every pass in between.
///
/// The moment is computed from this terminal's margin and tail floor, which it keeps the
/// station's in step with; a station still on longer ones answers short once more and the row
/// is then refused, until its catch-up lifts that with the rest.
///
/// Args:
///     window: The row's replay window.
///     min_tail_s: The shortest tail in force, seconds.
///     now_ms: The clock, true UTC milliseconds.
fn tail_may_follow(window: &ReplayWindow, min_tail_s: u32, now_ms: i64) -> bool {
    now_ms < tail_settles_at(window, min_tail_s)
}

/// When the recorder treats a row's post-close tail as final, true UTC milliseconds.
fn tail_settles_at(window: &ReplayWindow, min_tail_s: u32) -> i64 {
    moon_core::market::tape_recorder::tail_settles_at_ms(
        window.close_ms,
        window.margin_ms,
        record_tail_ms(min_tail_s, window.margin_ms),
    )
}

/// Whether a station-only row still deserves an automatic request in this generation.
pub(in crate::analytics::tuner) fn candidate(core_uid: u64, uid: i64) -> bool {
    !unfillable().uids.contains_key(&(core_uid, uid))
}

/// Bind refusal lifetime to the rows in the newly published sample, never to a numeric cap.
pub(in crate::analytics::tuner) fn retain_rows<'a>(
    rows: impl IntoIterator<Item = (u64, i64, Option<(&'a str, &'a str)>)>,
) {
    let mut live = HashSet::new();
    let mut markets = HashSet::new();
    for (core, uid, address) in rows {
        live.insert((core, uid));
        if let Some((exchange, market)) = address {
            markets.insert((exchange.to_string(), market.to_string()));
        }
    }
    unfillable().retain(&live, &markets);
}

/// One pass's refusal membership and revision, without a global lock for every row.
pub(in crate::analytics::tuner) fn refusal_snapshot() -> (u64, Arc<HashSet<RefusalKey>>) {
    unfillable().snapshot()
}

/// Refresh admission caches even when no tape row changed.
pub(in crate::analytics::tuner) fn refusal_revision() -> u64 {
    unfillable().revision
}

/// The refusal revision now and the `(exchange, market)` pairs whose refusal membership changed
/// after `since` — like `trade_cache::filed_since`, so a reader recounts only those markets.
pub(in crate::analytics::tuner) fn refusals_since(since: u64) -> (u64, HashSet<(String, String)>) {
    unfillable().since(since)
}

/// Only actual station filing invalidates held-row queries.
fn note_filed(filled: Filled) {
    if filled.pieces > 0 {
        FILED.fetch_add(1, Ordering::Release);
        super::job::station_filled();
    }
}

/// Keep station prefill visible until its background hand-over finishes.
struct Busy;
impl Busy {
    /// Mark one background pass active.
    fn new() -> Self {
        ACTIVE.fetch_add(1, Ordering::Relaxed);
        Self
    }
}
impl Drop for Busy {
    /// Release activity on normal completion, cancellation, or a failed spawn.
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Whether station work still precedes a venue batch.
pub(super) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed) > 0
}

/// Last station filing visible to a view, including a partial fill.
pub(super) fn filed_generation() -> u64 {
    FILED.load(Ordering::Acquire)
}

/// Discard complete rows and unfillable short tails whose pre-close tape is complete.
pub(super) fn drop_held<T>(
    rows: Vec<T>,
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    held_spans: impl Fn(&str, &str, i64, i64) -> Option<Vec<(i64, i64)>>,
    allow_short: impl Fn(&T, &Coverage) -> bool,
) -> (Vec<T>, usize) {
    drop_held_at(
        super::super::tail::effective_s(),
        rows,
        place,
        held_spans,
        allow_short,
    )
}

/// Apply the held-row rule at one captured floor, clamping it like sample admission.
fn drop_held_at<T>(
    min_s: u32,
    rows: Vec<T>,
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    held_spans: impl Fn(&str, &str, i64, i64) -> Option<Vec<(i64, i64)>>,
    allow_short: impl Fn(&T, &Coverage) -> bool,
) -> (Vec<T>, usize) {
    let min_s = moon_core::db::tuner::ticks::tail::clamp_min_tail_s(min_s);
    let mut by_market: HashMap<(String, String), Vec<(T, Coverage, ReplayWindow)>> = HashMap::new();
    for row in rows {
        let (exchange, market, window) = place(&row);
        let need = focus_with_tail(&window, record_tail_ms(min_s, window.margin_ms));
        by_market
            .entry((exchange, market))
            .or_default()
            .push((row, need, window));
    }
    let mut kept = Vec::new();
    let mut held = 0;
    for ((exchange, market), group) in by_market {
        let bounds = group
            .iter()
            .filter_map(|(_, need, _)| need.hull())
            .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)));
        let stored = bounds
            .and_then(|(from, to)| held_spans(&exchange, &market, from, to))
            .map(Coverage::from_spans);
        for (row, need, window) in group {
            if stored.as_ref().is_some_and(|stored| {
                let focus = focus_with_tail(&window, record_tail_ms(min_s, window.margin_ms));
                !need.is_empty()
                    && (stored.covers(&need)
                        || (allow_short(&row, &stored.clip(&focus))
                            && stored.covers(
                                &need.clip(&Coverage::one((window.from_ms, window.close_ms))),
                            )))
            }) {
                held += 1;
            } else {
                kept.push(row);
            }
        }
    }
    (kept, held)
}

/// Shared station-first pass, called only from background threads; check disk on both sides.
pub(super) fn prefill(
    rows: Vec<super::job::QueuedRow>,
    target: Option<&Target>,
    stop: impl Fn() -> bool,
) -> (Vec<super::job::QueuedRow>, usize, usize) {
    prefill_counted(rows, target, stop, |_| {})
}

/// Report missing rows before blocking transport so held deals leave the loading count.
pub(super) fn prefill_counted(
    rows: Vec<super::job::QueuedRow>,
    target: Option<&Target>,
    stop: impl Fn() -> bool,
    missing: impl Fn(usize),
) -> (Vec<super::job::QueuedRow>, usize, usize) {
    let _busy = Busy::new();
    // The rows' windows were built under the inputs in force now; an answer that lands after
    // they moved was asked for stretches no row needs any more.
    let judged_under = unfillable().generation;
    let place = |row: &super::job::QueuedRow| {
        (
            row.address.exchange_key.clone(),
            row.address.market.clone(),
            row.window,
        )
    };
    let held_spans = |exchange: &str, market: &str, from, to| {
        trade_cache::handle()?.held_spans(exchange, market, from, to)
    };
    // Sampled at the end of the fill, before the second disk check: a stop that lands during
    // the pull must not be remembered, and one that lands later is the same single sample.
    let mut stopped = false;
    let allow_short = |row: &super::job::QueuedRow, stored: &Coverage| {
        let trail = stored
            .hull()
            .map_or(0, |(_, to)| to.saturating_sub(row.window.close_ms).max(0));
        trail < floor_ms(super::super::tail::effective_s())
            && !super::super::tail::fillable_at(
                &row.address,
                row.deal.report_uid,
                row.window.close_ms,
                trail,
                moon_core::util::time::now_unix_ms_i64(),
            )
    };
    let (rows, held, from_station, filled) =
        prefill_rows(rows, place, held_spans, allow_short, |rows| {
            missing(rows.len());
            let Some(target) = target.filter(|_| !stop()) else {
                return Filled::default();
            };
            let now = moon_core::util::time::now_unix_ms_i64();
            let candidates: Vec<_> = rows
                .iter()
                .filter(|row| candidate(row.deal.core_uid, row.deal.report_uid))
                // One short answer is enough until the tail can be final: asked every pass
                // meanwhile, the station files a few more seconds each time.
                .filter(|row| {
                    !unfillable().waiting_tail(row.deal.core_uid, row.deal.report_uid, now)
                })
                .collect();
            let filled = fill(&candidates, |row| place(row), target, &stop);
            super::autoload::station_answered(filled.unreachable || filled.failed);
            stopped = stop();
            filled
        });
    commit_station_answer(&rows, filled, stopped, judged_under);
    let rows = rows
        .into_iter()
        .filter(|row| candidate(row.deal.core_uid, row.deal.report_uid))
        .collect();
    (rows, held, from_station)
}

/// File a station answer, then remember rows it left uncovered when nothing else can fetch them.
///
/// Filing invalidates only the affected market's refusals before this answer is remembered.
/// A row whose post-close tail the station may still be writing is not remembered
/// ([`tail_may_follow`]).
///
/// Args:
///     rows: Rows still missing after the station pass.
///     filled: The station's answer. `markets > 0` and not `unreachable` means it was asked.
///     stopped: The pass was cancelled after the answer; those rows stay eligible.
///     judged_under: The refusal generation the rows' windows were built under; an answer that
///         lands under another one is filed but not remembered as a refusal.
///
/// Returns:
///     Nothing.
fn commit_station_answer(
    rows: &[super::job::QueuedRow],
    filled: Filled,
    stopped: bool,
    judged_under: Generation,
) {
    note_filed(filled);
    let now = moon_core::util::time::now_unix_ms_i64();
    let min_tail_s = super::super::tail::effective_s();
    // A row whose tail can still arrive is left unrefused, and the earliest moment one of them
    // can be asked about again is armed for the tick's wake. On every pass, an unanswered one
    // included: the wake takes the armed moment with it, so the rows still waiting after it
    // have only the next pass to arm theirs.
    if let Some(retry_at) = rows
        .iter()
        .filter(|row| tail_may_follow(&row.window, min_tail_s, now))
        .map(|row| tail_settles_at(&row.window, min_tail_s))
        .min()
    {
        RETRY_AT.fetch_min(retry_at, Ordering::AcqRel);
    }
    if filled.markets == 0 || filled.unreachable || filled.failed || stopped {
        return;
    }
    let refused: Vec<_> = rows
        .iter()
        .filter(|row| !tail_may_follow(&row.window, min_tail_s, now))
        .filter(|row| crate::backend::station::known_cached() || !row.fetchable_after_station(now))
        .collect();
    if filled.pieces > 0 && !refused.is_empty() {
        log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks autoload: station filed {} piece(s), but {} row(s) remain uncovered; no repeat in this generation (partial coverage or cache eviction)",
            filled.pieces, refused.len()
        );
    }
    let mut refusals = unfillable();
    if refusals.generation != judged_under {
        return;
    }
    for row in rows
        .iter()
        .filter(|row| tail_may_follow(&row.window, min_tail_s, now))
    {
        refusals.deferred.insert(
            (row.deal.core_uid, row.deal.report_uid),
            tail_settles_at(&row.window, min_tail_s),
        );
    }
    let before = refusals.revision;
    for row in refused {
        refusals.refuse(
            row.deal.core_uid,
            row.deal.report_uid,
            &row.address.exchange_key,
            &row.address.market,
        );
    }
    let changed = before != refusals.revision;
    drop(refusals);
    if changed {
        super::job::station_filled();
    }
}

/// Preserve disk-before/station/disk-after ordering with the same coverage rule on both sides.
pub(super) fn prefill_rows<T>(
    rows: Vec<T>,
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    held_spans: impl Fn(&str, &str, i64, i64) -> Option<Vec<(i64, i64)>>,
    allow_short: impl Fn(&T, &Coverage) -> bool,
    fill: impl FnOnce(&[T]) -> Filled,
) -> (Vec<T>, usize, usize, Filled) {
    let (rows, held) = drop_held(rows, &place, &held_spans, &allow_short);
    if rows.is_empty() {
        return (rows, held, 0, Filled::default());
    }
    let filled = fill(&rows);
    let before = rows.len();
    let (rows, _) = drop_held(rows, place, held_spans, allow_short);
    let from_station = before - rows.len();
    (rows, held + from_station, from_station, filled)
}

/// Fill from the station before queuing eligible venue rows, entirely off the GPUI thread.
pub(super) fn prefill_then_enqueue(
    rows: Vec<super::job::QueuedRow>,
    defaults: HashMap<String, f64>,
) -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let generation = super::autoload::generation();
    let target = super::autoload::station_target();
    let busy = Busy::new();
    let spawned = std::thread::Builder::new()
        .name("tape-station-prefill".into())
        .spawn(move || {
            let _busy = busy;
            let stop = || super::autoload::generation() != generation;
            let (mut rows, _, _) = prefill(rows, target.as_ref(), stop);
            if crate::backend::station::known_cached() {
                let _ = tx.send(());
                return;
            }
            let now = moon_core::util::time::now_unix_ms_i64();
            rows.retain(|row| row.fetchable_after_station(now));
            rows.sort_by_key(|row| std::cmp::Reverse(row.deal.close_ms));
            super::autoload::enqueue_if_current(generation, rows, defaults);
            let _ = tx.send(());
        });
    if let Err(error) = spawned {
        log::warn!("[x] ticks station: prefill thread did not start: {error}");
    }
    rx
}

/// What the file lacks of `rows`' windows, per market: the whole window the tuner reads, not only
/// what marks a row covered — a stretch the station holds costs no core or venue request later —
/// minus the spans already filed.
///
/// Args:
///     rows: The rows.
///     place: A row's `(exchange key, market, window)`.
///     held_spans: `(exchange, market, from_ms, to_ms)` → the filed spans' bounds; `None` when the
///         read did not happen, and then the whole window is asked.
pub(super) fn lacking<T>(
    rows: &[T],
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    held_spans: impl Fn(&str, &str, i64, i64) -> Option<Vec<(i64, i64)>>,
) -> Vec<TapeWant> {
    let mut wanted: HashMap<(String, String), Coverage> = HashMap::new();
    for row in rows {
        let (exchange, market, window) = place(row);
        let need = wanted
            .entry((exchange, market))
            .or_insert_with(Coverage::none);
        for &span in focus_with_tail(
            &window,
            record_tail_ms(super::super::tail::current_s(), window.margin_ms),
        )
        .spans()
        {
            need.add(span);
        }
    }
    let mut wants: Vec<TapeWant> = wanted
        .into_iter()
        .filter_map(|((exchange, market), need)| {
            let (from_ms, to_ms) = need.hull()?;
            let held = held_spans(&exchange, &market, from_ms, to_ms)
                .map(Coverage::from_spans)
                .unwrap_or_else(Coverage::none);
            let lacking = need.minus(&held);
            (!lacking.is_empty()).then(|| TapeWant {
                exchange,
                market,
                spans: lacking.spans().to_vec(),
            })
        })
        .collect();
    wants.sort_by(|a, b| (&a.exchange, &a.market).cmp(&(&b.exchange, &b.market)));
    wants
}

/// What one fill brought.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Filled {
    /// Markets asked.
    pub markets: usize,
    /// Covered stretches filed, and the prints in them.
    pub pieces: usize,
    pub prints: usize,
    /// The station could not be reached at all: the next passes leave it out for a while rather
    /// than wait for it again on every retry.
    pub unreachable: bool,
    /// A partial pull or persistence failure is not an answered absence of tape.
    pub failed: bool,
}

/// Ask the station for every stretch of `rows`' windows the file does not hold, and file what it
/// answers as the station's. Blocking — SSH round trips — so it runs on the pass's own thread.
///
/// Args:
///     rows: The rows the file does not fully hold.
///     place: A row's `(exchange key, market, window)`.
///     target: The station's server.
///     stop: Asked before every request: a Stop or superseding pass ends the fill there.
pub(super) fn fill<T>(
    rows: &[T],
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    target: &Target,
    stop: impl Fn() -> bool,
) -> Filled {
    let mut filled = Filled::default();
    // A worker spawn failure leaves nowhere to file the station's tape.
    let Some(cache) = trade_cache::handle() else {
        return filled;
    };
    let wants = lacking(rows, place, |exchange, market, from_ms, to_ms| {
        cache.held_spans(exchange, market, from_ms, to_ms)
    });
    filled.markets = wants.len();
    if wants.is_empty() {
        return filled;
    }
    let pull = match moon_remote::station::pull::Pull::open(target) {
        Ok(pull) => pull,
        Err(e) => {
            log::warn!(
                target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                "[x] ticks autoload: the station is not reachable, left out for ten minutes: {e:#}"
            );
            filled.unreachable = true;
            return filled;
        }
    };
    let pulled = pull.tape(wants, stop, |want, from_ms, to_ms, ticks| {
        filled.pieces += 1;
        filled.prints += ticks.len();
        cache.insert_from_station(&want.exchange, &want.market, from_ms, to_ms, ticks);
        unfillable().filed(&want.exchange, &want.market);
    });
    // Whatever came before an error or a stop is filed too: the pass judges the rows by the file
    // right after this.
    if filled.pieces > 0 && !cache.sync(FILE_SYNC) {
        filled.failed = true;
        log::warn!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks autoload: the station's tape did not reach the file in time; the rest is judged without it"
        );
    }
    match pulled {
        Ok(count) => log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks autoload: the station answered {} stretch(es), {} print(s), for {} market(s) in {} request(s){}",
            count.pieces,
            count.prints,
            filled.markets,
            count.requests,
            if count.stopped { ", stopped" } else { "" }
        ),
        Err(e) => {
            filled.failed = true;
            log::warn!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks autoload: the station's tape pull broke off after {} stretch(es): {e:#}",
            filled.pieces
            );
        }
    }
    filled
}

#[cfg(test)]
mod tests;
