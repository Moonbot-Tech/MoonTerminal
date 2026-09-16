//! Unit tests for the pane layout every chart surface shares.

use super::*;
use crate::persistence::chart_persist::PriceAxisPos;

const PANE: Rect = Rect {
    x: 40.0,
    y: 12.0,
    w: 900.0,
    h: 400.0,
};

fn areas(pane: Rect, broom: bool, book: bool, axis: PriceAxisPos) -> PaneAreas {
    pane_layout(pane, broom, book, true, axis, None, 1.0)
}

fn hvol() -> Option<moon_chart::hvol::HvolZoneSpec> {
    Some(moon_chart::hvol::HvolZoneSpec { width_frac: 0.2 })
}

/// Both areas stay inside the pane and neither overlaps the other, whatever the flags — the
/// property every hit test depends on, checked across the combinations rather than per case.
#[test]
fn the_two_areas_tile_the_pane_without_overlapping() {
    for broom in [false, true] {
        for book in [false, true] {
            for axis in [PriceAxisPos::Left, PriceAxisPos::Right, PriceAxisPos::Hide] {
                let a = areas(PANE, broom, book, axis);
                let case = format!("broom={broom} book={book} axis={axis:?}");
                assert!(a.plot.x >= PANE.x, "plot starts left of the pane ({case})");
                assert!(
                    a.plot.x + a.plot.w <= PANE.x + PANE.w,
                    "plot runs past the pane ({case})"
                );
                assert!(a.glass.x >= PANE.x, "book starts left of the pane ({case})");
                assert!(
                    a.glass.x + a.glass.w <= PANE.x + PANE.w,
                    "book runs past the pane ({case})"
                );
                // A plot floored at one pixel is the collapsed one broom mode leaves behind; it
                // sits inside the book by construction and has nothing to overlap.
                if a.glass.w > 0.0 && a.plot.w > 1.0 {
                    assert!(
                        a.plot.x + a.plot.w <= a.glass.x || a.glass.x + a.glass.w <= a.plot.x,
                        "plot and book overlap ({case})"
                    );
                }
            }
        }
    }
}

/// The ordinary pane: an axis gutter on the left, the book flush against the right edge, and the
/// plot filling everything between them.
#[test]
fn a_left_axis_leaves_the_plot_between_its_gutter_and_the_book() {
    let a = areas(PANE, false, true, PriceAxisPos::Left);
    assert!(matches!(a.axis_pos, PriceAxisPos::Left));
    assert_eq!(a.glass.w, moon_chart::GLASS_ZONE_PX);
    assert_eq!(a.glass.x + a.glass.w, PANE.x + PANE.w);
    assert_eq!(a.plot.x, PANE.x + moon_chart::PRICE_AXIS_W);
    assert_eq!(a.plot.x + a.plot.w, a.glass.x);
}

/// A right-side axis puts its gutter OUTBOARD of the book, so the book is not flush right. Measuring
/// the book back from the pane's right edge instead — which two hit tests used to do — left its
/// left part answering as chart.
#[test]
fn a_right_axis_sits_outboard_of_the_book() {
    let a = areas(PANE, false, true, PriceAxisPos::Right);
    assert_eq!(a.plot.x, PANE.x);
    assert_eq!(a.glass.x, a.plot.x + a.plot.w);
    assert_eq!(
        a.glass.x + a.glass.w + moon_chart::PRICE_AXIS_W,
        PANE.x + PANE.w
    );
}

/// A pane too narrow to seat a full book beside a usable plot gets a narrower book rather than no
/// plot at all.
#[test]
fn a_cramped_pane_narrows_the_book_and_keeps_a_plot() {
    let narrow = Rect {
        w: moon_chart::PRICE_AXIS_W + moon_chart::GLASS_ZONE_PX * 2.5,
        ..PANE
    };
    let a = areas(narrow, false, true, PriceAxisPos::Left);
    assert!(a.glass.w < moon_chart::GLASS_ZONE_PX && a.glass.w > 0.0);
    assert!(a.plot.w > a.glass.w);
}

#[test]
fn a_disabled_book_gives_its_width_back_to_the_plot() {
    let a = areas(PANE, false, false, PriceAxisPos::Left);
    assert_eq!(a.glass.w, 0.0);
    assert_eq!(a.plot.x + a.plot.w, PANE.x + PANE.w);
}

/// The case the panel's hit testing exists to agree with: in broom mode the book IS the pane, edge
/// to edge, so a click anywhere on it is a book click and there is no plot left to pan.
#[test]
fn broom_mode_hands_the_whole_pane_to_the_book() {
    let a = areas(PANE, true, true, PriceAxisPos::Left);
    assert!(matches!(a.axis_pos, PriceAxisPos::Hide));
    assert_eq!(a.glass.x, PANE.x);
    assert_eq!(a.glass.w, PANE.w);
    assert_eq!(a.plot.w, 1.0);
}

