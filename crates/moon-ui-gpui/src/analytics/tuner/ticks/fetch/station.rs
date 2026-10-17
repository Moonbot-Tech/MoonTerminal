//! The station as the first source of the autoload (STATION.md §4.9): the station records the live
//! stream around every trade while the terminal is off, so what the terminal lacks of a closed
//! trade's tape is asked of it before a core's archive or a venue — those only get what it did
//! not hold.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use moon_core::db::tuner::ticks::tail::{
    floor_ms, focus_with_tail, record_tail_ms, required_with_floor,
};
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
/// Stop serializes against the final enqueue without tying user rows to autoload switches.
static CONTROL: Mutex<u64> = Mutex::new(0);

/// Station refusals survive unrelated filings, but not changes to their admission inputs.
#[derive(Default)]
struct Unfillable {
    generation: (u64, u32, (bool, bool)),
    revision: u64,
    uids: HashMap<i64, (String, String)>,
    snapshot_revision: u64,
    snapshot: Arc<HashSet<i64>>,
}

impl Unfillable {
    /// Rebuild membership only after a mutation; every unchanged pass shares the same set.
    fn snapshot(&mut self) -> (u64, Arc<HashSet<i64>>) {
        if self.snapshot_revision != self.revision {
            self.snapshot = Arc::new(self.uids.keys().copied().collect());
            self.snapshot_revision = self.revision;
        }
        (self.revision, Arc::clone(&self.snapshot))
    }

    /// A changed floor, source policy or station finalisation permits another attempt.
    fn refresh(&mut self, generation: (u64, u32, (bool, bool))) {
        if self.generation != generation {
            self.generation = generation;
            if !self.uids.is_empty() {
                self.uids.clear();
                self.revision += 1;
            }
        }
    }

    /// Remember a refusal and invalidate classifications only when membership changes.
    fn refuse(&mut self, uid: i64, exchange: &str, market: &str) {
        if self
            .uids
            .insert(uid, (exchange.into(), market.into()))
            .is_none()
        {
            self.revision += 1;
        }
    }

    /// Only a filing on this market can make its refused rows worth checking again.
    fn filed(&mut self, exchange: &str, market: &str) {
        let before = self.uids.len();
        self.uids
            .retain(|_, key| key.0 != exchange || key.1 != market);
        if self.uids.len() != before {
            self.revision += 1;
        }
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
        crate::backend::station::tail_final_cached(),
        super::super::tail::effective_s(),
        super::super::tail::switches(),
    ));
    rows
}

/// Whether a station-only row still deserves an automatic request in this generation.
pub(in crate::analytics::tuner) fn candidate(uid: i64) -> bool {
    !unfillable().uids.contains_key(&uid)
}

/// One pass's refusal membership and revision, without a global lock for every row.
pub(in crate::analytics::tuner) fn refusal_snapshot() -> (u64, Arc<HashSet<i64>>) {
    unfillable().snapshot()
}

/// Refresh admission caches even when no tape row changed.
pub(in crate::analytics::tuner) fn refusal_revision() -> u64 {
    unfillable().revision
}

/// Only actual station filing invalidates held-row queries.
fn note_filed(filled: Filled) {
    if filled.pieces > 0 {
        FILED.fetch_add(1, Ordering::Release);
        super::job::station_filled();
    }
}

/// Lock the station prefill cancellation epoch.
fn control() -> MutexGuard<'static, u64> {
    CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Cancel station work before stopping the venue queue.
