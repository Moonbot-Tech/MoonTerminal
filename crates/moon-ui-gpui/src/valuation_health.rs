//! Presentation of valuation-worker health, shared by every surface that reports it.
//!
//! The worker publishes machine facts — which stage, which failure class, since when. Turning those
//! into UI content consistently applies three choices: the core's single stall threshold, its
//! whole-minute age rounding, and translated labels paired with raw diagnostic codes. The report
//! footer and the Analytics quote-split note both read this module, so they cannot disagree about
//! the same worker.

use moon_core::db::valuation::ValuationStatus;
use rust_i18n::t;

/// Everything a surface needs to state one stall, already resolved.
pub(crate) struct StallFacts {
    /// Localized stage name.
    pub stage: String,
    /// Localized failure-class name.
    pub kind: String,
    /// Machine identifiers, kept together for a user to quote into a bug report.
    pub codes: String,
    /// Whole minutes the run has lasted.
    pub minutes: i64,
    /// Free-text detail from the provider or SQLite, untranslated by nature.
    pub detail: String,
}

/// Resolve the stalled run worth reporting, if any.
///
/// Args:
///     status: Health published by the valuation worker.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     Facts for the longest-failing stalled stage, or `None` while none qualifies.
pub(crate) fn stall_facts(status: &ValuationStatus, now_ms: i64) -> Option<StallFacts> {
    let stalled = status.stalled(now_ms)?;
    let fault = stalled.fault.as_ref()?;
    let stage_code = fault.stage.code();
    let kind_code = fault.kind.code();
    Some(StallFacts {
        stage: t!(format!("valuation.stage.{stage_code}")).to_string(),
        kind: t!(format!("valuation.kind.{kind_code}")).to_string(),
        codes: format!("{stage_code}/{kind_code}"),
        minutes: stalled.failing_for_minutes(now_ms),
        detail: fault.detail.clone(),
    })
}