/// Broom mode draws the book even with the window's own Order Book toggle cleared — `ChartDataState`
/// sets `orderbook_on = orderbook_enabled || orderbook_only` — so the layout must not take the
/// disabled branch and hand the pane to a plot nobody draws.
#[test]
fn broom_mode_outranks_a_cleared_order_book_toggle() {
    let a = areas(PANE, true, false, PriceAxisPos::Left);
    assert_eq!(a.glass.x, PANE.x);
    assert_eq!(a.glass.w, PANE.w);
}

/// A right-side axis is hidden by broom mode like any other, so the book reaches both edges instead
/// of leaving a gutter nothing draws into.
#[test]
fn broom_mode_hides_a_right_side_axis_too() {
    let a = areas(PANE, true, true, PriceAxisPos::Right);
    assert!(matches!(a.axis_pos, PriceAxisPos::Hide));
    assert_eq!(a.glass.w, PANE.w);
}

/// The time axis reserves its gutter under BOTH areas, and hiding it gives that height back —
/// the vertical half of the same answer, so a caller cannot take one half from here and derive the
/// other itself.
#[test]
fn the_time_axis_gutter_shortens_both_areas() {
    let with = pane_layout(PANE, false, true, true, PriceAxisPos::Left, None, 1.0);
    let without = pane_layout(PANE, false, true, false, PriceAxisPos::Left, None, 1.0);
    assert_eq!(with.plot.h, with.glass.h);
    assert_eq!(without.plot.h, PANE.h);
    assert_eq!(PANE.h - with.plot.h, moon_chart::TIME_AXIS_H);
}

/// Both areas scale with the display: at 2x device pixels the reserved gutters double, which is what
/// keeps a hit test in device pixels agreeing with what was drawn on a HiDPI screen.
#[test]
fn the_reserved_gutters_follow_the_pixel_scale() {
    let one = pane_layout(PANE, false, true, true, PriceAxisPos::Left, None, 1.0);
    let two = pane_layout(PANE, false, true, true, PriceAxisPos::Left, None, 2.0);
    assert_eq!(two.plot.x - PANE.x, (one.plot.x - PANE.x) * 2.0);
    assert_eq!(PANE.h - two.plot.h, (PANE.h - one.plot.h) * 2.0);
}

/// An unpresented slot reports a width of zero. Every number still has to come back finite, because
/// hit tests run against this layout before the first frame is drawn — and a broom pane's book has
/// to start at the pane rather than beside a gutter that mode does not reserve.
///
/// The plot is deliberately NOT asserted to be inside such a pane: a left-side axis reserves its
/// gutter regardless, which puts the plot past the right edge of a zero-width pane. Harmless,
/// because a pane of no width holds no pointer, and the first real frame replaces these numbers.
#[test]
fn an_unpresented_slot_stays_finite() {
    for broom in [false, true] {
        let a = areas(Rect { w: 0.0, ..PANE }, broom, true, PriceAxisPos::Left);
        for v in [
            a.plot.x, a.plot.w, a.plot.h, a.glass.x, a.glass.w, a.glass.h,
        ] {
            assert!(v.is_finite(), "non-finite geometry for broom={broom}");
        }
        assert_eq!(
            a.glass.x, PANE.x,
            "book starts off the pane for broom={broom}"
        );
    }
}

/// The horizontal-volume zone takes its share of the pane at the LEFT edge, outboard of the axis
/// gutter, and the three areas still tile the pane with either axis side.
#[test]
fn the_hvol_zone_sits_at_the_left_edge_and_tiles_with_the_plot_and_the_book() {
    for axis in [PriceAxisPos::Left, PriceAxisPos::Right, PriceAxisPos::Hide] {
        let a = pane_layout(PANE, false, true, true, axis, hvol(), 1.0);
        let case = format!("axis={axis:?}");
        assert_eq!(a.hvol.w, (PANE.w * 0.2).round(), "{case}");
        assert_eq!(a.hvol.h, a.plot.h, "{case}");
        assert_eq!(
            a.hvol.x, PANE.x,
            "the zone sits at the pane's edge ({case})"
        );
        let gutter = if matches!(axis, PriceAxisPos::Hide) {
            0.0
        } else {
            moon_chart::PRICE_AXIS_W
        };
        assert_eq!(
            a.plot.w + a.glass.w + a.hvol.w + gutter,
            PANE.w,
            "the areas and the gutter add up to the pane ({case})"
        );
        assert!(a.plot.x >= a.hvol.x + a.hvol.w, "{case}");
        if matches!(axis, PriceAxisPos::Left) {
            assert_eq!(a.plot.x, a.hvol.x + a.hvol.w + gutter, "{case}");
        }
    }
}

