// Do not use `super::*`: the parent pulls GPUI's `test` attribute macro in through `gpui::*`,
// which would shadow the built-in `#[test]`.
use super::order_zone_in;
use crate::chartdx::PaneAreas;
use crate::persistence::chart_persist::PriceAxisPos;
use moon_chart::view::Rect;

const PANE: Rect = Rect {
    x: 100.0,
    y: 0.0,
    w: 400.0,
    h: 300.0,
};

fn areas(glass_w: f32) -> PaneAreas {
    PaneAreas {
        axis_pos: PriceAxisPos::Left,
        plot: Rect {
            x: PANE.x,
            y: PANE.y,
            w: PANE.w - glass_w,
            h: 280.0,
        },
        glass: Rect {
            x: PANE.x + PANE.w - glass_w,
            y: PANE.y,
            w: glass_w,
            h: 280.0,
        },
        hvol: Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        },
    }
}

#[test]
fn drawn_book_is_the_zone_whatever_the_toggle_says() {
    let a = areas(150.0);
    for reserve_strip in [true, false] {
        let zone = order_zone_in(PANE, &a, true, reserve_strip);
        assert_eq!(zone.x, a.glass.x, "reserve_strip={reserve_strip}");
        assert_eq!(zone.w, a.glass.w, "reserve_strip={reserve_strip}");
    }
}

#[test]
fn hidden_book_with_the_toggle_on_reserves_the_capped_strip() {
    let a = areas(0.0);
    let zone = order_zone_in(PANE, &a, false, true);
    // 220 px would be more than half of this 400 px pane, so the cap wins.
    assert_eq!(zone.w, PANE.w * 0.5);
    assert_eq!(
        zone.x + zone.w,
        PANE.x + PANE.w,
        "strip hugs the right edge"
    );
    assert_eq!(zone.h, a.plot.h);
}

#[test]
fn hidden_book_with_the_toggle_off_has_no_zone() {
    let a = areas(0.0);
    let zone = order_zone_in(PANE, &a, false, false);
    assert_eq!(
        zone.w, 0.0,
        "no strip is reserved: the pane is chart edge to edge"
    );
    // Parked at the right edge, so `window_pos_in_glass_zone`'s `x >= zone.x` cannot claim any
    // pixel of the plot for the stack's wheel either.
    assert_eq!(zone.x, PANE.x + PANE.w);
}
