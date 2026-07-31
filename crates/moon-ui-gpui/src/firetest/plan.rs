//! The stage table: what a run consists of, in one place, at runtime.
//!
//! This is the source of truth the dispatcher reads, not a description of it. Every property that
//! used to be spread across `tick` arms is a column here — how long the stage dwells before it
//! acts, what fails it, whether the chart is held in high-present mode while it runs, whether it
//! accepts a chart-bounds probe, and how much of its head is dropped from the samples. A stage's
//! successor is its position in the table, so inserting one never means editing its neighbour.
//!
//! Shaped after `crate::panels::registry`, which solved the same problem for dock panels: one row
//! per subject with `fn` pointers, instead of a stage's identity restated across a phase list, a
//! name function, a plan constant and a dispatcher arm that had to agree.

use std::time::Duration;

use gpui::Context;

use crate::Backend;

use super::Runtime;
use super::config::Script;
use super::stages;

// The run's clock. Every one of these is a cell in the table below, which is the only thing that
// reads them — a duration that lived anywhere else could not be weighed against its neighbours.

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
/// The idle window: how long the run sits with a live chart open and nothing touching it.
const IDLE_FLOOR: Duration = Duration::from_millis(5000);
/// Head of the idle window that is not sampled, so the last frames of the preceding forced
/// high-present mode do not count as idle work.
const IDLE_FLOOR_WARMUP: Duration = Duration::from_millis(1500);
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

/// One stage of a FireTest run.
///
/// The tag samples are recorded against, and the identity the contract tests enumerate.
/// `StageCount` is a sentinel, never a runtime stage: it gives the tests the variant count so a
/// phase nobody placed in a table fails the build instead of silently never running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    WaitStartup,
    WaitOpen,
    WaitProbe,
    Settle,
    IdleFloor,
    Baseline,
    Storm,
    StaticTextGap,
    StaticTextWarmup,
    StaticTextStorm,
    CommandErrorContract,
    ToolWindowsOpen,
    ToolWindowsVerifyOpen,
    ToolWindowsDedup,
    ToolWindowsVerifyDedup,
    RootOverlayContract,
    LocaleSwitch,
    LocaleSwitchVerify,
    PriceScale50,
    PriceScale20,
    PriceScaleAuto,
    PriceScaleVerifyAuto,
    OrderCancelLag,
    Cooldown,
    // Keep this last: cargo tests use it to catch unplanned FireTest phases.
    #[allow(dead_code)]
    StageCount,
}

/// The stage name written to `firetest.log` once the run is over.
///
/// There is deliberately no `Phase::Done`: "finished" is the cursor sitting past the last row, not
/// a state something has to remember to enter, so the only thing the end of a run still needs is
/// its name.
pub(super) const DONE_STAGE_NAME: &str = "result";

/// What a stage's act reports back to the dispatcher.
pub(super) enum StageStep {
    /// Not finished; call again next tick.
    Stay,
    /// Finished; advance to the next row.
    Next,
    /// Finished badly; fail the whole run with this reason.
    Fail(String),
}

impl From<Result<(), String>> for StageStep {
    /// The shape of a verification stage: it either passed and the run moves on, or it failed and
    /// the run stops with what it said. Stages that also have to WAIT build their step by hand —
    /// `Stay` is deliberately not reachable through this conversion.
    fn from(result: Result<(), String>) -> Self {
        match result {
            Ok(()) => StageStep::Next,
            Err(reason) => StageStep::Fail(reason),
        }
    }
}

/// What a stage does each tick, once its `min_dwell` has elapsed.
type Act = fn(&mut Runtime, &mut Backend, &mut Context<Backend>) -> StageStep;

