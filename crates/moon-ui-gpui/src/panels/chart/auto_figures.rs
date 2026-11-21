//! The auto figures' recompute policy for one chart panel (`docs-internal/AUTO_FIGURES.md` §7).
//!
//! The engine half (`chartdx::auto_figures_sync`) only stores and draws what it is handed; this
//! half decides WHEN a row's detector runs. The rule is one key per row
//! ([`RowKey`]: detector parameters, resolved timeframe, open time of the last closed bar of that
//! timeframe, chart-history presence) and a run only when it moved. Concretely the causal chain is:
//!
//! - a bar of the row's timeframe closes → the key's time part moves at the next evaluation →
//!   ONE job for that pane, carrying only the rows whose key moved;
//! - the tab's auto-figure configuration changes → the keys of the edited rows move (a style edit
//!   moves no key and only re-publishes);
//! - the pane switches market → its slots are dropped and every drawn row runs once;
//! - the chart-label eye hides the pane → nothing is evaluated for it and its lines come off; the
//!   eye shows it again → the rows whose cached answer is for the current key are republished as
//!   they are, and only the others run. The eye's choice lives on the slots, so it ends with the
//!   market like they do, and while it stands it outranks the tab's switch ([`pane_shows`]);
//! - an `Auto` row's visible span crosses a hysteresis threshold AND stays past it for
//!   [`AUTO_TF_DWELL`] → that row's timeframe moves. Before the pane's time scale is measured an
//!   `Auto` row is not resolved at all: an unmeasured span would read as the finest timeframe and
//!   start an M1 job nobody asked for.
//!
//! The keys are evaluated on the panel's existing wakes — the backend observer, the render path
//! after a settings or camera change — and on ONE timer armed for the earliest moment a key can
//! next move, so a quiet market still recomputes on its candle close. Evaluating is a few integer
//! divisions per row; the detectors never run on that path. A mouse move reaches none of it.
//!
//! The job runs on the GPUI background executor (cache read, resample, detect), its completion
//! comes back through the task itself, and the publication bumps the engine's figure revision, so
//! the pane's userdata is rebuilt once — not the scene. Superseded results are recognised by key
//! and dropped, so a job outliving a newer one cannot overwrite it.
//!
//! What follows the PRICE — the kept set, each line's side, its events, the captions — is not
//! decided here but in the engine's price pass (`chartdx::auto_figures_sync`), which sees a price
//! change where it happens. This module publishes a run's answer selected against the price of
//! that moment, and the pass keeps it current. Hover, the card and the right-click menu are in
//! [`menu`].

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::Context;
use moon_chart::auto_figures::tf::{self, AutoTf};
use moon_core::config::auto_figures::{AutoFigureRow, AutoFiguresCfg, RowDetector, TfId, TfSource};
use moon_core::session::CoreId;

use super::ChartPanel;
use crate::chartdx::auto_figures_sync::job::{
    self, AutoRowOutcome, CacheAddr, PaneJob, RowKey, last_closed_open_ms, next_key_change_ms,
};
use crate::chartdx::auto_figures_sync::{AutoPaneFigures, AutoRowDrawn, LineKey, RowTitle};
use crate::chartdx::now_unix_ms;

/// Back-off after consecutive cache reads that did not happen (worker busy or gone), by failure
/// count: 10 s, 30 s, then 60 s for every further failure. Bounded both ways — a dead worker costs
/// one attempt a minute per row, never a loop, and a row still retries until the cache answers. A
/// market with NO cache address is not a failure and never enters this ladder.
const READ_RETRY: [Duration; 3] = [
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

/// How often a row whose newest bucket the cache had not filled yet is rechecked. The job stops
/// reporting it as lagging [`job::LAG_CAP_MS`] after the bucket's close, which bounds the rechecks
/// to about twenty per bar.
const LAG_RECHECK: Duration = Duration::from_secs(15);

/// How long an `Auto` row's span has to stay past a hysteresis threshold before the row switches
/// timeframe (`docs-internal/AUTO_FIGURES.md` §5: "switch only when the window went past the
/// threshold with a margin AND held there").
///
/// The margin alone stops jitter around one threshold; it does not stop a sweep. A wheel spun
/// through three notches, or a pan that drags the span across a threshold and back, crosses
/// thresholds on consecutive frames, and every crossing used to be a detector job and a redraw for
/// a timeframe that was gone a frame later. 400 ms is longer than a deliberate notch-to-notch
/// wheel rhythm and short enough that a zoom the reader stops at shows its lines without a
/// noticeable wait.
pub(super) const AUTO_TF_DWELL: Duration = Duration::from_millis(400);

/// How often an `Auto` row asks again while its pane's time scale is not measured yet. The first
/// market sync measures it, normally within a frame of the pane appearing, and the wake stops
/// being armed the moment it does.
const UNMEASURED_RECHECK: Duration = Duration::from_millis(250);

/// How many of those rechecks a pane gets before it waits for the panel's ordinary wakes instead:
/// five seconds. A pane that never gets a span (a book-only pane has none) must not turn this into
/// a 4 Hz wake for as long as it is open.
const UNMEASURED_RETRIES: u8 = 20;

/// `Auto`'s timeframe with a dwell on top of its hysteresis: the timeframe the row USES, and the
/// switch it is waiting to make.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DwellTf {
    /// The timeframe settled on, with its hysteresis memory.
    settled: AutoTf,
    /// A different timeframe the span has asked for, and since when.
    pending: Option<(TfId, Instant)>,
}

