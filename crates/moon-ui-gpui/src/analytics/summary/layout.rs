//! Layout hooks of the Summary tab: the debug selectors its layout probe measures.
//!
//! Every block names its boxes here, so the probe in `layout/tests.rs` and the renderers agree
//! on one spelling.

/// The whole KPI band.
pub(in crate::analytics) const KPI_BAND: &str = "sum-kpi-band";

/// One KPI group, 0..4 in drawing order.
pub(in crate::analytics) fn kpi_group(i: usize) -> String {
    format!("sum-kpi-g{i}")
}

/// A KPI metric's label box.
pub(in crate::analytics) fn kpi_label(name: &str) -> String {
    format!("sum-kpi-k{name}")
}

/// A KPI metric's value box.
pub(in crate::analytics) fn kpi_value(name: &str) -> String {
    format!("sum-kpi-v{name}")
}

/// A KPI metric's comparison, tail or muted caption line; `hero` is the Result group's.
pub(in crate::analytics) fn kpi_delta(name: &str) -> String {
    format!("sum-kpi-d{name}")
}

// Explicit imports in the probe, never `use super::*` through a `gpui::*` glob.
#[cfg(test)]
mod tests;
