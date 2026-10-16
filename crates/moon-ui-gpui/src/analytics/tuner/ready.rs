//! Tuning readiness for EVERY strategy of the list: the "Tuning" column, its filter and the
//! progress line above the table — on the "Entry/Exit" axis only.
//!
//! The pass reads the list scope's deals once, then replays them strategy by strategy through
//! the card's own stage C (`ticks::replay_batch`) and counts each strategy with the card's own
//! [`Readiness::of_rows`]; the verdict is the card's (`ticks::readiness_refusal`). It never
//! runs the search. It runs only while the "Entry/Exit" axis is on screen, one strategy at a
//! time with a pause between them, and publishes to the view at most once per progress tick —
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
use super::ticks::readiness::Readiness;
use super::ticks::readiness_refusal;
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
/// Pause between two strategies of a pass: the terminal runs beside live trading, and the
/// replay of a strategy is a burst of CPU; the pause hands the cores back between bursts.
const STRATEGY_PAUSE: Duration = Duration::from_millis(30);

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
    /// is left out — a revaluation moves no deal's tape or sample.
    pub(in crate::analytics) report_gen: u64,
    /// The tape store's revision: prints were filed for some market. Only a filed market some
    /// listed strategy trades on makes counts stale ([`ReadyPass::stale_strategies`]).
    pub(in crate::analytics) tape_rev: u64,
    /// The model the rows are judged under.
    pub(in crate::analytics) model: ModelSettings,
    /// The minimum tape tail past the close a row needs to be fit, in seconds.
    pub(in crate::analytics) min_tail_s: u32,
}