impl DwellTf {
    /// Feed the visible span at `now`; returns the timeframe to use and, while a switch is
    /// pending, when to ask again.
    ///
    /// The first answer is taken at once — there is nothing on screen to keep. After that a new
    /// answer from [`AutoTf::update`] is held for [`AUTO_TF_DWELL`]: it is adopted only when the
    /// span still asks for the same timeframe once the dwell has passed. A span that comes back to
    /// the settled timeframe, or asks for yet another one, restarts the wait.
    pub(super) fn update(&mut self, visible_ms: f64, now: Instant) -> (TfId, Option<Instant>) {
        let mut probe = self.settled;
        let next = probe.update(visible_ms);
        let Some(current) = self.settled.current() else {
            self.settled = probe;
            self.pending = None;
            return (next, None);
        };
        if next == current {
            self.pending = None;
            return (current, None);
        }
        match self.pending {
            Some((tf, since)) if tf == next => {
                if now.saturating_duration_since(since) >= AUTO_TF_DWELL {
                    self.settled = probe;
                    self.pending = None;
                    (next, None)
                } else {
                    (current, Some(since + AUTO_TF_DWELL))
                }
            }
            _ => {
                self.pending = Some((next, now));
                (current, Some(now + AUTO_TF_DWELL))
            }
        }
    }
}

/// One panel's auto-figure state: the configuration it last applied, the per-pane row slots and
/// the one timer waiting for the next candle close.
#[derive(Default)]
pub(super) struct AutoFiguresState {
    /// The configuration the slots were evaluated against.
    cfg: Option<AutoFiguresCfg>,
    /// One entry per pane target this panel shows auto figures for.
    panes: Vec<PaneSlots>,
    /// The armed wake: the Unix ms it fires at and its generation.
    timer: Option<(i64, u64)>,
    /// Generation of the most recently armed wake.
    timer_gen: u64,
    /// The auto line under the pointer.
    hover: menu::AutoHoverState,
}

/// One pane target's rows.
struct PaneSlots {
    target: (CoreId, String),
    /// Indexed like the configuration's rows.
    rows: Vec<RowSlot>,
    /// Lines the user hid from the right-click menu. Not persisted, and gone with the slots when
    /// the pane changes market — "hidden until the market changes".
    hidden: Vec<LineKey>,
    /// Rechecks spent waiting for the time scale, against [`UNMEASURED_RETRIES`]; reset once it is
    /// measured.
    unmeasured_retries: u8,
    /// The chart-label eye's choice for this pane's coin: `Some(shown)` once the reader pressed it,
    /// `None` while the pane follows the tab's switch (`docs-internal/AUTO_FIGURES.md` §13.1).
    ///
    /// Kept HERE, on the slots keyed by `(core, market)`, because that is exactly the lifetime
    /// LinKvo decided for it (19.11.2026): until the pane changes market, or the app restarts —
    /// the slots go with the market, and nothing here is ever written to disk. While it is set it
    /// wins over the popup's switch: the reader said it about THIS coin, after looking at it, and a
    /// tab-wide toggle flipped later says nothing about this one. See [`pane_shows`].
    eye: Option<bool>,
    /// Whether the engine holds a set published for this target, so a pane that stops showing
    /// takes its lines off once — and one that starts showing again republishes what its slots
    /// already hold, without waiting for a detector run that will not come.
    published: bool,
}

impl PaneSlots {
    /// Fresh slots for `target`: no rows evaluated, no eye pressed, nothing published.
    fn new(target: (CoreId, String)) -> Self {
        Self {
            target,
            rows: Vec::new(),
            hidden: Vec::new(),
            unmeasured_retries: 0,
            eye: None,
            published: false,
        }
    }
}

/// Whether a pane draws auto figures: the eye's choice where the reader made one, else the tab's
/// switch — and either way only while some row is drawn at all.
///
/// The eye overrides the switch, not the rows: "show" on a tab kind whose popup is OFF runs and
/// draws that kind's rows on this pane, but it cannot draw a row the popup has hidden.
fn pane_shows(cfg: &AutoFiguresCfg, eye: Option<bool>) -> bool {
    cfg.shows(eye)
}