/// One row of the table: a stage, and every rule the dispatcher applies to it.
///
/// Built with the chained `const fn`s below, so a row states only where it differs from the quiet
/// default — and a row that says nothing beyond its name and act is a stage that simply acts.
pub(super) struct StageDef {
    /// The tag samples taken during this stage carry.
    pub(super) phase: Phase,
    /// The stage name written to `firetest.log` on entry.
    pub(super) name: &'static str,
    /// The stage's work. Reached through [`StageDef::run`], never called as a bare field.
    act: Act,
    /// How long the stage sits still before it acts at all — the settling pause that lets it
    /// observe the previous stage's effect, or the measurement window it is timing.
    pub(super) min_dwell: Duration,
    /// How long after ENTERING the stage it may still be answering `Stay` before the run fails,
    /// and the reason it fails with. Measured from the same instant as `min_dwell`, so a row must
    /// keep this comfortably larger than its dwell. `None` for a stage its own clock already
    /// bounds — see the `a_stage_that_can_wait_forever_declares_a_timeout` test.
    pub(super) timeout: Option<(Duration, &'static str)>,
    /// Whether every live chart is held in forced high-present mode while this stage is current.
    /// Applied by the dispatcher on entry, so a stage never has to remember to set it.
    pub(super) present_pressure: bool,
    /// Whether a chart-bounds probe arriving during this stage is accepted. A probe from a later
    /// stage would retarget a storm that has already been aimed. Stated per row rather than
    /// derived from "is a storm running": which stages may aim the storm is a decision, and this
    /// project has been bitten before by reconstructing a decision from surrounding state.
    pub(super) wants_probe: bool,
    /// Leading slice of the stage whose samples are dropped, because they still carry the cost of
    /// entering the mode the stage is measuring. Only `baseline` needs one today; any stage
    /// measuring a floor it must first settle into will want its own.
    pub(super) sample_warmup: Duration,
}

impl StageDef {
    /// A row with the quiet defaults: acts immediately, no deadline, no present pressure, no
    /// probe, no warmup.
    const fn new(phase: Phase, name: &'static str, act: Act) -> Self {
        Self {
            phase,
            name,
            act,
            min_dwell: Duration::ZERO,
            timeout: None,
            present_pressure: false,
            wants_probe: false,
            sample_warmup: Duration::ZERO,
        }
    }

    /// Sit still for `dwell` before acting.
    const fn dwell(mut self, dwell: Duration) -> Self {
        self.min_dwell = dwell;
        self
    }

    /// Fail the run with `reason` if the stage is still waiting this long `after` entering it.
    const fn deadline(mut self, after: Duration, reason: &'static str) -> Self {
        self.timeout = Some((after, reason));
        self
    }

    /// Hold every live chart in forced high-present mode for the length of this stage.
    const fn hot(mut self) -> Self {
        self.present_pressure = true;
        self
    }

    /// Accept a chart-bounds probe arriving during this stage.
    const fn probes(mut self) -> Self {
        self.wants_probe = true;
        self
    }

    /// Drop this stage's first `warmup` of samples.
    const fn warmup(mut self, warmup: Duration) -> Self {
        self.sample_warmup = warmup;
        self
    }

