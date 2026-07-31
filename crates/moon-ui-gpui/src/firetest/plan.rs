//! The ordered stage plan: every phase the scenario can be in, its log name, and the two
//! scripted orders those phases run in.
//!
//! This module is the single answer to "what does a FireTest run consist of". `Phase` is the
//! state-machine tag, `stage_name` is what lands in `firetest.log`, and the two `*_STAGE_PLAN`
//! constants are what the sibling contract tests assert against so a new phase cannot be added
//! without deciding where in the run it belongs.

use super::config::Script;

/// One stage of a FireTest run.
///
/// `StageCount` is a sentinel, never a runtime phase: the contract tests read it as the phase
/// count so an unplanned phase fails the build instead of silently skipping the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    WaitStartup,
    WaitOpen,
    WaitProbe,
    Settle,
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
    Done,
    // Keep this last: cargo tests use it to catch unplanned FireTest phases.
    #[allow(dead_code)]
    StageCount,
}

/// The full `chart-smoke` run, in order. Asserted against `Phase` by the sibling tests.
#[cfg(test)]
pub(super) const STAGE_PLAN: [Phase; 24] = [
    Phase::WaitStartup,
    Phase::WaitOpen,
    Phase::WaitProbe,
    Phase::Settle,
    Phase::Baseline,
    Phase::Storm,
    Phase::StaticTextGap,
    Phase::StaticTextWarmup,
    Phase::StaticTextStorm,
    Phase::CommandErrorContract,
    Phase::ToolWindowsOpen,
    Phase::ToolWindowsVerifyOpen,
    Phase::ToolWindowsDedup,
    Phase::ToolWindowsVerifyDedup,
    Phase::RootOverlayContract,
    Phase::LocaleSwitch,
    Phase::LocaleSwitchVerify,
    Phase::PriceScale50,
    Phase::PriceScale20,
    Phase::PriceScaleAuto,
    Phase::PriceScaleVerifyAuto,
    Phase::OrderCancelLag,
    Phase::Cooldown,
    Phase::Done,
];

/// The narrow `order-cancel-lag` run: chart open plus the order path, nothing else.
#[cfg(test)]
pub(super) const ORDER_CANCEL_LAG_STAGE_PLAN: &[Phase] = &[
    Phase::WaitStartup,
    Phase::WaitOpen,
    Phase::WaitProbe,
    Phase::Settle,
    Phase::OrderCancelLag,
    Phase::Cooldown,
    Phase::Done,
];

impl Phase {
    /// The stage name written to `firetest.log`. Also the contract the docs and tests assert on.
    pub(super) fn stage_name(self) -> &'static str {
        match self {
            Phase::WaitStartup => "start",
            Phase::WaitOpen => "open_chart",
            Phase::WaitProbe => "wait_chart_probe",
            Phase::Settle => "settle_live_chart",
            Phase::Baseline => "baseline",
            Phase::Storm => "mouse_storm",
            Phase::StaticTextGap => "static_text_gap",
            Phase::StaticTextWarmup => "static_text_warmup",
            Phase::StaticTextStorm => "static_text_storm",
            Phase::CommandErrorContract => "command_error_contract",
            Phase::ToolWindowsOpen => "tool_windows_open",
            Phase::ToolWindowsVerifyOpen => "tool_windows_verify_open",
            Phase::ToolWindowsDedup => "tool_windows_dedup",
            Phase::ToolWindowsVerifyDedup => "tool_windows_verify_dedup",
            Phase::RootOverlayContract => "root_overlay_contract",
            Phase::LocaleSwitch => "locale_switch",
            Phase::LocaleSwitchVerify => "locale_switch_verify",
            Phase::PriceScale50 => "price_scale_50",
            Phase::PriceScale20 => "price_scale_20",
            Phase::PriceScaleAuto => "price_scale_auto",
            Phase::PriceScaleVerifyAuto => "price_scale_verify_auto",
            Phase::OrderCancelLag => "order_cancel_lag",
            Phase::Cooldown => "cooldown",
            Phase::Done => "result",
            Phase::StageCount => "__invalid_count",
        }
    }
}

/// Where a script goes once the live chart has settled: the full perf run, or straight to the
/// order path.
pub(super) fn phase_after_settle(script: Script) -> Phase {
    match script {
        Script::ChartSmoke => Phase::Baseline,
        Script::OrderCancelLag => Phase::OrderCancelLag,
    }
}