/// Drop the slots of every pane target that is no longer on screen — every one of them on a frozen
/// trade window — and return the targets dropped, whose lines the caller takes off.
///
/// This is where "until the market changes" happens for everything a pane remembers about its
/// coin: the cached answers, the lines hidden from the menu, and the eye's override all live on
/// the slots, keyed by `(core, market)`, so a pane that moves to another market leaves them all
/// behind at once. A pane that is merely HIDDEN — by the tab's switch or by its eye — stays.
fn drop_gone_panes(
    panes: &mut Vec<PaneSlots>,
    on_screen: &[(usize, (CoreId, String))],
    historical: bool,
) -> Vec<(CoreId, String)> {
    let mut gone = Vec::new();
    panes.retain(|p| {
        let keep = !historical && on_screen.iter().any(|(_, t)| *t == p.target);
        if !keep {
            gone.push(p.target.clone());
        }
        keep
    });
    gone
}

/// Lower `wake` to `at` if `at` comes first: the wake timer serves the earliest moment of all.
fn earliest(wake: &mut Option<i64>, at: i64) {
    *wake = Some(wake.map_or(at, |w| w.min(at)));
}

/// What one evaluation of a pane's rows asks for.
#[derive(Default)]
struct RowPlan {
    /// The rows a detector job has to run, with the key each runs for.
    jobs: Vec<(usize, RowKey)>,
    /// The earliest Unix ms a key of this pane can move, for the wake timer.
    wake: Option<i64>,
}

/// Evaluate every row key of one pane and say which rows have to run.
///
/// A pane that does not show ([`pane_shows`]) evaluates NOTHING: no key, no job, no wake — its
/// slots are left exactly as they are, finished answers included, so showing it again on the same
/// bar finds them and runs only the rows whose key moved meanwhile. That is the whole contract of
/// the eye: hiding costs no detector, and showing again costs one only where the cache is stale.
///
/// Args:
///     cfg: The tab's auto-figure configuration.
///     pane: The pane's slots; rows are resized to `cfg` and their keys updated.
///     visible_ms: The pane's measured time span, `None` before the first market sync.
///     chart_ready: Whether the pane holds candles of the chart's own timeframe.
///     chart_tf_ms: The chart's bar length, ms — on a chart coarser than one minute an `Auto` row
///         takes M5 for M1 ([`tf::lift_m1`]); `<= 0` changes nothing.
///     now: The instant the dwell and the back-offs are measured on.
///     now_ms: The same moment as Unix ms, for the bar arithmetic.
///
/// Returns:
///     The jobs to start and the next wake.
fn plan_rows(
    cfg: &AutoFiguresCfg,
    pane: &mut PaneSlots,
    visible_ms: Option<i64>,
    chart_ready: bool,
    chart_tf_ms: i64,
    now: Instant,
    now_ms: f64,
) -> RowPlan {
    let mut plan = RowPlan::default();
    if !pane_shows(cfg, pane.eye) {
        return plan;
    }
    pane.rows.resize_with(cfg.rows.len(), RowSlot::default);
    let measured = visible_ms.filter(|v| *v > 0);
    let mut unmeasured_wait = false;
    for (ix, (row, slot)) in cfg.rows.iter().zip(pane.rows.iter_mut()).enumerate() {
        let Some(params) = row.detector.known().filter(|_| row.is_drawn()) else {
            // A hidden or unknown row keeps nothing: showing it again runs it afresh.
            *slot = RowSlot::default();
            continue;
        };
        let resolved = match row.tf {
            TfSource::Fixed(tf) => tf,
            TfSource::Auto => {
                // Before the pane's time scale is measured there is no span to choose from:
                // resolving one would read as the finest timeframe and start a job for it. Keep
                // whatever the row drew and ask again shortly.
                let Some(visible) = measured else {
                    unmeasured_wait = true;
                    continue;
                };
                let (tf, recheck) = slot.auto_tf.update(visible as f64, now);
                if let Some(at) = recheck {
                    earliest(
                        &mut plan.wake,
                        wake_ms(now_ms, at.saturating_duration_since(now)),
                    );
                }
                // No M1 on a coarser chart: its 1-minute bars are cached only where the market was
                // once shown on 1 minute.
                tf::lift_m1(tf, chart_tf_ms)
            }
        };
        let tf = resolved.min(params.coarsest_tf());
        let key = RowKey {
            detector: *params,
            tf,
            last_closed_ms: last_closed_open_ms(now_ms, tf),
            chart_ready,
        };
        slot.current = Some(key);
        earliest(&mut plan.wake, next_key_change_ms(now_ms, tf));
        if needs_run(slot, &key, now) {
            slot.pending = Some(key);
            plan.jobs.push((ix, key));
        } else if let Some(retry) = retry_due_ms(slot, now, now_ms) {
            earliest(&mut plan.wake, retry);
        }
    }
    if measured.is_some() {
        pane.unmeasured_retries = 0;
    } else if unmeasured_wait && pane.unmeasured_retries < UNMEASURED_RETRIES {
        pane.unmeasured_retries += 1;
        earliest(&mut plan.wake, wake_ms(now_ms, UNMEASURED_RECHECK));
    }
    plan
}

