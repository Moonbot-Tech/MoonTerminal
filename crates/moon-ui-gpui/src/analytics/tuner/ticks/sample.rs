//! Visible context for the population used by the comparison card.

use moon_core::db::tuner::VarStats;
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// Keep the sample neutral until the shown scope's tape read answers.
pub(super) fn caption(n: usize, total: i64, reading: bool) -> String {
    if reading {
        t!("analytics.ticks.fetch_reading").to_string()
    } else {
        t!("analytics.ticks.kpi.sample", n = n, total = total).to_string()
    }
}

/// Warn in either direction when displayed PFs differ by at least 0.20 absolute points.
/// The comparison uses rounded hundredths, matching the two figures the user reads.
pub(super) fn bias_note(fit: &VarStats, all: &VarStats) -> Option<String> {
    if fit.n == 0 || fit.n >= all.n || !fit.pf.is_finite() || !all.pf.is_finite() {
        return None;
    }
    let fit_pf = (fit.pf * 100.0).round();
    let all_pf = (all.pf * 100.0).round();
    if (fit_pf - all_pf).abs() < 20.0 {
        return None;
    }
    Some(
        t!(
            if fit_pf < all_pf {
                "analytics.ticks.kpi.bias_worse"
            } else {
                "analytics.ticks.kpi.bias_better"
            },
            fit = format!("{:.2}", fit.pf),
            all = format!("{:.2}", all.pf)
        )
        .to_string(),
    )
}
