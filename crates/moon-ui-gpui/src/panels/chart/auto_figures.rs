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
use moon_chart::auto_figures::tf::AutoTf;
use moon_core::config::auto_figures::{AutoFiguresCfg, DetectorParams, TfId, TfSource};
use moon_core::session::CoreId;

use super::ChartPanel;
use crate::chartdx::auto_figures_sync::job::{
    self, AutoRowOutcome, CacheAddr, PaneJob, RowKey, last_closed_open_ms, next_key_change_ms,
};
use crate::chartdx::auto_figures_sync::select::select_kept;
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
        let (source, needed) = job::plan_row(&key, tf_ms, history_ms);
        if job::reads_cache(source) {
            resolved.push((ix, job::without_cache(key, source, needed)));
        } else {
            run.push((ix, key));
        }
    }
    (run, resolved)
}

/// When a held-off rerun is due, as Unix ms, for the wake timer.
fn retry_due_ms(slot: &RowSlot, now: Instant, now_ms: f64) -> Option<i64> {
    let (_, at) = slot.retry.as_ref()?;
    Some(now_ms as i64 + at.saturating_duration_since(now).as_millis() as i64)
}

/// The touch tolerance a row's zone band shows, in % of price; `0` for a detector without one.
fn tolerance_pct(params: &DetectorParams) -> f32 {
    match params {
        DetectorParams::Swing(p) => p.tolerance_pct,
        DetectorParams::Squeeze(p) => p.level.tolerance_pct,
        DetectorParams::Trend(p) => p.tolerance_pct,
        DetectorParams::PeriodExtreme(_) => 0.0,
    }
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
        let cfg_changed = {
            let b = self.backend.read(cx);
            let cfg = b.layout.auto_figures_for(self.default_kind);
            if self.auto_figures.cfg.as_ref() != Some(cfg) {
                self.auto_figures.cfg = Some(cfg.clone());
                true
            } else {
                false
            }
        };
        // A frozen trade window describes a closed interval; lines computed from today's candles
        // would describe a different picture.
        let active = !self.historical
            && self
                .auto_figures
                .cfg
                .as_ref()
                .is_some_and(AutoFiguresCfg::is_active);
        let targets: Vec<(usize, (CoreId, String))> = (0..self.chart.pane_count())
            .filter_map(|idx| self.chart.pane_target(idx).map(|t| (idx, t)))
            .collect();

        // Panes that left (or the whole feature switched off) take their lines with them.
        let gone: Vec<(CoreId, String)> = self
            .auto_figures
            .panes
            .iter()
            .filter(|p| !active || !targets.iter().any(|(_, t)| *t == p.target))
            .map(|p| p.target.clone())
            .collect();
        let mut republish = false;
        for target in &gone {
            self.auto_figures.panes.retain(|p| p.target != *target);
            republish |= self.chart.set_auto_figures(target.0, &target.1, None);
        }
        if !active {
            self.auto_figures.timer = None;
            if republish {
                self.resync_userdata(cx);
            }
            return;
        }
        let Some(cfg) = self.auto_figures.cfg.clone() else {
            return;
        };

        let mut next_wake: Option<i64> = None;
        for (idx, target) in targets {
            let Some(view) = self.chart.auto_pane_view(idx, target.0, &target.1) else {
                // Not stamped with this target yet; the next sync after the market sync will be.
                continue;
            };
            let at = match self
                .auto_figures
                .panes
                .iter()
                .position(|p| p.target == target)
            {
                Some(at) => at,
                None => {
                    self.auto_figures.panes.push(PaneSlots {
                        target: target.clone(),
                        rows: Vec::new(),
                        hidden: Vec::new(),
                        unmeasured_retries: 0,
                    });
                    self.auto_figures.panes.len() - 1
                }
            };
            let pane = &mut self.auto_figures.panes[at];
            pane.rows.resize_with(cfg.rows.len(), RowSlot::default);
            let measured = view.visible_ms.filter(|v| *v > 0);
            let mut unmeasured_wait = false;
            let mut jobs: Vec<(usize, RowKey)> = Vec::new();
            for (ix, (row, slot)) in cfg.rows.iter().zip(pane.rows.iter_mut()).enumerate() {
                let Some(params) = row.detector.known().filter(|_| row.is_drawn()) else {
                    // A hidden or unknown row keeps nothing: showing it again runs it afresh.
                    *slot = RowSlot::default();
                    continue;
                };
                let resolved = match row.tf {
                    TfSource::Fixed(tf) => tf,
                    TfSource::Auto => {
                        // Before the pane's time scale is measured there is no span to choose
                        // from: resolving one would read as the finest timeframe and start a job
                        // for it. Keep whatever the row drew and ask again shortly.
                        let Some(visible) = measured else {
                            unmeasured_wait = true;
                            continue;
                        };
                        let (tf, recheck) = slot.auto_tf.update(visible as f64, now);
                        if let Some(at) = recheck {
                            let wake = now_ms as i64
                                + at.saturating_duration_since(now).as_millis() as i64;
                            next_wake = Some(next_wake.map_or(wake, |w| w.min(wake)));
                        }
                        tf
                    }
                };
                let tf = resolved.min(params.coarsest_tf());
                let key = RowKey {
                    detector: *params,
                    tf,
                    last_closed_ms: last_closed_open_ms(now_ms, tf),
                    chart_ready: view.chart_ready,
                };
                let wake = next_key_change_ms(now_ms, tf);
                next_wake = Some(next_wake.map_or(wake, |w| w.min(wake)));
                if needs_run(slot, &key, now) {
                    slot.pending = Some(key);
                    jobs.push((ix, key));
                } else if let Some(retry) = retry_due_ms(slot, now, now_ms) {
                    next_wake = Some(next_wake.map_or(retry, |w| w.min(retry)));
                }
            }
            if measured.is_some() {
                pane.unmeasured_retries = 0;
            } else if unmeasured_wait && pane.unmeasured_retries < UNMEASURED_RETRIES {
                pane.unmeasured_retries += 1;
                let wake = now_ms as i64 + UNMEASURED_RECHECK.as_millis() as i64;
                next_wake = Some(next_wake.map_or(wake, |w| w.min(wake)));
            }
            if !jobs.is_empty() {
                self.spawn_auto_job(idx, target.clone(), jobs, view.chart_tf_ms, now_ms, cx);
            }
            if cfg_changed {
                // A style or selection edit moves no key; the lines still have to follow it.
                republish |= self.publish_auto_figures(&target);
            }
        }
        if republish {
            self.resync_userdata(cx);
        }
        self.arm_auto_figures_timer(next_wake, now_ms, cx);
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
        let cache = self
            .backend
            .read(cx)
            .session
            .market_source()
            .kline_address(target.0, &target.1)
            .map(|(cache, exchange)| CacheAddr {
                cache,
                exchange,
                market: target.1.clone(),
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
            let Some(slot) = pane.rows.get_mut(ix) else {
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
    fn publish_auto_figures(&mut self, target: &(CoreId, String)) -> bool {
        let (Some(cfg), Some(pane)) = (
            self.auto_figures.cfg.as_ref(),
            self.auto_figures.panes.iter().find(|p| p.target == *target),
        ) else {
            return false;
        };
        let idx = (0..self.chart.pane_count())
            .find(|&i| self.chart.pane_target(i).as_ref() == Some(target));
        let last_price = idx
            .and_then(|i| self.chart.auto_pane_view(i, target.0, &target.1))
            .and_then(|v| v.last_price);
        let now_ms = now_unix_ms();
        let mut scratch = Vec::new();
        let mut rows = Vec::new();
        for (row_ix, (row, slot)) in cfg.rows.iter().zip(&pane.rows).enumerate() {
            let (Some(params), Some(outcome)) = (row.detector.known(), slot.done.as_ref()) else {
                continue;
            };
            if !row.is_drawn() {
                continue;
            }
            let ref_price = last_price.or(outcome.last_close).unwrap_or(f64::NAN);
            let mut drawn = AutoRowDrawn {
                row_ix,
                title: RowTitle::of(row, params.kind()),
                style: row.style,
                caption: row.caption.clone(),
                take: row.take,
                marks: row.events,
                policy: params.broken_policy(),
                tolerance_pct: tolerance_pct(params),
                outcome: outcome.clone(),
                kept: Vec::new(),
                ref_ms: now_ms,
            };
            // The same selection the engine's price pass repeats on every price change.
            drawn.kept = select_kept(
                &drawn.outcome,
                &drawn.rules(&pane.hidden),
                ref_price,
                now_ms,
                &mut scratch,
            );
            rows.push(drawn);
        }
        let set = AutoPaneFigures::new(rows, pane.hidden.clone());
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
        let delay = Duration::from_millis((at_ms as f64 - now_ms).max(0.0) as u64);
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
