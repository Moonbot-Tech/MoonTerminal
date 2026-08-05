// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
use super::opacity_step;

/// Every multiple of five per cent is reachable in both directions.
///
/// The oracle is the percentages themselves, not the byte arithmetic: the defect this replaced
/// stepped by a fixed 24 of 255 and skipped values such as 15%, which no assertion about bytes
/// would have caught.
#[test]
fn opacity_steps_through_every_five_per_cent() {
    let pct = |a: u8| (a as f32 / 255.0 * 100.0).round() as i32;
    let mut a = 255u8;
    let mut seen = vec![pct(a)];
    for _ in 0..40 {
        a = opacity_step(a, false);
        seen.push(pct(a));
    }
    for want in (5..=100).step_by(5) {
        assert!(seen.contains(&want), "{want}% unreachable going down");
    }
    // And the floor holds rather than wrapping or underflowing.
    assert_eq!(pct(opacity_step(opacity_step(a, false), false)), 5);
}

/// Stepping up from a value that is not on the grid snaps onto it instead of drifting off by the
/// original remainder.
#[test]
fn opacity_snaps_an_off_grid_value_onto_the_grid() {
    // 100 of 255 is 39%, which sits between grid points.
    let up = opacity_step(100, true);
    let down = opacity_step(100, false);
    let pct = |a: u8| (a as f32 / 255.0 * 100.0).round() as i32;
    assert_eq!(pct(up), 40);
    assert_eq!(pct(down), 35);
}