/// Whether a showing pane has to (re)publish on this evaluation: after a configuration edit, as
/// before, or when the engine holds nothing for it yet while its slots already have something to
/// draw — the eye just turned it back on, and the cached answers are the picture.
///
/// A fresh pane with nothing answered yet does not publish an empty set here: its first run's
/// completion publishes, and an empty publication would only cost a userdata rebuild.
fn needs_publish(pane: &PaneSlots, cfg_changed: bool) -> bool {
    cfg_changed || (!pane.published && pane.rows.iter().any(|s| drawable(s).is_some()))
}

/// One row of one pane.
#[derive(Default)]
struct RowSlot {
    /// `Auto`'s memory of the timeframe it settled on, for its hysteresis and dwell.
    auto_tf: DwellTf,
    /// The last accepted run. Its key is what "unchanged" is measured against.
    done: Option<Arc<AutoRowOutcome>>,
    /// The key of a run in flight.
    pending: Option<RowKey>,
    /// A rerun of THIS key is held off until the instant: a failed read's back-off, or a lagging
    /// bucket's recheck. Tied to the key so a newer bar never waits on an older one's back-off.
    retry: Option<(RowKey, Instant)>,
    /// Consecutive failed reads, indexing [`READ_RETRY`].
    failures: usize,
    /// The key the row asked for at its latest evaluation: what [`drawable`] checks `done`
    /// against before it is published.
    current: Option<RowKey>,
}

/// Whether a row with `key` has to run: not already answered FINALLY for it (a lagging answer is
/// not final), not already running for it, and not held off for it.
fn needs_run(slot: &RowSlot, key: &RowKey, now: Instant) -> bool {
    let final_answer = slot
        .done
        .as_ref()
        .is_some_and(|d| d.key == *key && !d.lagging);
    if final_answer || slot.pending.as_ref() == Some(key) {
        return false;
    }
    if slot.done.as_ref().is_some_and(|d| d.key == *key) && slot.retry.is_none() {
        // Lagging with no recheck armed cannot happen through `accept`; never spin on it.
        return false;
    }
    slot.retry
        .as_ref()
        .is_none_or(|(k, at)| k != key || now >= *at)
}

/// Take a finished run's outcome into `slot` if it answers the key still pending. Returns whether
/// the drawable answer changed.
///
/// A definitive answer (bars, or none because the market has no cache) is kept and clears the
/// failure count. A failed read keeps the previous answer and backs off along [`READ_RETRY`]. A
/// lagging answer is kept — it is the best there is — and rechecked after [`LAG_RECHECK`].
fn accept(slot: &mut RowSlot, outcome: AutoRowOutcome, now: Instant) -> bool {
    if slot.pending != Some(outcome.key) {
        return false;
    }
    slot.pending = None;
    if outcome.read_failed {
        let wait = READ_RETRY[slot.failures.min(READ_RETRY.len() - 1)];
        slot.failures = slot.failures.saturating_add(1);
        slot.retry = Some((outcome.key, now + wait));
        return false;
    }
    slot.failures = 0;
    slot.retry = outcome.lagging.then_some((outcome.key, now + LAG_RECHECK));
    slot.done = Some(Arc::new(outcome));
    true
}

/// Split `rows` into those a job has to run and the definitive empty answers of rows that would
/// read the cache of a market that has none (`has_cache == false`).
fn split_no_cache(
    rows: Vec<(usize, RowKey)>,
    chart: &[moon_core::market::ChartCandle],
    chart_tf_ms: i64,
    has_cache: bool,
) -> (Vec<(usize, RowKey)>, Vec<(usize, AutoRowOutcome)>) {
    if has_cache {
        return (rows, Vec::new());
    }
    let (tf_ms, history_ms) = job::chart_extent(chart, chart_tf_ms);
    let mut run = Vec::new();
    let mut resolved = Vec::new();
    for (ix, key) in rows {
        let (source, needed) = job::plan_row(&key, tf_ms, history_ms, false);
        if job::reads_cache(source) {
            resolved.push((ix, job::without_cache(key, source, needed)));
        } else {
            run.push((ix, key));
        }
    }
    (run, resolved)
}

