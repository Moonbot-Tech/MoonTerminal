//! The per-row status mark, and the name fit of a row line.

use std::sync::Arc;

use moon_chart::auto_figures::tf::BarSource;
use moon_core::config::auto_figures::{
    AutoFigurePreset, AutoFigureRow, DetectorKind, DetectorParams, SwingParams, TfId,
};

use super::rows::fitted_name;
use super::status::{RowStatus, merge};
use crate::chartdx::auto_figures_sync::job::{AutoRowOutcome, RowKey};

/// A complete run of a swing row on `source`, `bars` of `needed`.
fn outcome(source: BarSource, bars: usize, needed: u32) -> AutoRowOutcome {
    AutoRowOutcome {
        key: RowKey {
            detector: DetectorParams::Swing(SwingParams::default()),
            tf: TfId::H1,
            last_closed_ms: 0,
            chart_ready: true,
        },
        source,
        bars,
        needed,
        last_close: Some(100.0),
        read_failed: false,
        lagging: false,
        found: Vec::new(),
        recent: Vec::new(),
    }
}

/// Mutation: drop the zero-bars arm of `RowStatus::of` — an empty cache then reads as short
/// history, which tells the reader to wait for bars that will never come.
#[test]
fn an_empty_cache_read_is_no_cache_not_short_history() {
    assert_eq!(
        RowStatus::of(&outcome(BarSource::Cache5m, 0, 200)),
        Some(RowStatus::NoCache)
    );
    assert_eq!(
        RowStatus::of(&outcome(BarSource::ChartSeries, 0, 200)),
        Some(RowStatus::ShortHistory)
    );
    assert_eq!(
        RowStatus::of(&outcome(BarSource::Unavailable, 0, 200)),
        Some(RowStatus::Unavailable)
    );
}

/// Mutation: compare `bars <= needed`, or mark a complete lagging-free run.
#[test]
fn a_complete_run_carries_no_mark_and_a_lagging_one_does() {
    assert_eq!(RowStatus::of(&outcome(BarSource::Cache5m, 200, 200)), None);
    let mut lagging = outcome(BarSource::Cache5m, 200, 200);
    lagging.lagging = true;
    assert_eq!(RowStatus::of(&lagging), Some(RowStatus::Lagging));
}

/// One row shown on two panes reports the worse of the two; an outcome naming a row past the end
/// (published before a removal reached the panel) lands nowhere. Mutation: keep the FIRST status,
/// or index without the bound check.
#[test]
fn merge_keeps_the_worst_per_row_and_ignores_rows_past_the_end() {
    let mut lag = outcome(BarSource::Cache5m, 200, 200);
    lag.lagging = true;
    let marks = merge(
        [
            (0, Arc::new(lag.clone())),
            (0, Arc::new(outcome(BarSource::Cache5m, 0, 200))),
            (1, Arc::new(outcome(BarSource::ChartSeries, 200, 200))),
            (5, Arc::new(lag)),
        ],
        &[swing_row(), swing_row()],
    );
    assert_eq!(marks, vec![Some(RowStatus::NoCache), None]);
}

fn swing_row() -> AutoFigureRow {
    AutoFigureRow::new(DetectorParams::Swing(SwingParams::default()))
}

/// Breakage: a mark read by index alone. After the popup reorders rows, an outcome published for
/// the old order names the position, not the row, and the neighbour would inherit its "no cache".
#[test]
fn an_outcome_of_another_detector_marks_nothing() {
    let trend = AutoFigureRow::new(DetectorKind::Trend.default_params());
    let marks = merge(
        [(0, Arc::new(outcome(BarSource::Cache5m, 0, 200)))],
        &[trend],
    );
    assert_eq!(marks, vec![None]);
}

/// A name wider than the button is cut with an ellipsis and never painted past the border; one
/// that fits is left whole. Mutation: return the title unfitted.
#[test]
fn a_long_name_is_fitted_and_a_short_one_kept() {
    let measure = |s: &str| s.chars().count() as f32 * 7.0;
    let mut row = AutoFigurePreset::Levels1h.row();
    row.name = "Short".to_string();
    assert_eq!(fitted_name(&row, 100.0, measure), "Short");
    let long = AutoFigureRow {
        name: "A very long user name for a level row".to_string(),
        ..row
    };
    let fitted = fitted_name(&long, 100.0, measure);
    assert!(
        measure(&fitted) <= 100.0,
        "{fitted} is wider than the budget"
    );
    assert_ne!(fitted, long.name);
}

/// The popup's opacity steps hold the shipped opacity and both bounds the repair keeps, so a
/// profile that never touched the setting lights its own step, and every stored value lights the
/// step nearest it rather than a neighbour that misstates it.
///
/// Breakage: `AF_OPACITY_DEFAULT` moved off the steps (the popup then lights 40 % or 80 % for a
/// block drawn at 60 %), or a bound moved past the outermost step.
#[test]
fn the_opacity_steps_hold_the_default_and_the_bounds() {
    use moon_core::config::auto_figures::{AF_OPACITY_DEFAULT, AF_OPACITY_MAX, AF_OPACITY_MIN};
    let steps = super::OPACITY_STEPS;
    for v in [AF_OPACITY_DEFAULT, AF_OPACITY_MIN, AF_OPACITY_MAX] {
        let ix = crate::chart_tabs::graphics_popup::nearest(&steps, v);
        assert_eq!(steps[ix], v, "{v}");
    }
}
