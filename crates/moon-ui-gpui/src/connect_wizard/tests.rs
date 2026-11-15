//! Where the wizard opens, against work areas it does and does not fit in.

use gpui::{Bounds, Pixels, point, px, size};

use super::{DEFAULT_SIZE, MIN_SIZE, opening_bounds};

/// A work area with its top-left corner at `(x, y)`.
fn work_area(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(w), px(h)),
    }
}

/// Whether `inner` lies wholly inside `outer`.
fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    inner.origin.x >= outer.origin.x
        && inner.origin.y >= outer.origin.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

/// A work area the default fits in leaves the size alone and centres the window in it, on a
/// second monitor as well as on the first; an unknown display leaves the size alone too.
#[test]
fn a_roomy_or_unknown_display_keeps_the_default_size() {
    let default = size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1));
    // A second monitor to the right of the first: its work area does not start at the origin.
    let second = work_area(2560.0, 0.0, 2560.0, 1392.0);
    let opened = opening_bounds(Some(second), 1.0);
    assert_eq!(opened.size, default);
    assert_eq!(opened.center(), second.center());

    assert_eq!(opening_bounds(None, 1.0).size, default);
}

/// On a 1366×768 laptop the taskbar leaves 720 px: the default height centred on the whole
/// screen would open the footer's buttons under it, and a zoomed interface would not fit the
/// width either. The window is held to the work area and stays inside it.
#[test]
fn a_small_display_holds_the_window_inside_its_work_area() {
    let laptop = work_area(0.0, 0.0, 1366.0, 720.0);

    let opened = opening_bounds(Some(laptop), 1.0);
    assert_eq!(opened.size.width, px(DEFAULT_SIZE.0));
    assert_eq!(opened.size.height, laptop.size.height);
    assert!(inside(opened, laptop), "{opened:?}");

    let zoomed = opening_bounds(Some(laptop), 1.5);
    assert_eq!(zoomed.size.width, px(DEFAULT_SIZE.0 * 1.5));
    assert_eq!(zoomed.size.height, laptop.size.height);
    assert!(inside(zoomed, laptop), "{zoomed:?}");

    // A taskbar on the left edge moves the work area, and the window with it.
    let side_taskbar = work_area(62.0, 0.0, 1304.0, 768.0);
    assert!(inside(
        opening_bounds(Some(side_taskbar), 1.5),
        side_taskbar
    ));
}

/// A work area smaller than the minimum cannot push the window under the size its layout is made
/// for: the minimum wins and the window overhangs instead.
#[test]
fn the_minimum_size_outranks_a_tiny_display() {
    let opened = opening_bounds(Some(work_area(0.0, 0.0, 320.0, 240.0)), 1.0);
    assert_eq!(opened.size, size(px(MIN_SIZE.0), px(MIN_SIZE.1)));
}
