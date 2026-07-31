//! Built-in diagnostic scenario runner.
//!
//! `moonterminal --debug-script chart-smoke` opens a chart, injects a short native mouse storm
//! over it and fails the process if cursor movement wakes expensive GPUI paths or burns CPU.
//!
//! The run is one ordered state machine and nothing else: [`plan::Phase`] is the list of stages,
//! [`Runtime::tick`] below is the only place that advances between them, and [`verdict`] scores the
//! samples at the end. Everything one stage needs lives in its own module under [`stages`], so its
//! logic is a file of its own rather than another slice of a single long file. Placing that stage
//! in the run is deliberately still four explicit edits — a `Phase` variant, its `stage_name`, its
//! entry in `STAGE_PLAN`, and a `tick` arm here — because the contract tests read the plan and the
//! names, and a stage that nobody decided where to put is exactly what they exist to catch.
//!
//! FireTest deliberately drives the real app: a real chart on a real core, real OS mouse input,
//! real windows. Static architecture checks belong in `tests/theme_contract/`, never here.

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
use plan::{Phase, phase_after_settle};
use sample::Sample;
use stages::order_cancel::OrderCancelRun;
use storm::MouseStorm;

// Every timing below paces the dispatcher, so they all live beside it. A stage module holds its
// stage's logic and types, not the clock the run advances on.
/// Grace period before the run touches the app, so startup finishes first.
const START_DELAY: Duration = Duration::from_millis(1000);
/// How long the freshly opened live chart is left alone before measuring anything.
const SETTLE: Duration = Duration::from_millis(5000);
/// Length of the cursor-free high-present baseline the storm is compared against.
const BASELINE: Duration = Duration::from_millis(5000);
/// Leading part of the baseline that is NOT sampled, so its first hot frames do not raise the
/// ceiling the storm is measured against.
const BASELINE_WARMUP: Duration = Duration::from_millis(1500);
/// Quiet tail before the verdict, letting the last present and the last samples land.
const COOLDOWN: Duration = Duration::from_millis(1200);
/// Timeout for finding an active visible core/window to open the chart on.
const OPEN_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Timeout for the chart reporting its real on-screen bounds after it opened.
const PROBE_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Settling pause between the short contract stages, so each observes the previous one's effect.
const STAGE_GAP: Duration = Duration::from_millis(200);
/// How long the static text layer bakes before the second storm starts.
const TEXT_WARMUP: Duration = Duration::from_millis(2500);
/// How long the whole place/cancel/observe chain may take before the order stage gives up.
const ORDER_CANCEL_TIMEOUT: Duration = Duration::from_millis(15_000);

/// The live state of one FireTest run.
///
/// The fields carry no visibility modifier on purpose: a private item of this module is already
/// visible to every module under it, which is exactly the stage modules that write them.
pub(crate) struct Runtime {
    config: Config,
    started: Instant,
    phase: Phase,
    phase_since: Instant,
    probe: Option<ChartProbe>,
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
        firetest_info(&format!(
            "[firetest] script={:?} market={} storm_ms={} mouse_hz={:.0} text_labels={} order_cancel_lag={}",
            config.script,
            config.market,
            config.storm.as_millis(),
            config.mouse_hz,
            config.text_labels,
            config.order_cancel_lag
        ));
        firetest_info("[firetest] stage=start");
        Self {
            config,
            started: now,
            phase: Phase::WaitStartup,
            phase_since: now,
            probe: None,
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

    /// Enter `phase`, restart its clock, and write its stage line to `firetest.log`.
    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_since = Instant::now();
        firetest_info(&format!("[firetest] stage={}", phase.stage_name()));
    }

    /// Accept the chart's reported bounds, but only while a stage still needs them: a probe from a
    /// later stage would retarget a storm that has already been aimed.
    fn observe_probe(&mut self, probe: ChartProbe) {
        if matches!(
            self.phase,
            Phase::WaitProbe
                | Phase::Settle
                | Phase::Baseline
                | Phase::Storm
                | Phase::StaticTextGap
                | Phase::StaticTextWarmup
                | Phase::StaticTextStorm
        ) {
            self.probe = Some(probe);
        }
    }

    /// Store one per-second diag sample against the current phase.
    ///
    /// The first `BASELINE_WARMUP` of the baseline is dropped on purpose: those samples still carry
    /// the cost of entering high-present mode, and the baseline's PEAK is what every `*_delta`
    /// threshold is measured against.
    fn record_sample(
        &mut self,
        rates: &[diag::DiagRate],
        metrics: MetricsSnapshot,
        gpu_frame_ms: f64,
    ) {
        if self.phase == Phase::Done {
            return;
        }
        if self.phase == Phase::Baseline && self.phase_since.elapsed() < BASELINE_WARMUP {
            return;
        }
        self.samples.push(Sample {
            phase: self.phase,
            rates: rates.to_vec(),
            metrics,
            gpu_frame_ms,
        });
    }

