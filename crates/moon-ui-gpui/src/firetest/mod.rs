//! Built-in diagnostic scenario runner.
//!
//! `moonterminal --debug-script chart-smoke` opens a chart, injects a short native mouse storm
//! over it and fails the process if cursor movement wakes expensive GPUI paths or burns CPU.
//!
//! The run is a table walk: [`plan`] holds one row per stage, [`Runtime::tick`] below reads the
//! current row and advances, and [`verdict`] scores the samples at the end. Nothing in this file
//! branches on which stage is current — if a rule starts needing a phase name here, it wants to be
//! a column in the table instead. That is what makes an inserted stage cheap: its successor is its
//! position, so no neighbour is edited and no arm is written here.
//!
//! What a new stage still costs, honestly: its `run` function in a module under [`stages`], that
//! module's line in `stages/mod.rs`, a `Phase` variant, its row in the table, its name in the
//! `tests.rs` oracle and in `docs/FIRETEST.md`, plus a `Runtime` field if it carries state across
//! ticks and a threshold in [`verdict`] if it is scored.
//!
//! FireTest deliberately drives the real app: a real chart on a real core, real OS mouse input,
//! real windows. Static architecture checks belong in `tests/theme_contract/`, never here.

mod bench;
mod config;
// Named `logging`, not `log`: a module called `log` here would shadow the `log` crate for every
// later edit in this file, and the shadowing error reads as a missing macro.
mod logging;
mod plan;
mod probe;
mod sample;
mod stages;
mod storm;
mod verdict;

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use gpui::Context;
use moon_core::config::Language;
use moon_core::metrics::MetricsSnapshot;

use crate::{Backend, diag};

pub(crate) use config::Config;
pub(crate) use probe::ChartProbe;

use logging::{firetest_error, firetest_info};
use plan::{DONE_STAGE_NAME, StageDef, StageStep, plan_for};
use sample::Sample;
use stages::order_cancel::OrderCancelRun;
use storm::MouseStorm;

/// The live state of one FireTest run.
///
/// The fields carry no visibility modifier on purpose: a private item of this module is already
/// visible to every module under it, which is exactly the stage modules that write them.
pub(crate) struct Runtime {
    config: Config,
    /// The table this run walks — the whole scenario, chosen once by script.
    plan: &'static [StageDef],
    /// Index into `plan`. Past the end means the run is over; there is no separate "done" flag,
    /// so nothing can report a stage the cursor disagrees with.
    cursor: usize,
    phase_since: Instant,
    /// When the first core session registered, which is when waiting for them to settle can begin.
    /// `None` until one appears — an empty session list at startup means "nobody has reported yet",
    /// not "nothing to wait for", and reading it as the latter skips the wait entirely.
    cores_seen_at: Option<Instant>,
    probe: Option<ChartProbe>,
    /// How much app the idle window was measured against. `None` until that stage records it.
    bench: Option<bench::BenchShape>,
    samples: Vec<Sample>,
    storm: Option<MouseStorm>,
    opened_group: Option<String>,
    tool_window_ids: Option<(String, String, String)>,
    locale_switch: Option<(Language, Language)>,
    order_cancel: Option<OrderCancelRun>,
    text_overlay_enabled: bool,
    present_pressure_enabled: bool,
    last_wait_log: Instant,
}

impl Runtime {
    /// Start a run: force diagnostics on regardless of build profile and announce the config.
    pub(crate) fn new(config: Config) -> Self {
        diag::force_enable();
        let now = Instant::now();
        let plan = plan_for(config.script);
        firetest_info(&format!(
            "[firetest] script={:?} market={} storm_ms={} mouse_hz={:.0} text_labels={} order_cancel_lag={}",
            config.script,
            config.market,
            config.storm.as_millis(),
            config.mouse_hz,
            config.text_labels,
            config.order_cancel_lag
        ));
        firetest_info(&format!(
            "[firetest] stage={}",
            plan.first().map_or(DONE_STAGE_NAME, |stage| stage.name)
        ));
        Self {
            config,
            plan,
            cursor: 0,
            phase_since: now,
            cores_seen_at: None,
            probe: None,
            bench: None,
            samples: Vec::new(),
            storm: None,
            opened_group: None,
            tool_window_ids: None,
            locale_switch: None,
            order_cancel: None,
            text_overlay_enabled: false,
            present_pressure_enabled: false,
            last_wait_log: now,
        }
    }

    /// Accept the chart's reported bounds, if the current stage declared it wants them.
    fn observe_probe(&mut self, probe: ChartProbe) {
        if self.current_stage().is_some_and(|stage| stage.wants_probe) {
            self.probe = Some(probe);
        }
    }

    /// Store one per-second diag sample against the current stage's phase.
    ///
    /// A stage's `sample_warmup` head is dropped on purpose: those samples still carry the cost of
    /// entering the mode the stage is measuring, and a baseline's PEAK is what every `*_delta`
    /// threshold is measured against. Past the last row there is nothing left to record.
    fn record_sample(
        &mut self,
        rates: &[diag::DiagRate],
        metrics: MetricsSnapshot,
        gpu_frame_ms: f64,
    ) {
        let Some(stage) = self.current_stage() else {
            return;
        };
        if self.phase_since.elapsed() < stage.sample_warmup {
            return;
        }
        self.samples.push(Sample {
            phase: stage.phase,
            rates: rates.to_vec(),
            metrics,
            gpu_frame_ms,
        });
    }

