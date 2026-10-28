//! Regression checks for axis invalidation and settings signature equality.

use super::ChartSettingsSig;
use moon_core::config::{ChartTheme, OrdersStyleSet};

/// Construct independent input settings without a GPUI entity or running backend.
fn signature() -> ChartSettingsSig {
    ChartSettingsSig {
        theme: ChartTheme::default(),
        orders: OrdersStyleSet::default(),
        follow: false,
        live_scope: moon_core::config::LiveScope::default(),
        live_resume_gen: 0,
        chart_graphics: moon_core::config::ChartGraphicsCfg::default(),
        candle_view: moon_core::market::CandleViewCfg::default(),
        chart_labels: std::rc::Rc::new(moon_core::config::ChartLabelsCfg::default()),
        report_axis_rev: 7,
    }
}

/// Omitting the axis revision from equality leaves quiet charts on old report timestamps.
#[test]
fn axis_rev_move_changes_sig() {
    let prev = signature();
    let next = ChartSettingsSig {
        report_axis_rev: 8,
        ..signature()
    };
    assert!(prev != next);
}

/// Comparing freshly allocated label handles by identity would wake unchanged charts every tick.
#[test]
fn unchanged_inputs_equal_sig() {
    let prev = signature();
    let next = signature();
    assert!(prev == next);
}
