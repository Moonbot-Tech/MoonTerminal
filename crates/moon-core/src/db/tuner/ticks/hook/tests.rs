use super::*;

/// A real comment, as the core wrote it (GateF, 22.09.2026).
const REAL: &str = " <HookTestN1>Hook Long Depth: 2.54% [2.54%] R: 62% d: 2.52% (High: 0.007838  Min: 0.007644  Max: 0.007765  [AbsHigh: 0.007870 Drop: 16.09%] VolK: 27.52) InitialPrice: 0.007644 Buffer: [1.25%..1.87%] SellPrice: 1.27%  AbsT: 8.3, NextP: 0.00000000\n CPU: Bot 17 (Avg: 10) Sys: 25  AppLatency: 0.0 sec  ";

#[test]
fn reads_the_depth_and_the_placed_take() {
    let d = parse_hook_detect(REAL).expect("hook detect");
    assert_eq!(d.depth_pct, 2.54);
    assert_eq!(d.stated_take_pct, Some(1.27));
    // The formula reproduces what the core placed, at HookSellLevel = 50.
    assert!((hook_take_pct(d.depth_pct, 50.0) - 1.27).abs() <= 0.01);
}

#[test]
fn a_comment_without_a_detect_is_not_a_hook() {
    assert_eq!(
        parse_hook_detect("MoonShot: (strategy <MainShot>)\n CPU: Bot 5"),
        None
    );
    assert_eq!(parse_hook_detect(""), None);
}

/// The per-cent sign is the guard: a price that follows a label must never read as a level.
#[test]
fn a_price_is_not_a_per_cent() {
    assert_eq!(
        parse_hook_detect("Hook Long Depth: 0.0076 InitialPrice: 0.0076"),
        None
    );
    let d = parse_hook_detect("Depth: 4.0% InitialPrice: 0.5").expect("depth");
    assert_eq!(d.depth_pct, 4.0);
    assert_eq!(d.stated_take_pct, None, "no SellPrice in this comment");
}

/// A zero or broken depth is no depth: the formula would put the take on the fill itself.
#[test]
fn a_zero_depth_is_rejected() {
    assert_eq!(
        parse_hook_detect("Hook Short Depth: 0% SellPrice: 1%"),
        None
    );
    assert_eq!(parse_hook_detect("Hook Short Depth: %"), None);
}

#[test]
fn the_take_scales_with_the_level() {
    assert_eq!(hook_take_pct(4.0, 100.0), 4.0, "the top of the move");
    assert_eq!(hook_take_pct(4.0, 50.0), 2.0);
    assert_eq!(hook_take_pct(4.0, 0.0), 0.0);
}