/// When a held-off rerun is due, as Unix ms, for the wake timer; `None` while the row's own run is
/// in flight.
///
/// A pending run answers for itself: its completion re-arms whatever hold-off it earns. A retry
/// instant left over from an earlier answer has usually passed by then, and turned into a wake it
/// is a 0 ms timer whose sync finds the row still pending, arms the same past instant again, and
/// spins the UI thread for as long as the job runs.
fn retry_due_ms(slot: &RowSlot, now: Instant, now_ms: f64) -> Option<i64> {
    if slot.pending.is_some() {
        return None;
    }
    let (_, at) = slot.retry.as_ref()?;
    Some(wake_ms(now_ms, at.saturating_duration_since(now)))
}

/// The Unix ms a wake `after` from `now_ms` is armed for, rounded UP.
///
/// Rounded down, a wake could fire up to a millisecond before what it waits for — a dwell or a
/// back-off measured on `Instant` — find it not yet due, and re-arm for the same instant at 0 ms.
fn wake_ms(now_ms: f64, after: Duration) -> i64 {
    (now_ms + after.as_secs_f64() * 1000.0).ceil() as i64
}

/// The run outcome a row may publish: its last accepted answer, only while that answer is for the
/// row as it now stands — the same detector parameters and timeframe as its current key.
///
/// Not the whole key: `last_closed_ms` moves with every bar close, and the previous bar's lines are
/// the right thing to show until the new run returns. What must never be drawn is an answer
/// computed for different parameters — a row moved into this slot, or edited, still holding the
/// old answer, which would draw the previous row's lines in this row's style.
fn drawable(slot: &RowSlot) -> Option<&Arc<AutoRowOutcome>> {
    let current = slot.current.as_ref()?;
    slot.done
        .as_ref()
        .filter(|d| d.key.detector == current.detector && d.key.tf == current.tf)
}

/// A row's identity for carrying its slot across an edit of the row list: what its answer
/// depends on, apart from the clock.
fn row_identity(row: &AutoFigureRow) -> (&RowDetector, TfSource) {
    (&row.detector, row.tf)
}

/// Where each NEW row's slot comes from in the OLD list: the same position when the row there is
/// the same, else the first unused old slot of an identical row, else none (a fresh slot).
///
/// Slots hold per-row state by position — the last answer, a run in flight, the `Auto` dwell — and
/// the popup moves, removes and inserts rows; without this, after a swap each row would inherit
/// its neighbour's answer.
fn slot_map(old: &[AutoFigureRow], new: &[AutoFigureRow]) -> Vec<Option<usize>> {
    let mut used = vec![false; old.len()];
    let mut map: Vec<Option<usize>> = new
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let same = old
                .get(i)
                .is_some_and(|o| row_identity(o) == row_identity(row));
            if same {
                used[i] = true;
            }
            same.then_some(i)
        })
        .collect();
    for (i, row) in new.iter().enumerate() {
        if map[i].is_some() {
            continue;
        }
        if let Some(j) =
            (0..old.len()).find(|&j| !used[j] && row_identity(&old[j]) == row_identity(row))
        {
            used[j] = true;
            map[i] = Some(j);
        }
    }
    map
}

/// Whether `map` keeps every row where it was: same length and each slot in place.
fn keeps_positions(map: &[Option<usize>], old_len: usize) -> bool {
    map.len() == old_len && map.iter().enumerate().all(|(i, m)| *m == Some(i))
}

/// Re-order `slots` by `map` (from [`slot_map`]); unmatched rows start fresh.
fn realign(slots: &mut Vec<RowSlot>, map: &[Option<usize>]) {
    let mut old: Vec<Option<RowSlot>> = std::mem::take(slots).into_iter().map(Some).collect();
    *slots = map
        .iter()
        .map(|m| m.and_then(|j| old.get_mut(j)?.take()).unwrap_or_default())
        .collect();
}

