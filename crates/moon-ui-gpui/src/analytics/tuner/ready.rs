//! Tuning readiness for EVERY strategy of the list: the "Tuning" column, its filter and the
//! progress line above the table — on the "Entry/Exit" axis only.
//!
//! The pass reads the list scope's deals once, then replays them strategy by strategy through
//! the card's own stage C (`ticks::fetch_batch` and `ticks::replay_fetched`) and counts each
//! strategy with the card's own [`Readiness::of_rows`]; the verdict is the card's
//! (`ticks::readiness_refusal`). It never runs the search. It runs only while the "Entry/Exit"
//! axis is on screen — the rows the list draws first, the reads of the next strategies beside
//! the replay of the current ones in the tuner's pool — and publishes to the view at most once
//! per progress tick —
//! never once per strategy: every `cx.notify()` rebuilds the whole Analytics tree.
//!
//! It reruns when the list's scope is reloaded by hand or the model settings change (at once),
//! and when the report moved — at most once per [`RERUN_GAP`], and never on top of a pass still
//! running. A tape filing recounts only the strategies with a tapeless row on a filed market,
//! and none at all when it touched none of them: leaving the tab and coming back, or picking a
//! row (the card's own replay files its markets), keeps the finished counts.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::*;
use moon_core::db::ReadFail;
use moon_core::db::tuner::ticks::{Deal, ModelSettings};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::auto_picks::AutoEmpty;
use super::ticks::Fetched;
use super::ticks::readiness::Readiness;
use super::ticks::readiness_refusal;
use super::ticks::signature::SamplePolicy;
use super::ticks::state::{DealRow, RowAddress, TapeStatus, TicksData};
use crate::analytics::bg::ReadLane;
use crate::analytics::refresh::CatchUpOutcome;

/// How long a burst of input changes keeps coalescing before the pass is launched.
const LAUNCH_DEBOUNCE: Duration = Duration::from_millis(400);
/// The shortest time between two passes started for a report or tape change: a busy fleet
/// moves the report every few seconds, and each pass replays the whole list.
const RERUN_GAP: Duration = Duration::from_secs(60);
/// How often the progress line repaints — and the partial results land — while a pass runs.
const PROGRESS_TICK: Duration = Duration::from_millis(250);
/// Pause before a busy database is asked again.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// What one strategy's readiness says, as the column and the filter read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum ReadyClass {
    /// The card would offer the search; `fit` trades in the sample.
    Can { fit: usize },
    /// Some trades are covered by tape, but the sample is under the floor.
    Few { fit: usize },
    /// No trade of the strategy is covered by tape.
    NoTape,
    /// Not computed yet (running, or the strategy has no trades in the period).
    Pending,
    /// The pass's read failed; the progress line says why.
    Unread,
}

impl ReadyClass {
    /// Classify one strategy's counts by the card's own verdict.
    ///
    /// Args:
    ///     ready: The strategy's sample, covered and fetchable rows.
    ///
    /// Returns:
    ///     `Can` exactly when the card would not refuse ([`readiness_refusal`]); otherwise
    ///     `NoTape` when no row is covered, else `Few`.
    pub(in crate::analytics) fn of(ready: &Readiness) -> Self {
        if readiness_refusal(ready).is_none() {
            ReadyClass::Can { fit: ready.fit }
        } else if ready.covered == 0 {
            ReadyClass::NoTape
        } else {
            ReadyClass::Few { fit: ready.fit }
        }
    }

    /// Sort rank: tunable first, then the ones closest to it, everything else after.
    pub(in crate::analytics) fn rank(self) -> u8 {
        match self {
            ReadyClass::Can { .. } => 0,
            ReadyClass::Few { .. } => 1,
            ReadyClass::NoTape => 2,
            ReadyClass::Pending => 3,
            ReadyClass::Unread => 4,
        }
    }

    /// The sample inside a rank, so the bigger sample leads its group.
    pub(in crate::analytics) fn size(self) -> usize {
        match self {
            ReadyClass::Can { fit } | ReadyClass::Few { fit } => fit,
            _ => 0,
        }
    }
}

/// The strategy toolbar's "Tuning" filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum ReadyFilter {
    All,
    /// Only strategies the card would offer the search for.
    Can,
}

impl ReadyFilter {
    /// Every filter, in menu order.
    pub(in crate::analytics) const ALL: [ReadyFilter; 2] = [ReadyFilter::All, ReadyFilter::Can];

    /// Does a strategy of this class pass?
    pub(in crate::analytics) fn keeps(self, class: ReadyClass) -> bool {
        match self {
            ReadyFilter::All => true,
            ReadyFilter::Can => matches!(class, ReadyClass::Can { .. }),
        }
    }

    /// Stable, locale-independent element key.
    pub(in crate::analytics) fn key(self) -> &'static str {
        match self {
            ReadyFilter::All => "all",
            ReadyFilter::Can => "can",
        }
    }

    /// The menu item's text.
    pub(in crate::analytics) fn label(self) -> String {
        match self {
            ReadyFilter::All => t!("analytics.ready.filter_all").to_string(),
            ReadyFilter::Can => t!("analytics.ready.filter_can").to_string(),
        }
    }
}

/// The pass's classes as the list's filter-and-sort memo reads them.
///
/// Compared by generation and filter only: the class map is rebuilt exactly when the generation
/// moves.
#[derive(Clone, Debug)]
pub(in crate::analytics) struct ReadyView {
    pub(in crate::analytics) generation: u64,
    pub(in crate::analytics) filter: ReadyFilter,
    pub(in crate::analytics) classes: Arc<HashMap<String, ReadyClass>>,
    /// The class of a row the pass holds no count for.
    pub(in crate::analytics) missing: ReadyClass,
}

impl PartialEq for ReadyView {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation && self.filter == other.filter
    }
}

impl Default for ReadyView {
    fn default() -> Self {
        Self {
            generation: 0,
            filter: ReadyFilter::All,
            classes: Arc::default(),
            missing: ReadyClass::Pending,
        }
    }
}

impl ReadyView {
    /// The class of one strategy-list row.
    pub(in crate::analytics) fn class(&self, key: &str) -> ReadyClass {
        self.classes.get(key).copied().unwrap_or(self.missing)
    }
}

/// What a pass was launched for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::analytics) struct ReadyKey {
    /// Bumped on every hand-made reload of the strategy list (period, mode, metric, cores).
    pub(in crate::analytics) scope_gen: u64,
    /// The report generation: a trade closed or a row was rewritten. The valuation generation
    /// is left out — a revaluation moves no deal's tape or sample. A move recounts only the
    /// strategies whose deals changed ([`ReadyPass::recount_set`]).
    pub(in crate::analytics) report_gen: u64,
    /// The tape store's revision: prints were filed for some market. Only a filed market some
    /// listed strategy trades on makes counts stale ([`ReadyPass::stale_strategies`]).
    pub(in crate::analytics) tape_rev: u64,
    /// The model the rows are judged under.
    pub(in crate::analytics) model: ModelSettings,
    /// The sample policy the rows are read and judged under: the tail floor, the automatic
    /// sources, the model's margin and the long-position split — the card's own
    /// ([`SamplePolicy`]).
    pub(in crate::analytics) sample: SamplePolicy,
    /// The station's refusal revision: a refused or admitted row changes its verdict.
    pub(in crate::analytics) refusal_revision: u64,
    /// The cores' catalogs: an address resolving where it did not (a core connecting, its
    /// catalog arriving) changes the groups' signatures, so the deals are read again.
    pub(in crate::analytics) addresses_rev: u64,
    /// The step-lag calibration ([`super::ticks::lags::revision`]): a group replayed on a core
    /// whose calibration moved is recounted ([`group_signature`]).
    pub(in crate::analytics) calibration_rev: u64,
}