    /// Advance the run by one app tick.
    ///
    /// The whole state machine is this: read the current row, hold the chart in whatever present
    /// mode the row declares, wait out its dwell, fail it if it has overstayed, otherwise let it
    /// act. A row that answers `Next` hands over to the row after it in the table — no stage names
    /// its successor, so inserting one never means editing its neighbour.
    fn tick(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) {
        let Some(stage) = self.current_stage() else {
            return;
        };
        self.set_present_pressure(backend, stage.present_pressure);
        let elapsed = self.phase_since.elapsed();
        if elapsed < stage.min_dwell {
            return;
        }
        match stage.run(self, backend, cx) {
            // The deadline is judged only after the stage has had this tick's attempt. Ticks are
            // 100 ms apart, so checking first would fail a run whose core or probe arrived during
            // the very window the deadline expired in. Two deliberate consequences: a stage's own
            // `Fail` outranks its deadline, which reports what actually went wrong instead of
            // "timed out"; and the order stage gets one last attempt, which is the attempt that
            // cancels the order it placed rather than abandoning it.
            StageStep::Stay => {
                if let Some((timeout, reason)) = stage.timeout
                    && elapsed >= timeout
                {
                    self.fail(reason);
                }
            }
            StageStep::Next => self.advance(backend),
            StageStep::Fail(reason) => self.fail(&reason),
        }
    }

    /// The row the run is on, or `None` once it has passed the last one.
    ///
    /// The plan is `'static`, so this borrows the row rather than `self` and a stage can be handed
    /// `&mut Runtime` while the dispatcher still holds it.
    fn current_stage(&self) -> Option<&'static StageDef> {
        self.plan.get(self.cursor)
    }

    /// Write the current row's name — or the closing name, once past the last row — to the log.
    /// The one place a `stage=` line is produced, so no caller can invent a name of its own.
    fn log_stage(&self) {
        firetest_info(&format!(
            "[firetest] stage={}",
            self.current_stage().map_or(DONE_STAGE_NAME, |s| s.name)
        ));
    }

    /// Move to the next row, announce it, and put the chart into that row's present mode.
    ///
    /// The mode is applied here rather than only at the top of the next tick so no stage ever runs
    /// a tick under its predecessor's. Past the last row the run is over, which is the one stage
    /// line that is not a row's name.
    fn advance(&mut self, backend: &mut Backend) {
        // Clamped, not incremented: a stage's `run` holds `&mut Runtime` and may have ended the
        // run itself (`finish` parks the cursor at the end). Without this, answering `Next` after
        // that would step past the end and make `current_stage` disagree with "finished".
        self.cursor = (self.cursor + 1).min(self.plan.len());
        self.phase_since = Instant::now();
        self.log_stage();
        // Applied here as well as at the top of `tick`: `tick`'s call covers the very first row,
        // this one covers the handover, so no stage ever runs a tick in its predecessor's mode.
        if let Some(stage) = self.current_stage() {
            self.set_present_pressure(backend, stage.present_pressure);
        }
    }

    /// End the run: step past every remaining row so no further stage acts and no further sample
    /// is recorded, and write the closing stage line.
    fn finish(&mut self) {
        self.cursor = self.plan.len();
        self.phase_since = Instant::now();
        self.log_stage();
    }

    /// Log a "still waiting" line at most once a second, so a stuck stage is visible in the log
    /// without drowning it at tick rate.
    fn wait_log(&mut self, msg: &str) {
        if self.last_wait_log.elapsed() < Duration::from_millis(1000) {
            return;
        }
        self.last_wait_log = Instant::now();
        firetest_info(&format!("[firetest] {msg}"));
    }

    /// End the run as a failure: stop the storm so the cursor is released, then exit with code 2.
    fn fail(&mut self, reason: &str) {
        self.finish();
        self.stop_storm();
        firetest_error(&format!(
            "[firetest] result=FAIL FIRETEST FAIL reason={reason}"
        ));
        std::process::exit(2);
    }
}

/// Advance the run, if this process was launched with `--debug-script`.
///
/// The runtime is taken out of the backend for the duration of the tick, because every stage needs
/// `&mut Backend` while it runs.
pub(crate) fn tick_backend(backend: &mut Backend, cx: &mut Context<Backend>) {
    let Some(mut runtime) = backend.firetest.take() else {
        return;
    };
    runtime.tick(backend, cx);
    backend.firetest = Some(runtime);
}

/// Report the chart's real on-screen bounds, called from the chart element's render.
pub(crate) fn observe_chart_probe(backend: &mut Backend, probe: ChartProbe) {
    if let Some(runtime) = backend.firetest.as_mut() {
        runtime.observe_probe(probe);
    }
}

/// Hand one per-second diagnostic sample to the run.
///
/// `take_gpu_frame_ms` drains an accumulator, so it is called unconditionally: skipping it when no
/// run is active would leave the next reader with stale frames.
pub(crate) fn record_diag_sample(backend: &mut Backend, rates: &[diag::DiagRate]) {
    let metrics = backend.snap;
    let gpu_frame_ms = diag::take_gpu_frame_ms();
    if let Some(runtime) = backend.firetest.as_mut() {
        runtime.record_sample(rates, metrics, gpu_frame_ms);
    }
}