    /// Advance the state machine by one app tick.
    ///
    /// Each arm is only the transition rule for its phase; the work itself lives in the matching
    /// module under [`stages`].
    fn tick(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) {
        match self.phase {
            Phase::WaitStartup => {
                if self.started.elapsed() >= START_DELAY {
                    self.set_phase(Phase::WaitOpen);
                }
            }
            Phase::WaitOpen => {
                if self.try_open_chart(backend, cx) {
                    self.set_phase(Phase::WaitProbe);
                } else if self.phase_since.elapsed() >= OPEN_TIMEOUT {
                    self.fail("no active visible core/window to open chart");
                } else {
                    self.wait_log("waiting for active visible core/window");
                }
            }
            Phase::WaitProbe => {
                if self.probe.is_some() {
                    self.set_phase(Phase::Settle);
                } else if self.phase_since.elapsed() >= PROBE_TIMEOUT {
                    self.fail("chart opened but no chart bounds probe arrived");
                } else {
                    self.wait_log("waiting for chart bounds probe");
                }
            }
            Phase::Settle => {
                self.set_present_pressure(backend, true);
                if self.phase_since.elapsed() >= SETTLE {
                    let next_phase = phase_after_settle(self.config.script);
                    if self.config.is_order_cancel_script() {
                        self.set_present_pressure(backend, false);
                    }
                    self.set_phase(next_phase);
                }
            }
            Phase::Baseline => {
                self.set_present_pressure(backend, true);
                if self.phase_since.elapsed() >= BASELINE {
                    match self.start_mouse_storm() {
                        Ok(storm) => {
                            self.storm = Some(storm);
                            self.set_phase(Phase::Storm);
                        }
                        Err(err) => self.fail(&err),
                    }
                }
            }
            Phase::Storm => {
                self.set_present_pressure(backend, true);
                let done = self.storm.as_ref().is_some_and(MouseStorm::is_done);
                if done || self.phase_since.elapsed() >= self.config.storm {
                    self.stop_storm();
                    self.set_phase(Phase::StaticTextGap);
                }
            }
            Phase::StaticTextGap => {
                self.set_present_pressure(backend, false);
                if self.phase_since.elapsed() >= STAGE_GAP {
                    let text_applied = self.enable_text_overlay(backend, cx);
                    if text_applied == 0 {
                        self.fail("chart opened but static text stress overlay did not attach");
                        return;
                    }
                    self.set_phase(Phase::StaticTextWarmup);
                }
            }
            Phase::StaticTextWarmup => {
                self.set_present_pressure(backend, true);
                if self.phase_since.elapsed() >= TEXT_WARMUP {
                    match self.start_mouse_storm() {
                        Ok(storm) => {
                            self.storm = Some(storm);
                            self.set_phase(Phase::StaticTextStorm);
                        }
                        Err(err) => self.fail(&err),
                    }
                }
            }
            Phase::StaticTextStorm => {
                self.set_present_pressure(backend, true);
                let done = self.storm.as_ref().is_some_and(MouseStorm::is_done);
                if done || self.phase_since.elapsed() >= self.config.storm {
                    self.stop_storm();
                    self.set_present_pressure(backend, false);
                    self.set_phase(Phase::CommandErrorContract);
                }
            }
            Phase::CommandErrorContract => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_command_error_contract(backend) {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::ToolWindowsOpen);
                    }
                }
            }
            Phase::ToolWindowsOpen => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    self.request_tool_windows_open(cx);
                    self.set_phase(Phase::ToolWindowsVerifyOpen);
                }
            }
            Phase::ToolWindowsVerifyOpen => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_tool_windows_open(backend) {
                        self.fail(&error);
                    } else {
                        self.request_tool_windows_open(cx);
                        self.set_phase(Phase::ToolWindowsDedup);
                    }
                }
            }
            Phase::ToolWindowsDedup => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    self.set_phase(Phase::ToolWindowsVerifyDedup);
                }
            }
            Phase::ToolWindowsVerifyDedup => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_tool_windows_dedup(backend) {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::RootOverlayContract);
                    }
                }
            }
            Phase::RootOverlayContract => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_root_overlay_contract(backend, cx) {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::LocaleSwitch);
                    }
                }
            }
            Phase::LocaleSwitch => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    self.request_locale_switch(backend, cx);
                    self.set_phase(Phase::LocaleSwitchVerify);
                }
            }
            Phase::LocaleSwitchVerify => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    let result = self.verify_locale_switch(backend);
                    self.restore_locale(backend, cx);
                    if let Err(error) = result {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::PriceScale50);
                    }
                }
            }
            Phase::PriceScale50 => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    self.request_price_scale(backend, Some(0.50), cx);
                    self.set_phase(Phase::PriceScale20);
                }
            }
            Phase::PriceScale20 => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_price_scale(backend, Some(0.50)) {
                        self.fail(&error);
                    } else {
                        self.request_price_scale(backend, Some(0.20), cx);
                        self.set_phase(Phase::PriceScaleAuto);
                    }
                }
            }
            Phase::PriceScaleAuto => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_price_scale(backend, Some(0.20)) {
                        self.fail(&error);
                    } else {
                        self.request_price_scale(backend, None, cx);
                        self.set_phase(Phase::PriceScaleVerifyAuto);
                    }
                }
            }
            Phase::PriceScaleVerifyAuto => {
                if self.phase_since.elapsed() >= STAGE_GAP {
                    if let Err(error) = self.verify_price_scale(backend, None) {
                        self.fail(&error);
                    } else {
                        self.set_phase(Phase::OrderCancelLag);
                    }
                }
            }
            Phase::OrderCancelLag => {
                if self.phase_since.elapsed() >= ORDER_CANCEL_TIMEOUT {
                    self.fail("order_cancel_lag timed out");
                    return;
                }
                match self.tick_order_cancel_lag(backend) {
                    Ok(true) => self.set_phase(Phase::Cooldown),
                    Ok(false) => {}
                    Err(error) => self.fail(&error),
                }
            }
            Phase::Cooldown => {
                self.set_present_pressure(backend, false);
                if self.phase_since.elapsed() >= COOLDOWN {
                    self.evaluate_and_exit();
                }
            }
            Phase::Done => {}
            Phase::StageCount => {
                unreachable!("firetest phase count sentinel is not a runtime phase")
            }
        }
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
        self.set_phase(Phase::Done);
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