impl ReadyKey {
    /// Whether moving from `self` to `next` makes every count wrong at once (another scope,
    /// model, sample policy or station refusal set), rather than a few strategies' counts stale
    /// (a report, tape, address or calibration change).
    pub(in crate::analytics) fn restarts(&self, next: &ReadyKey) -> bool {
        self.scope_gen != next.scope_gen
            || self.model != next.model
            || self.sample != next.sample
            || self.refusal_revision != next.refusal_revision
    }

    /// Whether moving to `next` must read the deals again to know which strategies changed.
    fn rereads(&self, next: &ReadyKey) -> bool {
        self.report_gen != next.report_gen
            || self.addresses_rev != next.addresses_rev
            || self.calibration_rev != next.calibration_rev
    }
}

/// The running pass's progress and its counts not yet handed to the view.
#[derive(Default)]
pub(in crate::analytics) struct ReadyProgress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Strategies a partial pass keeps from the last one: counted as done from the start.
    kept: AtomicUsize,
    cancelled: AtomicBool,
    /// Tape bytes fetched and not yet counted, and the most ever held at once.
    inflight: AtomicUsize,
    peak: AtomicUsize,
    /// Signalled whenever the replay frees tape bytes, for a fetch thread waiting for room.
    room: (Mutex<()>, std::sync::Condvar),
    /// The size of every chunk the replay took, in order.
    #[cfg(test)]
    chunks: Mutex<Vec<usize>>,
    partial: Mutex<Vec<(String, Readiness)>>,
    /// `(strategy, (exchange key, market))` of each replayed row left without tape: the only
    /// rows a later filing can change.
    waiting: Mutex<Vec<(String, (String, String))>>,
    /// Per counted strategy, the earliest moment the clock alone changes one of its rows'
    /// verdicts ([`verdict_deadline`]), and whether every held read behind it answered settled.
    deadlines: Mutex<Vec<(String, Option<i64>, bool)>>,
}

/// Remaining time at the observed pass speed, after a three-second warm-up.
fn eta(done_this_pass: usize, total_this_pass: usize, elapsed: Duration) -> Option<Duration> {
    if done_this_pass == 0 || elapsed < Duration::from_secs(3) || done_this_pass >= total_this_pass
    {
        return None;
    }
    Some(elapsed.mul_f64((total_this_pass - done_this_pass) as f64 / done_this_pass as f64))
}

impl ReadyProgress {
    /// Estimate remaining replay time using only work performed in this pass, excluding kept rows.
    fn remaining(&self, elapsed: Duration) -> Option<Duration> {
        eta(
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
            elapsed,
        )
    }

    /// Strategies counted so far and how many the pass holds, the kept ones included.
    pub(in crate::analytics) fn counts(&self) -> (usize, usize) {
        let kept = self.kept.load(Ordering::Relaxed);
        (
            self.done.load(Ordering::Relaxed) + kept,
            self.total.load(Ordering::Relaxed) + kept,
        )
    }

