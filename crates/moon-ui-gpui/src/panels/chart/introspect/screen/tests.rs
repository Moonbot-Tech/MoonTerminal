// Explicit imports, never `use super::*`: the panel's parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute (CONTRIBUTING.md § Build and checks).
use super::{sample_line, sample_squeeze_slope};
use crate::panels::chart::geom::PaneMap;
use moon_chart::auto_figures::FoundShape;
use moon_core::config::auto_figures::Extend;

/// A 230 x 100 plot at the origin showing 0..230 s (1 s per pixel) and prices 90..110, so every
/// expected pixel below is arithmetic on round numbers, not the projection under test.
fn map() -> PaneMap {
    PaneMap {
        plot: moon_chart::view::Rect {
            x: 0.0,
            y: 0.0,
            w: 230.0,
            h: 100.0,
        },
        epoch_ms: 0.0,
        left_rel: 0.0,
        window_ms: 230_000.0,
        center: 100.0,
        range: 20.0,
    }
}

#[test]
fn a_level_keeps_only_the_points_inside_its_drawn_stretch() {
    // 24 samples over 230 px: one every 10 px. The stretch starts at 120 s, so x 120..=230.
    let level = FoundShape::Horizontal { price: 105.0 };
    let points = sample_line(&level, None, (120_000.0, f64::INFINITY), &map(), 24);
    let xs: Vec<f32> = points.iter().map(|p| p.0).collect();
    let expected: Vec<f32> = (12..=23).map(|i| i as f32 * 10.0).collect();
    assert_eq!(xs, expected);
    // 105 is a quarter of the range above the centre: a quarter of the height above the middle.
    assert!(
        points.iter().all(|p| (p.1 - 25.0).abs() < 1e-3),
        "{points:?}"
    );
}

#[test]
fn a_level_clipped_off_the_plot_yields_no_point_to_aim_at() {
    let above = FoundShape::Horizontal { price: 115.0 };
    let points = sample_line(&above, None, (f64::NEG_INFINITY, f64::INFINITY), &map(), 24);
    assert!(points.is_empty(), "{points:?}");
}

#[test]
fn a_sloped_line_is_followed_from_corner_to_corner() {
    // From the bottom-left corner (0 s, 90) to the top-right one (230 s, 110).
    let trend = FoundShape::Sloped {
        t0_ms: 0.0,
        p0: 90.0,
        t1_ms: 230_000.0,
        p1: 110.0,
    };
    let points = sample_line(&trend, None, (0.0, 230_000.0), &map(), 24);
    assert_eq!(points.len(), 24, "{points:?}");
    let first = points[0];
    let last = points[23];
    assert!((first.0 - 0.0).abs() < 1e-3 && (first.1 - 100.0).abs() < 1e-3);
    assert!((last.0 - 230.0).abs() < 1e-3 && (last.1 - 0.0).abs() < 1e-3);
}

/// A squeeze on level 95 whose falling line touches 110 at 0 s and 105 at 50 s: it meets the
/// level at 150 s.
fn squeeze() -> FoundShape {
    FoundShape::Squeeze {
        level: 95.0,
        level_t0_ms: 0.0,
        level_t1_ms: 100_000.0,
        t0_ms: 0.0,
        p0: 110.0,
        t1_ms: 50_000.0,
        p1: 105.0,
        pressed: false,
    }
}

#[test]
fn a_squeeze_slope_runs_from_its_first_touch_to_the_apex() {
    let points = sample_squeeze_slope(&squeeze(), Extend::RightEdge, 200_000.0, &map(), 24);
    let xs: Vec<f32> = points.iter().map(|p| p.0).collect();
    let expected: Vec<f32> = (0..=15).map(|i| i as f32 * 10.0).collect();
    assert_eq!(xs, expected);
    // From 110 on the top edge (y 0) down to the level 95 at 150 s (y 75).
    assert!((points[0].1 - 0.0).abs() < 1e-3, "{points:?}");
    assert!((points[15].1 - 75.0).abs() < 1e-3, "{points:?}");
}

