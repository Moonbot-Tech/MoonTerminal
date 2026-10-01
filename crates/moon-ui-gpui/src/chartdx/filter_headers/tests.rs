//! Header-target regressions independent of a GPU device.

use super::{ColumnBand, FilterHeaderHit};
use moon_core::config::ChartLabelsCfg;
use std::rc::Rc;

/// Growing the hit target to the whole module consumes filter-body clicks; accepting a changed
/// configuration lets a stale row index toggle a different module after a profile edit.
#[test]
fn header_hit_is_bounded_and_rejects_a_replaced_profile() {
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = moon_core::config::strategy_filters_row();
    let hit = FilterHeaderHit {
        rect: [30.0, 50.0, 80.0, 20.0],
        row: 0,
        cfg: Rc::new(cfg.clone()),
    };
    assert!(hit.contains(30.0, 50.0, &cfg));
    assert!(hit.contains(110.0, 70.0, &cfg));
    assert!(!hit.contains(29.0, 50.0, &cfg));
    assert!(!hit.contains(50.0, 71.0, &cfg));
    cfg.rows.swap(0, 1);
    assert!(!hit.contains(50.0, 60.0, &cfg));
}

/// `ColumnBand::grow` letting a zero-sized run open or stretch a band puts the wheel zone over
/// empty chart (the wheel stops panning there); failing to union the runs of one row shrinks the
/// zone to the first line so the rest of the column no longer scrolls.
#[test]
fn a_column_band_is_the_union_of_its_drawn_runs_and_ignores_empty_ones() {
    let mut bands = Vec::new();
    ColumnBand::grow(&mut bands, 0, [0.0, 0.0, 0.0, 10.0]);
    ColumnBand::grow(&mut bands, 0, [0.0, 0.0, 10.0, 0.0]);
    assert!(bands.is_empty(), "a zero-size run opens no band");

    ColumnBand::grow(&mut bands, 0, [10.0, 20.0, 40.0, 12.0]);
    ColumnBand::grow(&mut bands, 0, [12.0, 32.0, 60.0, 12.0]);
    ColumnBand::grow(&mut bands, 0, [0.0, 0.0, 0.0, 0.0]);
    ColumnBand::grow(&mut bands, 1, [200.0, 20.0, 30.0, 12.0]);
    assert_eq!(bands.len(), 2);
    assert_eq!(bands[0].rect, [10.0, 20.0, 62.0, 24.0]);
    assert_eq!(bands[0].row, 0);
    assert_eq!(bands[1].rect, [200.0, 20.0, 30.0, 12.0]);
}

/// `ColumnBand::contains` turning an inclusive edge exclusive (or the reverse) moves the wheel
/// zone one pixel off the drawn column: the last line stops scrolling or the chart under the edge
/// stops panning. Edges are inclusive on all four sides, one pixel past is outside.
#[test]
fn a_column_band_includes_its_edges_and_nothing_past_them() {
    let band = ColumnBand {
        rect: [10.0, 20.0, 40.0, 30.0],
        row: 3,
    };
    for (x, y) in [
        (10.0, 20.0),
        (50.0, 50.0),
        (10.0, 50.0),
        (50.0, 20.0),
        (30.0, 35.0),
    ] {
        assert!(band.contains(x, y), "({x}, {y}) is on or inside the band");
    }
    for (x, y) in [(9.0, 30.0), (51.0, 30.0), (30.0, 19.0), (30.0, 51.0)] {
        assert!(!band.contains(x, y), "({x}, {y}) is outside the band");
    }
}

fn filters_cfg() -> ChartLabelsCfg {
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = moon_core::config::strategy_filters_row();
    cfg
}

/// Issue #831: with the list expanded, the wheel anywhere in the plot's left strip scrolls the
/// strategy-filter column, as in MoonBot. Dropping the strip puts the wheel back on the glyphs
/// alone, so a pointer beside the list pans the chart.
#[test]
fn an_expanded_filter_list_takes_the_wheel_across_the_left_strip() {
    let bands = vec![ColumnBand {
        rect: [20.0, 40.0, 80.0, 60.0],
        row: 0,
    }];
    let plot = [10.0, 30.0, 900.0, 500.0];
    let strip = ColumnBand::filter_strip(&bands, &filters_cfg(), plot).expect("expanded list");
    assert_eq!(strip.rect, [10.0, 30.0, 300.0, 470.0]);
    // Beside the glyphs, below the list, at the strip's far edge and at the plot's bottom.
    for (x, y) in [(200.0, 60.0), (30.0, 450.0), (310.0, 30.0), (10.0, 500.0)] {
        assert_eq!(
            ColumnBand::wheel_row_at(&bands, Some(&strip), x, y),
            Some(0),
            "({x}, {y}) is inside the strip"
        );
    }
    // Past the strip the wheel keeps panning time.
    for (x, y) in [(311.0, 60.0), (600.0, 300.0), (200.0, 501.0)] {
        assert_eq!(ColumnBand::wheel_row_at(&bands, Some(&strip), x, y), None);
    }
}

/// A collapsed list draws no lines and so keeps no band: opening a strip anyway would make the
/// wheel over the left of the chart stop panning while nothing on screen can scroll.
#[test]
fn a_collapsed_filter_list_opens_no_strip() {
    let plot = [10.0, 30.0, 900.0, 500.0];
    assert!(ColumnBand::filter_strip(&[], &filters_cfg(), plot).is_none());
    assert_eq!(ColumnBand::wheel_row_at(&[], None, 100.0, 100.0), None);
}

/// Only the strategy-filter column takes the strip: a band from another row (the arbitrage
/// column) keeps its glyph target, and a plot narrower than the strip caps it at the plot.
#[test]
fn the_strip_belongs_to_the_filter_row_and_never_leaves_the_plot() {
    let arb = vec![ColumnBand {
        rect: [20.0, 40.0, 80.0, 60.0],
        row: 1,
    }];
    let plot = [10.0, 30.0, 900.0, 500.0];
    assert!(ColumnBand::filter_strip(&arb, &filters_cfg(), plot).is_none());

    let bands = vec![ColumnBand {
        rect: [20.0, 40.0, 80.0, 60.0],
        row: 0,
    }];
    let narrow = ColumnBand::filter_strip(&bands, &filters_cfg(), [10.0, 30.0, 200.0, 500.0])
        .expect("expanded list");
    assert_eq!(narrow.rect, [10.0, 30.0, 190.0, 470.0]);

    let right_side = vec![ColumnBand {
        rect: [700.0, 40.0, 80.0, 60.0],
        row: 0,
    }];
    assert!(
        ColumnBand::filter_strip(&right_side, &filters_cfg(), plot).is_none(),
        "a list drawn on the right opens no left strip"
    );
}