pub(super) fn cancel() {
    *control() += 1;
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
    let floor = floor_ms(min_s);
    let mut by_market: HashMap<(String, String), Vec<(T, Coverage, ReplayWindow)>> = HashMap::new();
    for row in rows {
        let (exchange, market, window) = place(&row);
        let need = required_with_floor(&window, floor);
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
                let focus = focus_with_tail(&window, record_tail_ms(min_s, margin_ms()));
                !need.is_empty()
                    && (stored.covers(&need)
                        || (allow_short(&row, &stored.clip(&focus))
                            && stored.covers(&super::super::tail::through_close(&window))))
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
    let _busy = Busy::new();
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
                super::super::tail::switches(),
            )
    };
    let (rows, held, from_station, filled) =
        prefill_rows(rows, place, held_spans, allow_short, |rows| {
            let Some(target) = target.filter(|_| !stop()) else {
                return Filled::default();
            };
            let candidates: Vec<_> = rows
                .iter()
                .filter(|row| {
                    row.origin == super::job::RowOrigin::User || candidate(row.deal.report_uid)
                })
                .collect();
            let filled = fill(&candidates, |row| place(row), target, &stop);
            super::autoload::station_answered(filled.unreachable);
            stopped = stop();
            filled
        });
    commit_station_answer(&rows, filled, stopped);
    let rows = rows
        .into_iter()
        .filter(|row| row.origin == super::job::RowOrigin::User || candidate(row.deal.report_uid))
        .collect();
    (rows, held, from_station)
}

/// File a station answer, then remember rows it left uncovered when nothing else can fetch them.
///
/// Filing invalidates only the affected market's refusals before this answer is remembered.
///
/// Args:
///     rows: Rows still missing after the station pass.
///     filled: The station's answer. `markets > 0` and not `unreachable` means it was asked.
///     stopped: The pass was cancelled after the answer; those rows stay eligible.
///
/// Returns:
///     Nothing.
fn commit_station_answer(rows: &[super::job::QueuedRow], filled: Filled, stopped: bool) {
    note_filed(filled);
    if filled.markets == 0 || filled.unreachable || stopped {
        return;
    }
    let now = moon_core::util::time::now_unix_ms_i64();
    let refused: Vec<_> = rows
        .iter()
        .filter(|row| !row.fetchable_after_station(now))
        .collect();
    let mut refusals = unfillable();
    let before = refusals.revision;
    for row in refused {
        refusals.refuse(
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
fn prefill_rows<T>(
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
    origin: super::job::RowOrigin,
) -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let generation = super::autoload::generation();
    let epoch = *control();
    let target = super::autoload::station_target();
    let busy = Busy::new();
    let spawned = std::thread::Builder::new()
        .name("tape-station-prefill".into())
        .spawn(move || {
            let _busy = busy;
            let stop = || {
                *control() != epoch
                    || (origin == super::job::RowOrigin::Autoload
                        && (super::autoload::generation() != generation
                            || !moon_core::market::trade_replay::tape_autoload()))
            };
            let (mut rows, _, _) = prefill(rows, target.as_ref(), stop);
            let now = moon_core::util::time::now_unix_ms_i64();
            rows.retain(|row| row.fetchable_after_station(now));
            rows.sort_by_key(|row| std::cmp::Reverse(row.deal.close_ms));
            let current = control();
            if *current == epoch {
                if origin == super::job::RowOrigin::User {
                    super::job::enqueue(rows, defaults);
                } else {
                    super::autoload::enqueue_if_current(generation, rows, defaults);
                }
            }
            drop(current);
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
fn lacking<T>(
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
            record_tail_ms(super::super::tail::current_s(), margin_ms()),
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
}

/// Ask the station for every stretch of `rows`' windows the file does not hold, and file what it
/// answers as the station's. Blocking — SSH round trips — so it runs on the pass's own thread.
///
/// Args:
///     rows: The rows the file does not fully hold.
///     place: A row's `(exchange key, market, window)`.
///     target: The station's server.
///     stop: Asked before every request: a Stop, or the switches going off, ends the fill there.
pub(super) fn fill<T>(
    rows: &[T],
    place: impl Fn(&T) -> (String, String, ReplayWindow),
    target: &Target,
    stop: impl Fn() -> bool,
) -> Filled {
    let mut filled = Filled::default();
    // Tape persistence switched off: nowhere to file what the station holds.
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
        Err(e) => log::warn!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks autoload: the station's tape pull broke off after {} stretch(es): {e:#}",
            filled.pieces
        ),
    }
    filled
}

#[cfg(test)]
mod tests;