impl ChartPanel {
    /// Evaluate every row key of every pane, start a job for the rows whose key moved, and keep
    /// the wake for the next candle close armed. Cheap and idempotent: called from every wake the
    /// panel already has.
    pub(super) fn sync_auto_figures(&mut self, cx: &mut Context<Self>) {
        if !self.scene_visible {
            return;
        }
        let now_ms = now_unix_ms();
        let now = Instant::now();
        let (cfg_changed, moved) = {
            let b = self.backend.read(cx);
            let cfg = b.layout.auto_figures_for(self.default_kind);
            if self.auto_figures.cfg.as_ref() != Some(cfg) {
                let moved = self.auto_figures.cfg.as_ref().map(|old| {
                    let map = slot_map(&old.rows, &cfg.rows);
                    (!keeps_positions(&map, old.rows.len())).then_some(map)
                });
                self.auto_figures.cfg = Some(cfg.clone());
                (true, moved.flatten())
            } else {
                (false, None)
            }
        };
        // Rows moved, came or went: every slot follows its row, and the hidden lines are dropped —
        // they name rows by position, and they are a passing choice, not a setting.
        if let Some(map) = moved {
            for pane in &mut self.auto_figures.panes {
                realign(&mut pane.rows, &map);
                pane.hidden.clear();
            }
        }
        let targets: Vec<(usize, (CoreId, String))> = (0..self.chart.pane_count())
            .filter_map(|idx| self.chart.pane_target(idx).map(|t| (idx, t)))
            .collect();

        // Panes that left — another market, or a closed slot — take their lines, their cached
        // answers and their eye with them. A frozen trade window describes a closed interval, and
        // lines computed from today's candles would describe a different picture, so it keeps
        // nothing at all.
        //
        // A pane the tab's switch (or its eye) hides is NOT dropped: its slots are what lets the
        // eye show it again without a detector run, and its eye is what must outlive a flip of the
        // popup's switch until the market changes.
        let gone = drop_gone_panes(&mut self.auto_figures.panes, &targets, self.historical);
        let mut republish = false;
        for target in &gone {
            republish |= self.chart.set_auto_figures(target.0, &target.1, None);
        }
        let cfg = match (self.historical, self.auto_figures.cfg.clone()) {
            (false, Some(cfg)) => cfg,
            _ => {
                self.auto_figures.timer = None;
                if republish {
                    self.resync_userdata(cx);
                }
                return;
            }
        };

        let mut next_wake: Option<i64> = None;
        for (idx, target) in targets {
            let Some(view) = self.chart.auto_pane_view(idx, target.0, &target.1) else {
                // Not stamped with this target yet; the next sync after the market sync will be.
                continue;
            };
            let found = self
                .auto_figures
                .panes
                .iter()
                .position(|p| p.target == target);
            let eye = found.and_then(|at| self.auto_figures.panes[at].eye);
            let shows = pane_shows(&cfg, eye);
            let at = match found {
                Some(at) => at,
                // Nothing to keep for a pane that neither shows nor was ever pressed: a tab with
                // the switch off walks its panes and allocates nothing.
                None if !shows => continue,
                None => {
                    self.auto_figures.panes.push(PaneSlots::new(target.clone()));
                    self.auto_figures.panes.len() - 1
                }
            };
            let pane = &mut self.auto_figures.panes[at];
            let plan = plan_rows(
                &cfg,
                pane,
                view.visible_ms,
                view.chart_ready,
                view.chart_tf_ms,
                now,
                now_ms,
            );
            if let Some(wake) = plan.wake {
                next_wake = Some(next_wake.map_or(wake, |w| w.min(wake)));
            }
            // A style or selection edit moves no key; the lines still have to follow it. A pane
            // that stopped showing takes its lines off once; one shown again by its eye puts back
            // what its slots already answered.
            let publish = match shows {
                true => needs_publish(pane, cfg_changed),
                false => pane.published,
            };
            if !plan.jobs.is_empty() {
                self.spawn_auto_job(idx, target.clone(), plan.jobs, view.chart_tf_ms, now_ms, cx);
            }
            if publish {
                republish |= self.publish_auto_figures(&target);
            }
        }
        if republish {
            self.resync_userdata(cx);
        }
        self.arm_auto_figures_timer(next_wake, now_ms, cx);
    }

    /// Whether the pane showing `target` draws auto figures right now — what the chart-label eye
    /// prints, open or crossed out.
    ///
    /// Read against the tab's configuration as the backend holds it rather than the copy the last
    /// sync applied, so the eye is right on the very first render of a pane, before any sync ran.
    pub(super) fn auto_figures_shown(
        &self,
        cfg: &AutoFiguresCfg,
        target: &(CoreId, String),
    ) -> bool {
        let eye = self
            .auto_figures
            .panes
            .iter()
            .find(|p| p.target == *target)
            .and_then(|p| p.eye);
        !self.historical && pane_shows(cfg, eye)
    }