impl ReadyKey {
    /// Whether moving from `self` to `next` makes every count wrong at once (another scope or
    /// another model), rather than a few strategies' counts stale (a report or tape change).
    pub(in crate::analytics) fn restarts(&self, next: &ReadyKey) -> bool {
        self.scope_gen != next.scope_gen
            || self.model != next.model
            || self.min_tail_s != next.min_tail_s
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
    partial: Mutex<Vec<(String, Readiness)>>,
    /// `(strategy, (exchange key, market))` of each replayed row left without tape: the only
    /// rows a later filing can change.
    waiting: Mutex<Vec<(String, (String, String))>>,
}

impl ReadyProgress {
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
    /// Bumped whenever `counts` or `run` changes, so the list's memo knows to re-sort.
    pub(in crate::analytics) generation: u64,
    /// `counts` classified, shared with the list's memo.
    classes: Arc<HashMap<String, ReadyClass>>,
    /// The filter the user picked from the menu; `None` until then ("all").
    pub(in crate::analytics) chosen: Option<ReadyFilter>,
    /// Debounce timer, progress ticker and retry timer; dropping one cancels it.
    pub(in crate::analytics) launch_task: Option<Task<()>>,
    pub(in crate::analytics) ticker: Option<Task<()>>,
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
            generation: 0,
            classes: Arc::default(),
            chosen: None,
            launch_task: None,
            ticker: None,
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
    ///     user's menu choice, "all" until one is made.
    pub(in crate::analytics) fn filter(&self, ticks_axis: bool) -> ReadyFilter {
        if ticks_axis {
            self.chosen.unwrap_or(ReadyFilter::All)
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
        }
        self.partial = only.is_some();
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
        if let ReadyRun::Running { progress } = &self.run {
            let waiting = progress.drain_waiting();
            self.wait(waiting);
        }
        match result {
            Ok(Some(counts)) => {
                if self.partial {
                    self.counts.extend(counts);
                } else {
                    self.counts = counts;
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
    /// Returns:
    ///     `None` when only a whole pass answers it: no finished pass, another scope, model or
    ///     report. Otherwise the strategies trading on a refiled market — empty when the move
    ///     touched none of them (a tab switch, a selection, a print filed for another list),
    ///     and then the finished counts stand as they are.
    pub(in crate::analytics) fn stale_strategies(
        &self,
        key: &ReadyKey,
        refiled: &HashSet<(String, String)>,
    ) -> Option<HashSet<String>> {
        let ran = self.ran?;
        if !matches!(self.run, ReadyRun::Done)
            || ran.restarts(key)
            || ran.report_gen != key.report_gen
        {
            return None;
        }
        Some(
            refiled
                .iter()
                .filter_map(|m| self.markets.get(m))
                .flatten()
                .cloned()
                .collect(),
        )
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
                Some((
                    t!("analytics.ready.progress", k = done.min(total), m = total).to_string(),
                    false,
                ))
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
    readiness_refusal(ready).unwrap_or_else(|| {
        t!(
            "analytics.ready.tip_can",
            n = ready.fit,
            min = Readiness::min()
        )
        .to_string()
    })
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

/// Count every strategy, one at a time: its rows with an address replayed by `replay` (the
/// card's stage C), the others kept as the card keeps them (no address), then counted with
/// [`Readiness::of_rows`]. Each count is handed to `progress` as it is made.
///
/// Args:
///     groups: Each strategy's rows ([`group_by_strategy`]).
///     replay: Replays a batch of addressed deals; `ticks::replay_batch` in the app.
///     progress: Takes each count; a cancel stops the loop.
///     pause: The pause between two strategies ([`STRATEGY_PAUSE`] in the app).
///
/// Returns:
///     Every strategy's counts, or `None` when the pass was cancelled midway.
pub(in crate::analytics::tuner) fn count_strategies(
    groups: Vec<StrategyRows>,
    mut replay: impl FnMut(Vec<(Deal, Arc<RowAddress>)>) -> Vec<DealRow>,
    progress: &ReadyProgress,
    pause: Duration,
) -> Option<HashMap<String, Readiness>> {
    progress.total.store(groups.len(), Ordering::Relaxed);
    let mut out = HashMap::with_capacity(groups.len());
    for (i, (key, rows)) in groups.into_iter().enumerate() {
        if progress.is_cancelled() || moon_core::db::current_is_cancelled() {
            return None;
        }
        if i > 0 && !pause.is_zero() {
            std::thread::sleep(pause);
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
        let replayed = if targets.is_empty() {
            Vec::new()
        } else {
            replay(targets)
        };
        for address in replayed
            .iter()
            .filter(|row| row.tape != TapeStatus::Covered)
            .filter_map(|row| row.address.as_ref())
        {
            progress.wait_on(&key, (address.exchange_key.clone(), address.market.clone()));
        }
        // The card's memory cap on its tape, so a fit row the card would let go of is not
        // counted here either.
        let mut data = TicksData {
            rows: replayed.into_iter().chain(unaddressed).collect(),
            ..Default::default()
        };
        data.retain_within_cap();
        let ready = Readiness::of_rows(
            &data.rows,
            chrono::Utc::now().timestamp_millis(),
            super::ticks::tail::effective_s(),
        );
        progress.push(key.clone(), ready);
        out.insert(key, ready);
    }
    Some(out)
}

impl AnalyticsView {
    /// The inputs a pass for the current view would run with.
    fn ready_key(&self) -> ReadyKey {
        ReadyKey {
            scope_gen: self.auto_picks.scope_gen,
            report_gen: self
                .report_generation
                .as_ref()
                .map_or(0, |generation| generation.load(Ordering::Relaxed)),
            tape_rev: moon_core::market::trade_replay::trade_cache::filed_since(u64::MAX).0,
            model: super::ticks::model_cfg::current(),
            min_tail_s: super::ticks::tail::effective_s(),
        }
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
        let key = self.ready_key();
        if !loaded || self.ready.launched == Some(key) {
            return;
        }
        // A tab switch or a selection moves the tape revision (the card's own replay files its
        // markets) and nothing else; a finished pass none of whose markets were filed stands.
        if self.ready_stale(&key).is_some_and(|stale| stale.is_empty()) {
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
                    match this.ready_stale(&key) {
                        Some(stale) if stale.is_empty() => this.ready.adopt(key),
                        only => this.run_ready_pass(key, only, cx),
                    }
                });
            });
        }));
    }

    /// The strategies a pass for `key` must recount ([`ReadyPass::stale_strategies`]), with
    /// the markets filed since the finished pass read off the tape store now.
    fn ready_stale(&self, key: &ReadyKey) -> Option<HashSet<String>> {
        let since = self.ready.ran?.tape_rev;
        let (_, refiled) = moon_core::market::trade_replay::trade_cache::filed_since(since);
        self.ready.stale_strategies(key, &refiled)
    }

    /// Run the pass now: the scope's deals off the database, their addresses resolved here,
    /// then the per-strategy replay in the background.
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
        let (seq, progress) = match &only {
            Some(only) => self.ready.begin_partial(key, Instant::now(), only),
            None => self.ready.begin(key, Instant::now()),
        };
        self.spawn_latest_db(
            &[ReadLane::TicksReady],
            false,
            cx,
            move || moon_core::db::tuner::ticks::read_deals(&q),
            move |this, read, cx| {
                if this.ready.seq != seq {
                    return;
                }
                match read {
                    Ok(read) => this.run_ready_replay(key, seq, read.deals, only, progress, cx),
                    Err(fail) => this.finish_ready_pass(key, seq, Err(fail), cx),
                }
            },
        );
        self.arm_ready_ticker(seq, cx);
        self.strat_visible = None;
        cx.notify();
    }

    /// The pass's second stage: every strategy replayed and counted, one at a time.
    fn run_ready_replay(
        &mut self,
        key: ReadyKey,
        seq: u64,
        deals: Vec<Deal>,
        only: Option<HashSet<String>>,
        progress: Arc<ReadyProgress>,
        cx: &mut Context<Self>,
    ) {
        let addresses = self.resolve_addresses(&deals, cx);
        let order: Vec<String> = self
            .strategy_data
            .data()
            .map(|data| {
                self.visible_indices()
                    .iter()
                    .filter_map(|i| data.strategies.get(*i))
                    .map(|g| g.key.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut groups = group_by_strategy(deals, &addresses, &order);
        if let Some(only) = &only {
            groups.retain(|(key, _)| only.contains(key));
        }
        let defaults = self.filter_defaults(cx);
        let klines = self.backend.read(cx).session.market_source().kline_cache();
        let model = key.model;
        self.spawn_latest_db(
            &[ReadLane::TicksReady],
            false,
            cx,
            move || {
                let long_position_ms = moon_core::market::trade_replay::long_position_ms();
                count_strategies(
                    groups,
                    |targets| {
                        super::ticks::replay_batch(
                            targets,
                            &defaults,
                            model,
                            klines.as_ref(),
                            long_position_ms,
                            false,
                        )
                    },
                    &progress,
                    STRATEGY_PAUSE,
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
