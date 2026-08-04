//! Regression coverage for the shared stack layout decisions.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::tile_gutter;

/// `stack.rs:tile_gutter` must keep the gutter off a lone tile.
///
/// Breakage this pins: passing a literal `true`/`!fullscreen` at a `chart_stack_card` call site.
/// The gutter is 8px of panel colour below the chart with nothing beneath it to separate, and it
/// shrinks a lone chart enough to cause a vertical jump or a permanent empty strip.
#[test]
fn a_lone_tile_draws_no_gutter() {
    assert!(
        !tile_gutter(false, 1),
        "one tile has nothing to separate from"
    );
    assert!(
        !tile_gutter(false, 0),
        "an empty stack draws no gutter either"
    );
    assert!(tile_gutter(false, 2), "two tiles must stay separated");
    assert!(
        !tile_gutter(true, 3),
        "the full-bleed chart never gutters, however many siblings it hides"
    );
}
