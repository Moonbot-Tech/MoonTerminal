//! Stages `price_scale_50` … `price_scale_verify_auto`: a toolbar scale choice reaches the active
//! chart's state.
//!
//! The request is written the way the toolbar writes it, and the check reads the resulting chart
//! spec back — not the field that was just set. Two fixed steps plus `Auto` catch both a scale that
//! never arrives and one that arrives but cannot be cleared.

use gpui::Context;
use moon_core::config::ChartBucket;

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;

impl Runtime {
    /// Write a scale request for the opened group, as the chart toolbar does.
    pub(in crate::firetest) fn request_price_scale(
        &self,
        backend: &mut Backend,
        scale: Option<f32>,
        cx: &mut Context<Backend>,
    ) {
        backend.price_scale = scale;
        backend.price_scale_group = self.opened_group.clone();
        backend.price_scale_rev = backend.price_scale_rev.wrapping_add(1);
        firetest_info(&format!(
            "[firetest] price_scale_request value={}",
            scale_label(scale)
        ));
        cx.notify();
    }

    /// Require that the main shared chart of the opened group now carries `expected`.
    pub(in crate::firetest) fn verify_price_scale(
        &self,
        backend: &Backend,
        expected: Option<f32>,
    ) -> Result<(), String> {
        let group = self
            .opened_group
            .as_ref()
            .ok_or_else(|| "price scale contract has no opened chart group".to_string())?;
        let actual = backend
            .chart_specs
            .iter()
            .find(|spec| {
                spec.group == *group && spec.num == 0 && spec.bucket() == ChartBucket::Shared
            })
            .and_then(|spec| spec.scale);
        if actual != expected {
            return Err(format!(
                "price scale did not reach active chart: expected {}, got {}",
                scale_label(expected),
                scale_label(actual)
            ));
        }
        firetest_info(&format!(
            "[firetest] price_scale_verify group={group} value={}",
            scale_label(actual)
        ));
        Ok(())
    }
}

/// The toolbar label for a scale value, used in the stage log lines.
fn scale_label(scale: Option<f32>) -> &'static str {
    if scale.is_none() {
        "Auto"
    } else if scale == Some(0.50) {
        "50%"
    } else if scale == Some(0.20) {
        "20%"
    } else if scale == Some(0.10) {
        "10%"
    } else if scale == Some(0.05) {
        "5%"
    } else if scale == Some(0.02) {
        "2%"
    } else {
        "custom"
    }
}
