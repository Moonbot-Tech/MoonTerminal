// Explicit imports, never `use super::*`: the panel's parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute (CONTRIBUTING.md § Build and checks).
use super::sample_line;
use crate::panels::chart::geom::PaneMap;
use moon_chart::auto_figures::FoundShape;

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