    /// Ask the running pass to stop at the next strategy.
    pub(in crate::analytics) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether [`Self::cancel`] was called.
    pub(in crate::analytics) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Take the counts finished since the last call.
    pub(in crate::analytics) fn drain(&self) -> Vec<(String, Readiness)> {
        std::mem::take(
            &mut *self
                .partial
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Take the tapeless markets noted since the last call.
    fn drain_waiting(&self) -> Vec<(String, (String, String))> {
        std::mem::take(
            &mut *self
                .waiting
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Note that a replayed row of `key` on `market` was left without tape.
    pub(in crate::analytics) fn wait_on(&self, key: &str, market: (String, String)) {
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((key.to_string(), market));
    }

    /// The most tape bytes the pass held at once.
    #[cfg(test)]
    pub(in crate::analytics) fn peak_bytes(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    fn hold(&self, bytes: usize) {
        let now = self.inflight.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak.fetch_max(now, Ordering::Relaxed);
    }

    fn release(&self, bytes: usize) {
        self.inflight.fetch_sub(bytes, Ordering::Relaxed);
        let (lock, freed) = &self.room;
        let _held = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        freed.notify_all();
    }

    /// Tape bytes fetched and not yet counted.
    fn inflight(&self) -> usize {
        self.inflight.load(Ordering::Relaxed)
    }

    /// Wait until fewer than `budget` bytes are held, or `cancelled` says stop (the pass was
    /// cancelled or its replay side is gone).
    ///
    /// Returns:
    ///     `false` when told to stop while waiting.
    fn wait_for_room(&self, budget: usize, cancelled: &impl Fn() -> bool) -> bool {
        let (lock, freed) = &self.room;
        let mut held = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while self.inflight() >= budget {
            if cancelled() {
                return false;
            }
            held = freed
                .wait_timeout(held, Duration::from_millis(50))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        !cancelled()
    }

    fn settle(&self, key: String, deadline: Option<i64>, settled: bool) {
        self.deadlines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((key, deadline, settled));
    }

    fn drain_deadlines(&self) -> Vec<(String, Option<i64>, bool)> {
        std::mem::take(
            &mut *self
                .deadlines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn push(&self, key: String, ready: Readiness) {
        self.partial
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((key, ready));
        self.done.fetch_add(1, Ordering::Relaxed);
    }
}

/// Where the pass stands.
pub(in crate::analytics) enum ReadyRun {
    Idle,
    Running {
        progress: Arc<ReadyProgress>,
    },
    Done,
    /// The read failed; `retrying` when it was a busy database and a retry is armed.
    Failed {
        fail: ReadFail,
        retrying: bool,
    },
}

/// The pass's state on the view.
pub(in crate::analytics) struct ReadyPass {
    /// Generation of the pass allowed to publish; a superseded reply is dropped.
    pub(in crate::analytics) seq: u64,
    /// Inputs of the pass launched last (or armed to launch).
    pub(in crate::analytics) launched: Option<ReadyKey>,
    /// Inputs of the pass that actually started last.
    pub(in crate::analytics) ran: Option<ReadyKey>,
    /// When the last pass started, for [`RERUN_GAP`].
    pub(in crate::analytics) started: Option<Instant>,
    /// Inputs that came due while a pass was running; launched once it ends.
    pub(in crate::analytics) queued: bool,
    pub(in crate::analytics) run: ReadyRun,
    /// The counts per strategy-list row key: the last pass's, refreshed as the running one
    /// answers.
    pub(in crate::analytics) counts: HashMap<String, Readiness>,
    /// Which strategies trade on each `(exchange key, market)`, as the passes replayed them —
    /// a tape filing elsewhere leaves every count standing.
    markets: HashMap<(String, String), HashSet<String>>,
    /// The running pass recounts only some strategies and merges into `counts`.
    partial: bool,
    /// Each counted strategy's deals and addresses, folded ([`group_signature`]): a report
    /// move recounts only the strategies whose signature changed.
    signatures: HashMap<String, u64>,
    /// The running pass's signatures of the strategies it recounts, adopted when it lands.
    pending: HashMap<String, u64>,
    /// When the clock alone flips a verdict of some strategy's rows (a retention edge, an
    /// archive retry); past it, that strategy is recounted.
    deadlines: HashMap<String, i64>,
    /// Strategies whose deadline passed and for which a pass is armed.
    due: HashSet<String>,
    /// Bumped whenever `counts` or `run` changes, so the list's memo knows to re-sort.
    pub(in crate::analytics) generation: u64,
    /// `counts` classified, shared with the list's memo.
    classes: Arc<HashMap<String, ReadyClass>>,
    /// The filter the user picked from the menu; `None` until then ("all").
    pub(in crate::analytics) chosen: Option<ReadyFilter>,
    /// Debounce timer, progress ticker and retry timer; dropping one cancels it.
    pub(in crate::analytics) launch_task: Option<Task<()>>,
    pub(in crate::analytics) ticker: Option<Task<()>>,
    /// Wakes the view at the earliest verdict deadline, so a passed one is acted on without
    /// waiting for an unrelated render.
    deadline_task: Option<Task<()>>,
}

impl Default for ReadyPass {
    fn default() -> Self {
        Self {
            seq: 0,
            launched: None,
            ran: None,
            started: None,
            queued: false,
            run: ReadyRun::Idle,
            counts: HashMap::new(),
            markets: HashMap::new(),
            partial: false,
            signatures: HashMap::new(),
            pending: HashMap::new(),
            deadlines: HashMap::new(),
            due: HashSet::new(),
            generation: 0,
            classes: Arc::default(),
            chosen: None,
            launch_task: None,
            ticker: None,
            deadline_task: None,
        }
    }
}

impl ReadyPass {
    /// The filter in force on one axis of the strategy list.
    ///
    /// Args:
    ///     ticks_axis: The list is on the "Entry/Exit" axis, where the column lives.
    ///
    /// Returns:
    ///     "all" on every other axis, whatever the menu holds: the filter is drawn only on
    ///     "Entry/Exit", so it must not narrow a list where it cannot be seen. There, the
    ///     user's menu choice, "can tune" until one is made.
    pub(in crate::analytics) fn filter(&self, ticks_axis: bool) -> ReadyFilter {
        if ticks_axis {
            self.chosen.unwrap_or(ReadyFilter::Can)
        } else {
            ReadyFilter::All
        }
    }

    /// The classes and the filter, for the list's memo and its rows.
    pub(in crate::analytics) fn view(&self, ticks_axis: bool) -> ReadyView {
        ReadyView {
            generation: self.generation,
            filter: self.filter(ticks_axis),
            classes: self.classes.clone(),
            missing: self.view_missing(),
        }
    }

    /// The class of a row the pass holds no count for: unread after a failure; after a
    /// finished pass, a strategy without trades in the scope, which the card refuses as having
    /// nothing to tune on; pending otherwise.
    pub(in crate::analytics) fn view_missing(&self) -> ReadyClass {
        if matches!(self.run, ReadyRun::Failed { .. }) {
            ReadyClass::Unread
        } else if matches!(self.run, ReadyRun::Done) {
            ReadyClass::of(&Readiness::default())
        } else {
            ReadyClass::Pending
        }
    }

    /// The class of one row, as the cell draws it.
    pub(in crate::analytics) fn class(&self, key: &str) -> ReadyClass {
        self.counts
            .get(key)
            .map_or_else(|| self.view_missing(), ReadyClass::of)
    }

    /// What an EMPTY strategy list shows under the filter: `None` when it is off, "still
    /// computing" until the pass has answered, "none can be tuned" once it did.
    pub(in crate::analytics) fn empty_face(&self, filter: ReadyFilter) -> Option<AutoEmpty> {
        if filter == ReadyFilter::All {
            None
        } else if matches!(self.run, ReadyRun::Done | ReadyRun::Failed { .. }) {
            Some(AutoEmpty::NoFinding)
        } else {
            Some(AutoEmpty::Computing)
        }
    }

    /// How long to wait before launching a pass for `key`.
    ///
    /// Args:
    ///     key: The inputs now.
    ///     now: The current instant.
    ///
    /// Returns:
    ///     The debounce for a first pass, a new scope or a new model; for a report or tape
    ///     change, whatever is left of [`RERUN_GAP`] since the last start, at least the debounce.
    pub(in crate::analytics) fn delay(&self, key: &ReadyKey, now: Instant) -> Duration {
        match (self.ran, self.started) {
            (Some(ran), Some(started)) if !ran.restarts(key) => RERUN_GAP
                .saturating_sub(now.saturating_duration_since(started))
                .max(LAUNCH_DEBOUNCE),
            _ => LAUNCH_DEBOUNCE,
        }
    }

    /// Start a pass of generation `seq + 1`, retiring whatever ran before.
    ///
    /// Args:
    ///     key: The inputs it runs with.
    ///     now: When it starts.
    ///
    /// Returns:
    ///     The new generation and its progress handle. The counts of the last pass stay on
    ///     screen until this one replaces them, unless the scope or the model moved: those
    ///     counts describe nothing any more.
    pub(in crate::analytics) fn begin(
        &mut self,
        key: ReadyKey,
        now: Instant,
    ) -> (u64, Arc<ReadyProgress>) {
        self.start(key, now, None)
    }

    /// Start a pass that recounts only `only`, keeping every other strategy's count.
    ///
    /// Args:
    ///     key: The inputs it runs with.
    ///     now: When it starts.
    ///     only: The strategies whose counts went stale ([`Self::stale_strategies`]).
    ///
    /// Returns:
    ///     As [`Self::begin`]; the progress counts the kept strategies as done.
    pub(in crate::analytics) fn begin_partial(
        &mut self,
        key: ReadyKey,
        now: Instant,
        only: &HashSet<String>,
    ) -> (u64, Arc<ReadyProgress>) {
        self.start(key, now, Some(only))
    }

    fn start(
        &mut self,
        key: ReadyKey,
        now: Instant,
        only: Option<&HashSet<String>>,
    ) -> (u64, Arc<ReadyProgress>) {
        if let ReadyRun::Running { progress } = &self.run {
            progress.cancel();
        }
        if self.ran.is_none_or(|ran| ran.restarts(&key)) {
            self.counts.clear();
            self.classes = Arc::default();
            self.signatures.clear();
            self.deadlines.clear();
        }
        self.partial = only.is_some();
        self.pending.clear();
        self.due.clear();
        match only {
            None => self.markets.clear(),
            // The recount files its strategies' tapeless rows afresh.
            Some(only) => self.markets.retain(|_, keys| {
                keys.retain(|k| !only.contains(k));
                !keys.is_empty()
            }),
        }
        let kept = only.map_or(0, |only| {
            self.counts.keys().filter(|k| !only.contains(*k)).count()
        });
        self.seq = self.seq.wrapping_add(1);
        self.launched = Some(key);
        self.ran = Some(key);
        self.started = Some(now);
        self.queued = false;
        let progress = Arc::new(ReadyProgress::default());
        progress.kept.store(kept, Ordering::Relaxed);
        // Until the replay stores its own total, the bar must not read as already full.
        if let Some(only) = only {
            progress.total.store(only.len(), Ordering::Relaxed);
        }
        self.run = ReadyRun::Running {
            progress: progress.clone(),
        };
        self.generation = self.generation.wrapping_add(1);
        (self.seq, progress)
    }

    /// Fold the counts the running pass finished since the last tick — one batch per tick.
    ///
    /// Returns:
    ///     Whether anything was folded (the list must re-sort).
    pub(in crate::analytics) fn absorb(&mut self, seq: u64) -> bool {
        let ReadyRun::Running { progress } = &self.run else {
            return false;
        };
        if seq != self.seq {
            return false;
        }
        let waiting = progress.drain_waiting();
        let batch = progress.drain();
        self.wait(waiting);
        if batch.is_empty() {
            return false;
        }
        self.counts.extend(batch);
        self.reclassify();
        true
    }

    /// Publish a pass's reply.
    ///
    /// Args:
    ///     seq: The generation the pass was launched under.
    ///     result: The background answer; `Ok(None)` is a cancelled pass.
    ///
    /// Returns:
    ///     Whether it landed (`false` for a superseded or cancelled reply) and, when it failed,
    ///     whether the failure is a busy database worth retrying.
    pub(in crate::analytics) fn land(
        &mut self,
        seq: u64,
        result: moon_core::db::ReadResult<Option<HashMap<String, Readiness>>>,
    ) -> (bool, bool) {
        if seq != self.seq {
            return (false, false);
        }
        let mut deadlines = Vec::new();
        if let ReadyRun::Running { progress } = &self.run {
            let waiting = progress.drain_waiting();
            deadlines = progress.drain_deadlines();
            self.wait(waiting);
        }
        match result {
            Ok(Some(counts)) => {
                let mut pending = std::mem::take(&mut self.pending);
                // A strategy whose held reads did not all settle is not vouched for: no
                // signature is adopted for it, and its deadline asks again soon.
                for (key, _, settled) in &deadlines {
                    if !settled {
                        pending.remove(key);
                        self.signatures.remove(key);
                    }
                }
                if self.partial {
                    self.signatures.extend(pending);
                    self.deadlines.retain(|k, _| !counts.contains_key(k));
                    self.counts.extend(counts);
                } else {
                    self.signatures = pending;
                    self.deadlines.clear();
                    self.counts = counts;
                }
                for (key, deadline, _) in deadlines {
                    match deadline {
                        Some(at) => self.deadlines.insert(key, at),
                        None => self.deadlines.remove(&key),
                    };
                }
                self.reclassify();
                self.run = ReadyRun::Done;
                (true, false)
            }
            Ok(None) => {
                self.run = ReadyRun::Idle;
                self.generation = self.generation.wrapping_add(1);
                (false, false)
            }
            Err(fail) => {
                let retrying = CatchUpOutcome::of_read::<()>(&Err(fail.clone())).is_transient();
                self.run = ReadyRun::Failed { fail, retrying };
                self.generation = self.generation.wrapping_add(1);
                (true, retrying)
            }
        }
    }

    /// What a pass for `key` would have to recount, measured against the finished one.
    ///
    /// Args:
    ///     key: The inputs now.
    ///     refiled: The markets filed since the finished pass's tape revision, as
    ///         `trade_cache::filed_since` names them.
    ///
    ///     now_ms: The clock, against the strategies' verdict deadlines.
    ///
    /// Returns:
    ///     `None` when only a whole pass answers it: no finished pass, another scope, model,
    ///     tail floor, refusal set or source switch. Otherwise the strategies trading on a
    ///     refiled market or past a verdict deadline — empty when the move touched none of them
    ///     (a tab switch, a selection, a print filed for another list). A report move adds the
    ///     strategies whose deals changed once the pass has read them ([`Self::recount_set`]).
    pub(in crate::analytics) fn stale_strategies(
        &self,
        key: &ReadyKey,
        refiled: &HashSet<(String, String)>,
        now_ms: i64,
    ) -> Option<HashSet<String>> {
        let ran = self.ran?;
        if !matches!(self.run, ReadyRun::Done) || ran.restarts(key) {
            return None;
        }
        Some(
            refiled
                .iter()
                .filter_map(|m| self.markets.get(m))
                .flatten()
                .chain(self.due.iter())
                .chain(
                    self.deadlines
                        .iter()
                        .filter(|(_, at)| **at <= now_ms)
                        .map(|(k, _)| k),
                )
                .cloned()
                .collect(),
        )
    }

    /// Whether the finished counts stand for `key` as they are: nothing went stale and neither
    /// the report, the addresses nor the calibration moved (a move must read the deals to know
    /// what changed).
    pub(in crate::analytics::tuner) fn stands(
        &self,
        key: &ReadyKey,
        stale: Option<&HashSet<String>>,
    ) -> bool {
        stale.is_some_and(HashSet::is_empty) && self.ran.is_some_and(|ran| !ran.rereads(key))
    }

    /// The earliest verdict deadline of a finished pass.
    pub(in crate::analytics::tuner) fn next_deadline(&self) -> Option<i64> {
        if !matches!(self.run, ReadyRun::Done) {
            return None;
        }
        self.deadlines.values().copied().min()
    }

    /// Move the strategies whose verdict deadline passed to `due`, once.
    ///
    /// Returns:
    ///     Whether any did — a finished pass then has strategies to recount though its
    ///     inputs did not move.
    pub(in crate::analytics::tuner) fn take_due(&mut self, now_ms: i64) -> bool {
        if !matches!(self.run, ReadyRun::Done) {
            return false;
        }
        let due: Vec<String> = self
            .deadlines
            .iter()
            .filter(|(_, at)| **at <= now_ms)
            .map(|(k, _)| k.clone())
            .collect();
        if due.is_empty() {
            return false;
        }
        for key in &due {
            self.deadlines.remove(key);
        }
        self.due.extend(due);
        true
    }

    /// Settle what the running pass recounts once its deals are read and grouped.
    ///
    /// Args:
    ///     groups: Every strategy of the scope with its rows ([`group_by_strategy`]).
    ///     only: For a partial pass, the strategies already known stale; `None` for a whole
    ///         pass.
    ///     progress: The running pass's handle; its kept and total counts are set here.
    ///
    /// Returns:
    ///     `None` for a whole pass. Otherwise `only` plus every strategy whose deals or
    ///     addresses changed and every new one. A strategy gone from the scope loses its count.
    pub(in crate::analytics::tuner) fn recount_set(
        &mut self,
        groups: &[StrategyRows],
        only: Option<&HashSet<String>>,
        progress: &ReadyProgress,
    ) -> Option<HashSet<String>> {
        let signatures: HashMap<String, u64> = groups
            .iter()
            .map(|(key, rows)| (key.clone(), group_signature(rows)))
            .collect();
        let Some(only) = only else {
            self.pending = signatures;
            return None;
        };
        let recount: HashSet<String> = signatures
            .iter()
            .filter(|(key, sig)| only.contains(*key) || self.signatures.get(*key) != Some(sig))
            .map(|(key, _)| key.clone())
            .collect();
        let before = self.counts.len();
        self.counts.retain(|k, _| signatures.contains_key(k));
        self.signatures.retain(|k, _| signatures.contains_key(k));
        self.deadlines.retain(|k, _| signatures.contains_key(k));
        self.markets.retain(|_, keys| {
            keys.retain(|k| signatures.contains_key(k) && !recount.contains(k));
            !keys.is_empty()
        });
        if self.counts.len() != before {
            self.reclassify();
        }
        self.pending = signatures
            .into_iter()
            .filter(|(key, _)| recount.contains(key))
            .collect();
        let kept = self.counts.keys().filter(|k| !recount.contains(*k)).count();
        progress.kept.store(kept, Ordering::Relaxed);
        progress.total.store(recount.len(), Ordering::Relaxed);
        Some(recount)
    }

    /// Take `key` as the finished pass's own inputs: nothing it counted moved.
    pub(in crate::analytics) fn adopt(&mut self, key: ReadyKey) {
        self.launched = Some(key);
        self.ran = Some(key);
    }

    /// File tapeless rows under their market, for [`Self::stale_strategies`].
    fn wait(&mut self, waiting: Vec<(String, (String, String))>) {
        for (key, market) in waiting {
            self.markets.entry(market).or_default().insert(key);
        }
    }

    /// Rebuild the class map from `counts` and bump the generation.
    fn reclassify(&mut self) {
        self.classes = Arc::new(
            self.counts
                .iter()
                .map(|(k, r)| (k.clone(), ReadyClass::of(r)))
                .collect(),
        );
        self.generation = self.generation.wrapping_add(1);
    }

    /// The progress sentence and whether it is a warning.
    ///
    /// Args:
    ///     total: The strategy list after every filter but this one — the header's "of Y".
    ///
    /// Returns:
    ///     `None` before the first pass, or when the database is simply absent.
    pub(in crate::analytics) fn status(&self, total: usize) -> Option<(String, bool)> {
        match &self.run {
            ReadyRun::Idle => None,
            ReadyRun::Running { progress } => {
                let (done, _) = progress.counts();
                let remaining = self
                    .started
                    .and_then(|start| progress.remaining(start.elapsed()));
                let text = match remaining {
                    Some(remaining) => t!(
                        "analytics.ready.progress_eta",
                        k = done.min(total),
                        m = total,
                        eta = super::ticks::estimate::duration_text(remaining)
                    )
                    .to_string(),
                    None => {
                        t!("analytics.ready.progress", k = done.min(total), m = total).to_string()
                    }
                };
                Some((text, false))
            }
            ReadyRun::Done => {
                let n = self
                    .classes
                    .values()
                    .filter(|c| ReadyFilter::Can.keeps(**c))
                    .count();
                Some((
                    t!("analytics.ready.done", n = n, m = total).to_string(),
                    false,
                ))
            }
            ReadyRun::Failed {
                fail: ReadFail::NotReady,
                ..
            } => None,
            ReadyRun::Failed { retrying: true, .. } => {
                Some((t!("analytics.ready.busy").to_string(), true))
            }
            ReadyRun::Failed { .. } => Some((t!("analytics.ready.failed").to_string(), true)),
        }
    }

    /// The running pass's progress as a 0..=1 fraction, for the thin bar.
    pub(in crate::analytics) fn fraction(&self) -> Option<f32> {
        match &self.run {
            ReadyRun::Running { progress } => {
                let (done, of) = progress.counts();
                Some(if of == 0 {
                    0.0
                } else {
                    (done as f32 / of as f32).min(1.0)
                })
            }
            _ => None,
        }
    }
}

/// The cell's hover text: the card's own sentence for a strategy it would refuse, the sample
/// against the floor for one it would tune.
///
/// Args:
///     ready: The strategy's counts.
pub(in crate::analytics) fn cell_tip(ready: &Readiness) -> String {
    let headline = readiness_refusal(ready).unwrap_or_else(|| {
        t!(
            "analytics.ready.tip_can",
            n = ready.fit,
            min = Readiness::min()
        )
        .to_string()
    });
    let mut lines = vec![
        headline,
        t!(
            "analytics.ticks.why.breakdown",
            deals = ready.deals,
            no_tape = ready.no_tape,
            short_tail = ready.short_tail,
            holed = ready.holed,
            missed = ready.missed,
            unjudged = ready.unjudged,
            fit = ready.fit
        )
        .to_string(),
        t!("analytics.ticks.why.floor", min = Readiness::min()).to_string(),
    ];
    if ready.fetchable > 0 {
        let key = super::plural_key("analytics.ticks.why.fetchable", ready.fetchable);
        lines.push(t!(key.as_str(), n = ready.fetchable).to_string());
    }
    // The count of deals still loading is the autoload's, not the yield above. Idle
    // autoload (`None`) keeps the sentence and drops the number; a strategy with nothing
    // left to fetch stays on the breakdown alone.
    if ready.fetchable > 0 || super::ticks::fetch::autoload::pending_rows().is_some() {
        lines.push(super::ticks::why::background_loading_line());
    }
    lines.join("\n")
}

/// One strategy's rows before their replay: the deals, each with where its prints live.
pub(in crate::analytics::tuner) type StrategyRows = (String, Vec<(Deal, Option<Arc<RowAddress>>)>);

/// Group the scope's deals per strategy-list row key, in the order `order` names first (the
/// list as drawn, so the rows on screen answer first), then the rest in key order.
///
/// Args:
///     deals: The scope's deals.
///     addresses: Where each `(core, coin)` lives, as the card resolves it.
///     order: Row keys in the order the list draws them.
pub(in crate::analytics::tuner) fn group_by_strategy(
    deals: Vec<Deal>,
    addresses: &HashMap<(u64, String), Option<Arc<RowAddress>>>,
    order: &[String],
) -> Vec<StrategyRows> {
    let mut groups: HashMap<String, Vec<(Deal, Option<Arc<RowAddress>>)>> = HashMap::new();
    for deal in deals {
        let address = addresses
            .get(&(deal.core_uid, deal.coin.clone()))
            .cloned()
            .flatten();
        groups
            .entry(format!("{}@{}", deal.strategy_id, deal.core_uid))
            .or_default()
            .push((deal, address));
    }
    let mut out: Vec<StrategyRows> = Vec::with_capacity(groups.len());
    for key in order {
        if let Some(rows) = groups.remove(key) {
            out.push((key.clone(), rows));
        }
    }
    let mut rest: Vec<StrategyRows> = groups.into_iter().collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(rest);
    out
}

/// One strategy's deals, where their prints live and the step-lag calibration of their cores,
/// folded: equal signatures replay the same rows
/// ([`super::ticks::signature::signature_of`] over the deals, every address and core beside).
pub(in crate::analytics::tuner) fn group_signature(
    rows: &[(Deal, Option<Arc<RowAddress>>)],
) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write_u64(super::ticks::signature::signature_of(
        rows.iter().map(|(deal, _)| deal),
    ));
    super::ticks::signature::hash_sorted(
        &mut hasher,
        rows.iter()
            .map(|(deal, address)| ((deal.core_uid, deal.report_uid), address)),
    );
    super::ticks::lags::hash_calibration(&mut hasher, rows.iter().map(|(deal, _)| deal.core_uid));
    hasher.finish()
}

/// The earliest moment past `now_ms` at which the clock alone changes a verdict among `rows`:
/// a retention edge of a covered short tail or a missing row (the card's own rule,
/// [`super::ticks::rows::next_verdict_flip`]) or an archive retry coming due.
pub(in crate::analytics::tuner) fn verdict_deadline(
    rows: &[DealRow],
    now_ms: i64,
    min_tail_s: u32,
) -> Option<i64> {
    rows.iter()
        .filter_map(|row| {
            let retry = match row.tape {
                TapeStatus::Refused(
                    moon_core::market::trade_replay::TickStatus::ArchivePending { retry_at_ms },
                ) => Some(retry_at_ms),
                _ => None,
            };
            [
                super::ticks::rows::next_verdict_flip(row, min_tail_s, now_ms),
                retry,
            ]
            .into_iter()
            .flatten()
            .filter(|&at| at > now_ms)
            .min()
        })
        .min()
}

/// Tape bytes a pass reads ahead of the replay: the fetch thread starts no further strategy
/// while the chunk being replayed and the ones fetched behind it hold this much. The check
/// comes before each strategy's fetch, so the most ever held is this budget plus the one
/// strategy fetched just under it.
pub(in crate::analytics::tuner) const READY_INFLIGHT_BYTES: usize = 256 << 20;

/// How a pass cuts its strategies into chunks.
#[derive(Clone, Copy, Debug)]
pub(in crate::analytics::tuner) struct ChunkLimits {
    /// Strategies in the first chunk, so the first counts land after a short read; each next
    /// chunk doubles, up to `max_count`.
    pub(in crate::analytics::tuner) first_count: usize,
    /// Most strategies in one chunk.
    pub(in crate::analytics::tuner) max_count: usize,
    /// Tape bytes in memory at which the fetch thread waits for the replay to free some
    /// ([`READY_INFLIGHT_BYTES`]).
    pub(in crate::analytics::tuner) max_bytes: usize,
}

impl ChunkLimits {
    /// The app's: four strategies first, then up to enough to keep every pool thread busy.
    pub(in crate::analytics::tuner) fn app() -> Self {
        Self {
            first_count: 4,
            max_count: (2 * moon_core::db::tuner::threshold_search::search_threads()).max(4),
            max_bytes: READY_INFLIGHT_BYTES,
        }
    }
}

/// One strategy of a chunk: its addressed rows as fetched (`None` when it has none - nothing
/// is asked or replayed for it) and its rows without an address.
struct Fetching {
    key: String,
    fetched: Option<Fetched>,
    unaddressed: Vec<DealRow>,
    bytes: usize,
}

/// Set when the replay side of a pass ends, by a return or a panic, so a fetch thread waiting
/// for room stops instead of waiting for bytes nobody will free.
struct ConsumerGone<'a>(&'a AtomicBool);

impl Drop for ConsumerGone<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// The raw prints a fetched batch holds.
fn tape_bytes(fetched: &Fetched) -> usize {
    fetched
        .tapes
        .values()
        .map(|(ticks, _, _)| ticks.len() * std::mem::size_of::<moon_core::feed::types::Tick>())
        .sum()
}

/// Count every strategy: its rows with an address fetched by `fetch` and replayed by `replay`
/// (the card's stage C), the others kept as the card keeps them (no address), then counted
/// with [`Readiness::of_rows`]. Each count is handed to `progress` as it is made.
///
/// The strategies go in `groups`' order, in chunks ([`ChunkLimits`]): one thread of its own
/// fetches the next chunk - the blocking waits on the tape worker stay off the pool - while
/// the tuner's pool replays the current one, one task per strategy. A strategy's rows are
/// counted alone, never mixed with another's, so the card's memory cap applies per strategy.
/// Chunks go in priority order; within a chunk each strategy pushes its count to `progress`
/// as it finishes, so counts inside one chunk land in any order.
///
/// Args:
///     groups: Each strategy's rows ([`group_by_strategy`]).
///     fetch: Reads a batch of addressed deals; `ticks::fetch_batch` in the app.
///     replay: Replays what `fetch` read; `ticks::replay_fetched` in the app.
///     progress: Takes each count; a cancel stops the pass at the next strategy - up to one
///         strategy's fetch later, since a fetch under way is not interrupted.
///     limits: The chunk cut ([`ChunkLimits::app`] in the app).
///     now_ms: One clock for the whole pass ([`Readiness::of_rows`]).
///     min_tail_s: One tail floor for the whole pass.
///
/// Returns:
///     Every strategy's counts, or `None` when the pass was cancelled midway.
pub(in crate::analytics::tuner) fn count_strategies(
    groups: Vec<StrategyRows>,
    fetch: impl Fn(Vec<(Deal, Arc<RowAddress>)>) -> Fetched + Sync,
    replay: impl Fn(Fetched) -> Vec<DealRow> + Sync,
    progress: &ReadyProgress,
    limits: ChunkLimits,
    now_ms: i64,
    min_tail_s: u32,
) -> Option<HashMap<String, Readiness>> {
    progress.total.store(groups.len(), Ordering::Relaxed);
    let cancelled = || progress.is_cancelled() || moon_core::db::current_is_cancelled();
    if cancelled() {
        return None;
    }
    let token = moon_core::db::current_read_cancellation();
    let budget = limits.max_bytes.max(1);
    let max_count = limits.max_count.max(1);
    let first_count = limits.first_count.clamp(1, max_count);
    // Depth zero: the fetch thread holds the chunk it fetched ahead until the replay takes it.
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<Fetching>>(0);
    let mut out = HashMap::with_capacity(groups.len());
    let gone = AtomicBool::new(false);
    let finished = std::thread::scope(|scope| {
        let fetch = &fetch;
        let gone = &gone;
        let stop = move || cancelled() || gone.load(Ordering::Relaxed);
        let fetcher = move || {
            let mut chunk: Vec<Fetching> = Vec::new();
            let mut cap = first_count;
            for (key, rows) in groups {
                if cancelled() {
                    return;
                }
                // Memory full: what is fetched goes to the replay, and nothing more is read
                // until the replay frees room.
                if progress.inflight() >= budget {
                    if !chunk.is_empty() {
                        if tx.send(std::mem::take(&mut chunk)).is_err() {
                            return;
                        }
                        cap = (cap * 2).min(max_count);
                    }
                    if !progress.wait_for_room(budget, &stop) {
                        return;
                    }
                }
                let mut targets = Vec::new();
                let mut unaddressed = Vec::new();
                for (deal, address) in rows {
                    match address {
                        Some(address) => targets.push((deal, address)),
                        None => unaddressed.push(DealRow {
                            deal,
                            tape: TapeStatus::NoAddress,
                            verdict: None,
                            address: None,
                            ticks: None,
                            entry_line: None,
                            held: None,
                        }),
                    }
                }
                let fetched = (!targets.is_empty()).then(|| fetch(targets));
                let item_bytes = fetched.as_ref().map_or(0, tape_bytes);
                progress.hold(item_bytes);
                chunk.push(Fetching {
                    key,
                    fetched,
                    unaddressed,
                    bytes: item_bytes,
                });
                // A full chunk goes at once, so nothing past it is read before it is taken.
                if chunk.len() >= cap {
                    if tx.send(std::mem::take(&mut chunk)).is_err() {
                        return;
                    }
                    cap = (cap * 2).min(max_count);
                }
            }
            if !chunk.is_empty() {
                let _ = tx.send(chunk);
            }
        };
        // Owned here, so returning early drops it and a fetch thread waiting to hand a chunk
        // over sees the send fail and ends.
        let rx = rx;
        let _gone = ConsumerGone(gone);
        scope.spawn(move || match token {
            Some(token) => moon_core::db::with_read_cancellation(token, fetcher),
            None => fetcher(),
        });
        loop {
            if cancelled() {
                return false;
            }
            let Ok(chunk) = rx.recv() else {
                break;
            };
            #[cfg(test)]
            progress.chunks.lock().unwrap().push(chunk.len());
            let counted = moon_core::db::tuner::threshold_search::par_map_job(chunk, |item| {
                if cancelled() {
                    return None;
                }
                let Fetching {
                    key,
                    fetched,
                    unaddressed,
                    bytes,
                } = item;
                let settled = fetched.as_ref().is_none_or(|fetched| {
                    fetched
                        .rows
                        .iter()
                        .all(|row| fetched.settled.contains(&row.deal.report_uid))
                });
                let replayed = fetched.map(&replay).unwrap_or_default();
                for address in replayed
                    .iter()
                    .filter(|row| row.tape != TapeStatus::Covered)
                    .filter_map(|row| row.address.as_ref())
                {
                    progress.wait_on(&key, (address.exchange_key.clone(), address.market.clone()));
                }
                // The card's memory cap on its tape, so a fit row the card would let go of is
                // not counted here either.
                let mut data = TicksData {
                    rows: replayed.into_iter().chain(unaddressed).collect(),
                    ..Default::default()
                };
                data.retain_within_cap();
                let ready = Readiness::of_rows(&data.rows, now_ms, min_tail_s);
                let mut deadline = verdict_deadline(&data.rows, now_ms, min_tail_s);
                if !settled {
                    let retry = now_ms.saturating_add(RETRY_AFTER.as_millis() as i64);
                    deadline = Some(deadline.map_or(retry, |at| at.min(retry)));
                }
                progress.settle(key.clone(), deadline, settled);
                progress.release(bytes);
                progress.push(key.clone(), ready);
                Some((key, ready))
            });
            for counted in counted {
                let Some((key, ready)) = counted else {
                    return false;
                };
                out.insert(key, ready);
            }
        }
        !cancelled()
    });
    finished.then_some(out)
}

impl AnalyticsView {
    /// The inputs a pass for the current view would run with.
    fn ready_key(&self, cx: &App) -> ReadyKey {
        ReadyKey {
            scope_gen: self.auto_picks.scope_gen,
            report_gen: self
                .report_generation
                .as_ref()
                .map_or(0, |generation| generation.load(Ordering::Relaxed)),
            tape_rev: moon_core::market::trade_replay::trade_cache::filed_since(u64::MAX).0,
            model: super::ticks::model_cfg::current(),
            sample: SamplePolicy::current(),
            refusal_revision: super::ticks::fetch::station::refusal_revision(),
            addresses_rev: self.catalogs_revision(cx),
            calibration_rev: super::ticks::lags::revision(),
        }
    }

    /// Every configured core's catalog revision and whether it resolves an address, folded.
    fn catalogs_revision(&self, cx: &App) -> u64 {
        use std::hash::{Hash, Hasher};
        let backend = self.backend.read(cx);
        let source = backend.session.market_source();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for server in &backend.config.servers {
            server.id.hash(&mut hasher);
            source.catalog_revision(server.id).hash(&mut hasher);
            source.replay_address(server.id).is_ok().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Arm a pass when the list is loaded on "Entry/Exit" and its inputs moved since the last
    /// launch. Called on every render of the strategy list; cheap when nothing moved.
    pub(in crate::analytics::tuner) fn ensure_ready_pass(&mut self, cx: &mut Context<Self>) {
        if !self.strat_mode.shows_ready() {
            return;
        }
        let loaded = self.strategy_data.data().is_some()
            && !matches!(
                self.strategy_data,
                crate::analytics::profit_load::ProfitLoadState::Loading
            );
        let key = self.ready_key(cx);
        if !loaded {
            return;
        }
        let now_ms = moon_core::util::time::now_unix_ms_i64();
        if self.ready.launched == Some(key) {
            // The inputs stand, but the clock may have flipped some strategy's verdict.
            if !self.ready.take_due(now_ms) {
                return;
            }
        }
        // A tab switch or a selection moves the tape revision (the card's own replay files its
        // markets) and nothing else; a finished pass none of whose markets were filed stands.
        if self
            .ready
            .stands(&key, self.ready_stale(&key, now_ms).as_ref())
        {
            self.ready.adopt(key);
            return;
        }
        // Marked as launched now so the next render does not re-arm the timer.
        self.ready.launched = Some(key);
        let delay = self.ready.delay(&key, Instant::now());
        self.ready.launch_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(delay).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // A later change re-armed the timer with its own key; this one is retired.
                    if this.ready.launched != Some(key) {
                        return;
                    }
                    // A report or tape change does not cut a running pass short — on a busy
                    // fleet it would never finish; it runs once the current one ends.
                    if matches!(this.ready.run, ReadyRun::Running { .. })
                        && this.ready.ran.is_some_and(|ran| !ran.restarts(&key))
                    {
                        this.ready.queued = true;
                        return;
                    }
                    let stale = this.ready_stale(&key, moon_core::util::time::now_unix_ms_i64());
                    if this.ready.stands(&key, stale.as_ref()) {
                        this.ready.adopt(key);
                    } else {
                        this.run_ready_pass(key, stale, cx);
                    }
                });
            });
        }));
    }

    /// The strategies a pass for `key` must recount ([`ReadyPass::stale_strategies`]), with
    /// the markets filed since the finished pass read off the tape store now.
    fn ready_stale(&self, key: &ReadyKey, now_ms: i64) -> Option<HashSet<String>> {
        let since = self.ready.ran?.tape_rev;
        let (_, refiled) = moon_core::market::trade_replay::trade_cache::filed_since(since);
        self.ready.stale_strategies(key, &refiled, now_ms)
    }

    /// Run the pass now: the scope's deals off the database and their addresses resolved
    /// beside the read, then the per-strategy replay in the background.
    ///
    /// Args:
    ///     key: The inputs it runs with.
    ///     only: Recount just these strategies and keep the rest; `None` recounts all.
    ///     cx: Analytics context.
    fn run_ready_pass(
        &mut self,
        key: ReadyKey,
        only: Option<HashSet<String>>,
        cx: &mut Context<Self>,
    ) {
        let q = self.tuner_query_all();
        let mut resolver = self.address_resolver(cx);
        let (seq, progress) = match &only {
            Some(only) => self.ready.begin_partial(key, Instant::now(), only),
            None => self.ready.begin(key, Instant::now()),
        };
        self.spawn_latest_db(
            &[ReadLane::TicksReady],
            false,
            cx,
            move || {
                moon_core::db::tuner::ticks::read_deals(&q).map(|read| {
                    let addresses = super::ticks::resolve_in(&mut resolver, &read.deals);
                    (read.deals, addresses)
                })
            },
            move |this, read, cx| {
                if this.ready.seq != seq {
                    return;
                }
                match read {
                    Ok((deals, addresses)) => {
                        this.run_ready_replay(key, seq, deals, addresses, only, progress, cx)
                    }
                    Err(fail) => this.finish_ready_pass(key, seq, Err(fail), cx),
                }
            },
        );
        self.arm_ready_ticker(seq, cx);
        self.strat_visible = None;
        cx.notify();
    }

    /// The strategy-list keys in the order the pass counts them: the rows the user sees first
    /// in the list's current sort, whatever the tuning filter hides
    /// ([`super::list::ready_priority_indices`]).
    fn ready_priority_order(&self) -> Vec<String> {
        let Some(data) = self.strategy_data.data() else {
            return Vec::new();
        };
        let all = &data.strategies;
        super::list::ready_priority_indices(all, self.strat_visible_key(all))
            .into_iter()
            .filter_map(|i| all.get(i))
            .map(|g| g.key.clone())
            .collect()
    }

    /// The pass's second stage: every strategy replayed and counted, chunk by chunk.
    fn run_ready_replay(
        &mut self,
        key: ReadyKey,
        seq: u64,
        deals: Vec<Deal>,
        addresses: HashMap<(u64, String), Option<Arc<RowAddress>>>,
        only: Option<HashSet<String>>,
        progress: Arc<ReadyProgress>,
        cx: &mut Context<Self>,
    ) {
        let order = self.ready_priority_order();
        let mut groups = group_by_strategy(deals, &addresses, &order);
        if let Some(recount) = self.ready.recount_set(&groups, only.as_ref(), &progress) {
            groups.retain(|(key, _)| recount.contains(key));
        }
        let defaults = self.filter_defaults(cx);
        let klines = self.backend.read(cx).session.market_source().kline_cache();
        let model = key.model;
        let min_tail_s = key.sample.min_tail_s;
        self.spawn_latest_db(
            &[ReadLane::TicksReady],
            false,
            cx,
            move || {
                let long_position_ms = key.sample.long_position_ms;
                let now_ms = chrono::Utc::now().timestamp_millis();
                // One strategy reader for the whole pass.
                let reads = super::ticks::PassReads::new();
                // The reads on a thread of their own, the replay in the tuner's pool: there it
                // shares the search's CPU budget instead of adding threads on top.
                count_strategies(
                    groups,
                    |targets| super::ticks::fetch_batch(targets, long_position_ms),
                    |fetched| {
                        super::ticks::replay_fetched(
                            fetched,
                            &defaults,
                            model,
                            klines.as_ref(),
                            now_ms,
                            Some(&reads),
                        )
                    },
                    &progress,
                    ChunkLimits::app(),
                    now_ms,
                    min_tail_s,
                )
            },
            move |this, counts, cx| this.finish_ready_pass(key, seq, Ok(counts), cx),
        );
    }

    /// Publish a pass's end and launch whatever came due while it ran.
    fn finish_ready_pass(
        &mut self,
        key: ReadyKey,
        seq: u64,
        result: moon_core::db::ReadResult<Option<HashMap<String, Readiness>>>,
        cx: &mut Context<Self>,
    ) {
        let (landed, retry) = self.ready.land(seq, result);
        if seq == self.ready.seq {
            self.ready.ticker = None;
        }
        self.arm_ready_deadline(cx);
        // A finished pass changes what the list filters and sorts by.
        self.strat_visible = None;
        if landed && retry {
            self.arm_ready_retry(key, cx);
        } else if std::mem::take(&mut self.ready.queued) {
            // The next render re-arms the timer for the inputs now in force.
            self.ready.launched = None;
        }
        cx.notify();
    }

    /// Repaint the progress line — and fold the counts finished since the last tick — while the
    /// pass runs. ONE notify per tick, however many strategies answered in it.
    fn arm_ready_ticker(&mut self, seq: u64, cx: &mut Context<Self>) {
        self.ready.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(PROGRESS_TICK).await;
                let running = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        if this.ready.absorb(seq) {
                            this.strat_visible = None;
                        }
                        cx.notify();
                        this.ready.seq == seq && matches!(this.ready.run, ReadyRun::Running { .. })
                    })
                    .unwrap_or(false)
                });
                if !running {
                    break;
                }
            }
        }));
    }

    /// Wake the view at the finished pass's earliest verdict deadline: the next render takes
    /// the due strategies ([`ReadyPass::take_due`]) and launches their recount.
    fn arm_ready_deadline(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.ready.next_deadline() else {
            self.ready.deadline_task = None;
            return;
        };
        let wait = at
            .saturating_sub(moon_core::util::time::now_unix_ms_i64())
            .max(0) as u64;
        self.ready.deadline_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_millis(wait)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |_, cx| cx.notify());
            });
        }));
    }

    /// Ask a busy database again after a pause, unless the inputs moved meanwhile.
    fn arm_ready_retry(&mut self, key: ReadyKey, cx: &mut Context<Self>) {
        self.ready.launch_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(RETRY_AFTER).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.ready.launched == Some(key)
                        && matches!(this.ready.run, ReadyRun::Failed { .. })
                    {
                        this.run_ready_pass(key, None, cx);
                    }
                });
            });
        }));
    }
}

#[cfg(test)]
mod tests;