    /// The chart-label eye was pressed on the pane showing `target`: flip what THIS pane draws,
    /// for this coin only (`docs-internal/AUTO_FIGURES.md` §13.1).
    ///
    /// Nothing is written — not the popup's configuration, not the disk — and nothing is sent to
    /// the core. The flip is stored on the pane's slots ([`PaneSlots::eye`]), and the ordinary
    /// sync does the rest by its own rules: a pane turned off evaluates no key and only takes its
    /// lines off; a pane turned on runs a detector only for the rows whose cached answer is not
    /// for the current key, and republishes the cached ones at once. The wake is this panel's own:
    /// the userdata rebuild for the one pane, and a notify of this panel so the button's own state
    /// follows the press — never the scene.
    ///
    /// A press for a market the pane no longer shows (the rectangles are a frame old) is dropped.
    pub(super) fn toggle_auto_figures_eye(
        &mut self,
        core: CoreId,
        market: String,
        cx: &mut Context<Self>,
    ) {
        let target = (core, market);
        let on_screen = (0..self.chart.pane_count())
            .any(|i| self.chart.pane_target(i).as_ref() == Some(&target));
        if self.historical || !on_screen {
            return;
        }
        let shown = {
            let b = self.backend.read(cx);
            self.auto_figures_shown(b.layout.auto_figures_for(self.default_kind), &target)
        };
        let at = match self
            .auto_figures
            .panes
            .iter()
            .position(|p| p.target == target)
        {
            Some(at) => at,
            None => {
                self.auto_figures.panes.push(PaneSlots::new(target.clone()));
                self.auto_figures.panes.len() - 1
            }
        };
        self.auto_figures.panes[at].eye = Some(!shown);
        self.sync_auto_figures(cx);
        cx.notify();
    }