/// Breakage this pins: the apex ring of a film drawn on the last grid column before the apex,
/// up to a 23rd of the plot short of where the lines meet.
#[test]
fn a_squeeze_slope_ends_exactly_at_an_apex_between_grid_columns() {
    // Level 95.5: the line from 110 falling 5 per 50 s meets it at 145 s, between columns.
    let shape = FoundShape::Squeeze {
        level: 95.5,
        level_t0_ms: 0.0,
        level_t1_ms: 100_000.0,
        t0_ms: 0.0,
        p0: 110.0,
        t1_ms: 50_000.0,
        p1: 105.0,
        pressed: false,
    };
    let points = sample_squeeze_slope(&shape, Extend::RightEdge, 200_000.0, &map(), 24);
    let last = points.last().copied().expect("points");
    assert!((last.0 - 145.0).abs() < 1e-3, "{points:?}");
    // 95.5 is 14.5 below 110: 72.5 % of the height.
    assert!((last.1 - 72.5).abs() < 1e-3, "{points:?}");
}

#[test]
fn a_squeeze_slope_stops_at_the_runs_time_before_its_apex_and_never_before_its_last_touch() {
    let at_ref = sample_squeeze_slope(&squeeze(), Extend::RightEdge, 100_000.0, &map(), 24);
    assert_eq!(at_ref.last().map(|p| p.0), Some(100.0), "{at_ref:?}");
    let stale = sample_squeeze_slope(&squeeze(), Extend::RightEdge, 10_000.0, &map(), 24);
    assert_eq!(stale.last().map(|p| p.0), Some(50.0), "{stale:?}");
}

#[test]
fn a_squeeze_drawn_between_touches_ends_its_slope_at_the_last_touch() {
    let points = sample_squeeze_slope(&squeeze(), Extend::BetweenTouches, 200_000.0, &map(), 24);
    assert_eq!(points.last().map(|p| p.0), Some(50.0), "{points:?}");
}

/// Breakage this pins: `BothEdges` sent down a branch of its own — a squeeze has no left extension,
/// so it ends where `RightEdge` does.
#[test]
fn a_squeeze_drawn_to_both_edges_ends_its_slope_like_one_drawn_to_the_right() {
    let both = sample_squeeze_slope(&squeeze(), Extend::BothEdges, 200_000.0, &map(), 24);
    assert_eq!(both.last().map(|p| p.0), Some(150.0), "{both:?}");
}

/// Breakage this pins: a flat line's infinite "apex" taken as an end — it has none, so the slope
/// runs to the run's time.
#[test]
fn a_flat_squeeze_line_runs_to_the_runs_time() {
    let flat = FoundShape::Squeeze {
        level: 95.0,
        level_t0_ms: 0.0,
        level_t1_ms: 100_000.0,
        t0_ms: 0.0,
        p0: 105.0,
        t1_ms: 50_000.0,
        p1: 105.0,
        pressed: false,
    };
    let points = sample_squeeze_slope(&flat, Extend::RightEdge, 120_000.0, &map(), 24);
    assert_eq!(points.last().map(|p| p.0), Some(120.0), "{points:?}");
}

#[test]
fn a_squeeze_with_a_non_positive_level_has_no_slope() {
    let shape = FoundShape::Squeeze {
        level: 0.0,
        level_t0_ms: 0.0,
        level_t1_ms: 100_000.0,
        t0_ms: 0.0,
        p0: 110.0,
        t1_ms: 50_000.0,
        p1: 105.0,
        pressed: false,
    };
    assert!(sample_squeeze_slope(&shape, Extend::RightEdge, 200_000.0, &map(), 24).is_empty());
}

#[test]
fn only_a_squeeze_has_a_slope() {
    let level = FoundShape::Horizontal { price: 100.0 };
    assert!(sample_squeeze_slope(&level, Extend::RightEdge, 1.0, &map(), 24).is_empty());
}
