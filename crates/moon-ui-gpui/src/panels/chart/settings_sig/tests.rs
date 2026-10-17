//! Regression checks for hidden settings deferral and signature equality.

use super::{ChartSettingsSig, settings_step};
use moon_core::config::{ChartTheme, OrdersStyleSet};

/// Construct independent input settings without a GPUI entity or running backend.
fn signature() -> ChartSettingsSig {
    ChartSettingsSig {
        order_book_width_px: moon_core::config::book_width::DEFAULT,
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

/// Requiring a new signature while hidden would restore backend-tick signature work.
#[test]
fn hidden_chart_skips_sig_and_marks_pending() {
    assert_eq!(
        settings_step(false, false, &signature(), None),
        (false, true)
    );
}

/// Dropping pending work or leaving it set would miss reveal settings or repeat their queries.
#[test]
fn becoming_visible_applies_pending_once() {
    let prev = signature();
    let next = signature();
    let (apply, pending) = settings_step(true, true, &prev, Some(&next));
    assert!(apply);
    assert!(!pending);
    assert_eq!(
        settings_step(true, pending, &next, Some(&next)),
        (false, false)
    );
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
    assert_eq!(
        settings_step(true, false, &prev, Some(&next)),
        (true, false)
    );
}

/// Comparing freshly allocated label handles by identity would wake unchanged charts every tick.
#[test]
fn unchanged_inputs_equal_sig() {
    let prev = signature();
    let next = signature();
    assert!(prev == next);
    assert_eq!(
        settings_step(true, false, &prev, Some(&next)),
        (false, false)
    );
}