    /// Run the stage for one tick.
    ///
    /// The act stays a private field reached only here, the way `panels::registry` keeps its
    /// `mk_docked`/`mk_detached` behind `build_docked`/`build_detached`.
    pub(super) fn run(
        &self,
        runtime: &mut Runtime,
        backend: &mut Backend,
        cx: &mut Context<Backend>,
    ) -> StageStep {
        (self.act)(runtime, backend, cx)
    }
}

// The rows shared by both scripts. Naming them once is what keeps the two tables from drifting
// apart on a stage neither script means to differ on.

/// Let the app finish starting before anything is asked of it.
const WAIT_STARTUP: StageDef =
    StageDef::new(Phase::WaitStartup, "start", |_, _, _| StageStep::Next).dwell(START_DELAY);

/// Put a real live chart on screen; every later stage measures against it.
const WAIT_OPEN: StageDef = StageDef::new(Phase::WaitOpen, "open_chart", stages::chart::open_chart)
    .deadline(OPEN_TIMEOUT, "no active visible core/window to open chart");

/// Learn the chart's real on-screen bounds — the storm has to be aimed at pixels, not guesses.
const WAIT_PROBE: StageDef = StageDef::new(
    Phase::WaitProbe,
    "wait_chart_probe",
    stages::chart::wait_probe,
)
.deadline(
    PROBE_TIMEOUT,
    "chart opened but no chart bounds probe arrived",
)
.probes();

/// Let the freshly opened chart reach its steady state before anything is measured.
const SETTLE_LIVE_CHART: StageDef = StageDef::new(Phase::Settle, "settle_live_chart", |_, _, _| {
    StageStep::Next
})
.dwell(SETTLE)
.hot()
.probes();

/// The opt-in real place/cancel measurement. Skipped — as a pass — unless it was asked for.
const ORDER_CANCEL_LAG: StageDef = StageDef::new(
    Phase::OrderCancelLag,
    "order_cancel_lag",
    stages::order_cancel::run,
)
.deadline(ORDER_CANCEL_TIMEOUT, "order_cancel_lag timed out");

/// The quiet tail, then the verdict. `evaluate_and_exit` ends the process on every path; the
/// `Fail` below is what would happen if it ever stopped doing so, because a run that reaches the
/// end and neither passes nor fails is worse than a loud one.
const COOLDOWN_STAGE: StageDef = StageDef::new(Phase::Cooldown, "cooldown", |runtime, _, _| {
    runtime.evaluate_and_exit();
    StageStep::Fail("verdict returned without ending the run".to_string())
})
.dwell(COOLDOWN);

/// The full `chart-smoke` run: the perf measurement, then every runtime contract, in order.
pub(super) const CHART_SMOKE: &[StageDef] = &[
    WAIT_STARTUP,
    WAIT_OPEN,
    WAIT_PROBE,
    SETTLE_LIVE_CHART,
    // Deliberately HERE, before anything is built: the static text layer has no disable path and
    // the tool windows are never closed, so an idle window placed after them would be measuring
    // idle plus 10k retained labels plus three open windows and calling the result a floor.
    //
    // Not `hot()`, which is the whole point — this is the only stage that measures the app when
    // nothing is forcing it to work. A live BTC feed still legitimately drives the chart's own
    // pass, so what this catches is the GPUI view path waking without input: a broadcast on every
    // tick, a panel repainting on a revision that did not change, a timer nobody needed.
    StageDef::new(Phase::IdleFloor, "idle_floor", stages::idle_floor::measure)
        .dwell(IDLE_FLOOR)
        .warmup(IDLE_FLOOR_WARMUP),
    StageDef::new(Phase::Baseline, "baseline", stages::perf::start_storm)
        .dwell(BASELINE)
        .hot()
        .probes()
        .warmup(BASELINE_WARMUP),
    StageDef::new(Phase::Storm, "mouse_storm", stages::perf::await_storm)
        .hot()
        .probes(),
    StageDef::new(
        Phase::StaticTextGap,
        "static_text_gap",
        stages::perf::attach_text_overlay,
    )
    .dwell(STAGE_GAP)
    .probes(),
    StageDef::new(
        Phase::StaticTextWarmup,
        "static_text_warmup",
        stages::perf::start_storm,
    )
    .dwell(TEXT_WARMUP)
    .hot()
    .probes(),
    StageDef::new(
        Phase::StaticTextStorm,
        "static_text_storm",
        stages::perf::await_storm,
    )
    .hot()
    .probes(),
    StageDef::new(
        Phase::CommandErrorContract,
        "command_error_contract",
        stages::command_error::verify,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::ToolWindowsOpen,
        "tool_windows_open",
        stages::tool_windows::request_open,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::ToolWindowsVerifyOpen,
        "tool_windows_verify_open",
        stages::tool_windows::verify_open_then_reopen,
    )
    .dwell(STAGE_GAP),
    StageDef::new(Phase::ToolWindowsDedup, "tool_windows_dedup", |_, _, _| {
        StageStep::Next
    })
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::ToolWindowsVerifyDedup,
        "tool_windows_verify_dedup",
        stages::tool_windows::verify_dedup,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::RootOverlayContract,
        "root_overlay_contract",
        stages::root_overlay::verify,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::LocaleSwitch,
        "locale_switch",
        stages::locale::request_switch,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::LocaleSwitchVerify,
        "locale_switch_verify",
        stages::locale::verify_then_restore,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::PriceScale50,
        "price_scale_50",
        stages::price_scale::request_50,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::PriceScale20,
        "price_scale_20",
        stages::price_scale::verify_50_then_request_20,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::PriceScaleAuto,
        "price_scale_auto",
        stages::price_scale::verify_20_then_request_auto,
    )
    .dwell(STAGE_GAP),
    StageDef::new(
        Phase::PriceScaleVerifyAuto,
        "price_scale_verify_auto",
        stages::price_scale::verify_auto,
    )
    .dwell(STAGE_GAP),
    ORDER_CANCEL_LAG,
    COOLDOWN_STAGE,
];

/// The narrow `order-cancel-lag` run: open a chart, drive the order path, nothing else. It exists
/// only for when the full run's mouse/text/window stages get in the way of isolating that path.
pub(super) const ORDER_CANCEL_LAG_PLAN: &[StageDef] = &[
    WAIT_STARTUP,
    WAIT_OPEN,
    WAIT_PROBE,
    SETTLE_LIVE_CHART,
    ORDER_CANCEL_LAG,
    COOLDOWN_STAGE,
];

/// The table a script runs.
pub(super) fn plan_for(script: Script) -> &'static [StageDef] {
    match script {
        Script::ChartSmoke => CHART_SMOKE,
        Script::OrderCancelLag => ORDER_CANCEL_LAG_PLAN,
    }
}