    /// Start one background run for pane `idx`'s `rows`.
    fn spawn_auto_job(
        &mut self,
        idx: usize,
        target: (CoreId, String),
        rows: Vec<(usize, RowKey)>,
        chart_tf_ms: i64,
        now_ms: f64,
        cx: &mut Context<Self>,
    ) {
        let chart: Arc<[_]> = self
            .chart
            .auto_pane_candles(idx, target.0, &target.1, now_ms)
            .into();
        let source = self.backend.read(cx).session.market_source();
        let cache = source
            .kline_address(target.0, &target.1)
            .map(|(cache, exchange)| CacheAddr {
                cache,
                exchange,
                market: target.1.clone(),
            });
        // Once per job (a bar close), never per frame: the detectors' tolerance floor.
        let price_tick = source.price_tick(target.0, &target.1);
        // The diagnostics channel, asked once per job: one atomic load while it is off.
        let diag = moon_core::diagnostics::auto_figures_follows(&target.1).then(|| {
            let cfg_rows = self.auto_figures.cfg.as_ref().map(|c| c.rows.as_slice());
            job::diag::JobDiag {
                label: format!("{:?}/{}", target.0, target.1),
                takes: rows
                    .iter()
                    .filter_map(|&(ix, _)| cfg_rows?.get(ix).map(|row| (ix, row.take)))
                    .collect(),
            }
        });
        // A market with no cache address is a definitive "no cache", not a failed read: its cache
        // rows are answered here, empty and final — no job, no detector, no retry — and run again
        // only when their key moves.
        let (rows, no_cache) = split_no_cache(rows, &chart, chart_tf_ms, cache.is_some());
        if !no_cache.is_empty() {
            self.auto_job_done(target.clone(), no_cache, cx);
        }
        if rows.is_empty() {
            return;
        }
        let job = PaneJob {
            rows,
            chart_tf_ms,
            chart,
            cache,
            now_ms,
            price_tick,
            diag,
        };
        // Detached on purpose: replacing a handle would CANCEL a run whose rows are still marked
        // pending, and they would never be retried. A late result is recognised by its key.
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let outcomes = executor
                .spawn(async move {
                    let cache = job.cache.as_ref().map(|c| c as &dyn job::BarCache);
                    job::run(&job, cache)
                })
                .await;
            let _ = this.update(cx, |this, cx| this.auto_job_done(target, outcomes, cx));
        })
        .detach();
    }

    /// Accept a finished run's outcomes whose key is still the one asked for, then republish.
    fn auto_job_done(
        &mut self,
        target: (CoreId, String),
        outcomes: Vec<(usize, AutoRowOutcome)>,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self
            .auto_figures
            .panes
            .iter_mut()
            .find(|p| p.target == target)
        else {
            return;
        };
        let now = Instant::now();
        let mut accepted = false;
        let mut retry = false;
        for (ix, outcome) in outcomes {
            // By position first, else wherever the run's row moved while it ran (`realign`).
            let at = if pane
                .rows
                .get(ix)
                .is_some_and(|s| s.pending == Some(outcome.key))
            {
                Some(ix)
            } else {
                pane.rows
                    .iter()
                    .position(|s| s.pending == Some(outcome.key))
            };
            let Some(slot) = at.and_then(|at| pane.rows.get_mut(at)) else {
                continue;
            };
            // Only an outcome that arms a hold-off needs the wake re-armed; a definitive answer
            // (the synchronous no-cache path included) must not re-enter the key evaluation.
            let holds = outcome.read_failed || outcome.lagging;
            accepted |= accept(slot, outcome, now);
            retry |= holds;
        }
        if accepted && self.publish_auto_figures(&target) {
            self.resync_userdata(cx);
        }
        if retry {
            // Re-arms the timer for the back-off; the run itself starts when it expires.
            self.sync_auto_figures(cx);
        }
    }

    /// Hand one pane target's accepted outcomes to the engine, selected against its current price.
    ///
    /// A pane that does not show ([`pane_shows`]: the eye or the tab's switch says no) publishes
    /// NOTHING instead — its lines come off once, and its slots keep the answers for the next show.
    /// A run completing for such a pane therefore fills the cache and draws nothing.
    fn publish_auto_figures(&mut self, target: &(CoreId, String)) -> bool {
        let (Some(cfg), Some(pane)) = (
            self.auto_figures.cfg.as_ref(),
            self.auto_figures
                .panes
                .iter_mut()
                .find(|p| p.target == *target),
        ) else {
            return false;
        };
        if self.historical || !pane_shows(cfg, pane.eye) {
            return std::mem::take(&mut pane.published)
                && self.chart.set_auto_figures(target.0, &target.1, None);
        }
        pane.published = true;
        let pane = &*pane;
        let idx = (0..self.chart.pane_count())
            .find(|&i| self.chart.pane_target(i).as_ref() == Some(target));
        let last_price = idx
            .and_then(|i| self.chart.auto_pane_view(i, target.0, &target.1))
            .and_then(|v| v.last_price);
        let now_ms = now_unix_ms();
        let mut rows = Vec::new();
        let mut ref_prices = Vec::new();
        for (row_ix, (row, slot)) in cfg.rows.iter().zip(&pane.rows).enumerate() {
            let (Some(params), Some(outcome)) = (row.detector.known(), drawable(slot)) else {
                continue;
            };
            if !row.is_drawn() {
                continue;
            }
            ref_prices.push(last_price.or(outcome.last_close).unwrap_or(f64::NAN));
            rows.push(AutoRowDrawn {
                row_ix,
                title: RowTitle::of(row, params.kind()),
                style: row.style,
                caption: row.caption.clone(),
                take: row.take,
                marks: row.events,
                policy: params.broken_policy(),
                outcome: outcome.clone(),
                kept: Vec::new(),
                twins: Default::default(),
                ref_ms: now_ms,
            });
        }
        // Every row at once — each row's `take`, then a formation two rows keep drawn once, then the
        // confluences across them: the same passes the engine's price pass repeats on every price
        // change.
        let mut set = AutoPaneFigures::new(rows, pane.hidden.clone());
        set.opacity = cfg.opacity;
        set.select_all(&ref_prices, now_ms);
        // One line per publication — a run completing or a configuration edit, never a frame — so
        // the app log says which source each row read and whether its history was short.
        log::info!("auto figures {}: {}", target.1, set.summary());
        self.chart
            .set_auto_figures(target.0, &target.1, Some(Rc::new(set)))
    }

    /// Rebuild the userdata the published lines live in — the path news and figures take.
    fn resync_userdata(&mut self, cx: &mut Context<Self>) {
        let b = self.backend.read(cx);
        self.chart.sync_orders_if_visible(&b.session, true);
    }

    /// Keep a wake armed for `at_ms`, the earliest moment a row key can move.
    ///
    /// An already armed wake that fires no later is kept: when it fires it evaluates the keys and
    /// arms the next one. A later one is superseded by generation rather than cancelled — the
    /// timers are detached, because dropping a task handle from inside that task's own callback
    /// (which is where re-arming happens) is not something to rely on — and a superseded wake that
    /// fires does nothing.
    fn arm_auto_figures_timer(&mut self, at_ms: Option<i64>, now_ms: f64, cx: &mut Context<Self>) {
        let Some(at_ms) = at_ms else {
            self.auto_figures.timer = None;
            return;
        };
        if self
            .auto_figures
            .timer
            .is_some_and(|(armed, _)| armed <= at_ms)
        {
            return;
        }
        self.auto_figures.timer_gen = self.auto_figures.timer_gen.wrapping_add(1);
        let generation = self.auto_figures.timer_gen;
        self.auto_figures.timer = Some((at_ms, generation));
        // Rounded up, like the instants it is armed for (`wake_ms`): firing early only re-arms.
        let delay = Duration::from_millis((at_ms as f64 - now_ms).max(0.0).ceil() as u64);
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .auto_figures
                    .timer
                    .is_some_and(|(_, g)| g == generation)
                {
                    this.auto_figures.timer = None;
                    this.sync_auto_figures(cx);
                }
            });
        })
        .detach();
    }
}

mod menu;

#[cfg(test)]
mod tests;