/// The zone is measured against the PANE, so a book toggle leaves it where it was; and a pane too
/// narrow to seat a readable zone gets none rather than a sliver.
#[test]
fn the_hvol_zone_keeps_its_width_across_the_book_toggle_and_vanishes_when_cramped() {
    let with_book = pane_layout(PANE, false, true, true, PriceAxisPos::Left, hvol(), 1.0);
    let no_book = pane_layout(PANE, false, false, true, PriceAxisPos::Left, hvol(), 1.0);
    assert_eq!(with_book.hvol.w, no_book.hvol.w);
    assert_eq!(no_book.plot.x + no_book.plot.w, PANE.x + PANE.w);

    let cramped = Rect { w: 150.0, ..PANE };
    let a = pane_layout(cramped, false, true, true, PriceAxisPos::Left, hvol(), 1.0);
    assert_eq!(a.hvol.w, 0.0, "30 px is under the zone's floor");
    assert_eq!(a.plot.x, cramped.x + moon_chart::PRICE_AXIS_W);

    let broom = pane_layout(PANE, true, true, true, PriceAxisPos::Left, hvol(), 1.0);
    assert_eq!(broom.hvol.w, 0.0, "the broom owns the whole pane");
    assert_eq!(broom.glass.w, PANE.w);
}

/// Every D3D11 entry point compiles offline.
///
/// The shaders compile at runtime, on the first frame that needs them, and a compile error there
/// is caught per frame as a skipped `prepare` — the layer simply never appears and the log fills
/// with the same panic thirty times a second. A reserved word used as a local (`shared`) shipped
/// the horizontal volumes exactly that way once. `D3DCompile` needs no device, so this is a unit
/// test rather than a bench run.
#[cfg(windows)]
#[test]
fn every_hlsl_entry_point_compiles() {
    const SHADERS: &[(&str, &str, &[&str], &[&str])] = &[
        (
            "background.hlsl",
            include_str!("shaders/background.hlsl"),
            &["background_vertex"],
            &["background_fragment"],
        ),
        (
            "bars.hlsl",
            include_str!("shaders/bars.hlsl"),
            &["bars_vertex", "bg_vertex"],
            &["bars_fragment", "bg_fragment"],
        ),
        (
            "blit.hlsl",
            include_str!("shaders/blit.hlsl"),
            &["blit_vertex"],
            &["blit_fragment", "blit_opaque_fragment"],
        ),
        (
            "candles.hlsl",
            include_str!("shaders/candles.hlsl"),
            &[
                "candles_vertex",
                "volume_bars_vertex",
                "volume_scale_vertex",
            ],
            &[
                "candles_fragment",
                "volume_bars_fragment",
                "volume_scale_fragment",
            ],
        ),
        (
            "crosses.hlsl",
            include_str!("shaders/crosses.hlsl"),
            &["crosses_vertex", "volume_vertex", "price_line_vertex"],
            &[
                "crosses_fragment",
                "volume_fragment",
                "price_last_fragment",
                "price_mark_fragment",
            ],
        ),
        (
            "cursor.hlsl",
            include_str!("shaders/cursor.hlsl"),
            &["cursor_vertex"],
            &["cursor_fragment"],
        ),
        (
            "grid.hlsl",
            include_str!("shaders/grid.hlsl"),
            &["grid_vertex"],
            &["grid_fragment"],
        ),
        (
            "hvol.hlsl",
            include_str!("shaders/hvol.hlsl"),
            &["hvol_row_vertex", "hvol_bg_vertex"],
            &["hvol_row_fragment", "hvol_bg_fragment"],
        ),
        (
            "order_lines.hlsl",
            include_str!("shaders/order_lines.hlsl"),
            &["zone_vertex", "hline_vertex", "seg_vertex", "marker_vertex"],
            &[
                "zone_fragment",
                "hline_fragment",
                "seg_fragment",
                "marker_fragment",
            ],
        ),
        (
            "readout.hlsl",
            include_str!("shaders/readout.hlsl"),
            &["readout_rect_vertex"],
            &["readout_rect_fragment"],
        ),
        (
            "side_volume.hlsl",
            include_str!("shaders/side_volume.hlsl"),
            &["side_band_vertex", "side_scale_vertex"],
            &["side_band_fragment", "side_scale_fragment"],
        ),
    ];
    for (name, src, vs, ps) in SHADERS {
        for entry in *vs {
            // `compile_shader` panics with the compiler's own message on failure.
            let _ = super::gpu::compile_shader(src, entry, "vs_4_1");
            eprintln!("{name}: {entry} ok");
        }
        for entry in *ps {
            let _ = super::gpu::compile_shader(src, entry, "ps_4_1");
            eprintln!("{name}: {entry} ok");
        }
    }
}
