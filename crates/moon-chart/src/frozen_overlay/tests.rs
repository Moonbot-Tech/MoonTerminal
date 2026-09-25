use super::*;
use crate::layers::{SEG_PATTERN_DASH, SEG_PATTERN_DOT};

fn build(overlay: &FrozenOverlay) -> (Vec<ZoneInstance>, Vec<SegInstance>, Vec<MarkerInstance>) {
    let (mut zones, mut segs, mut markers) = (Vec::new(), Vec::new(), Vec::new());
    build_overlay_geometry(
        overlay,
        &OrdersStyle::default(),
        &ChartGraphicsCfg::default(),
        1.0,
        1_000.0,
        &mut zones,
        &mut segs,
        &mut markers,
    );
    (zones, segs, markers)
}

/// The corridor is a band between its two prices — in either order — over the order's life only,
/// not the whole plot a live corridor spans.
#[test]
fn the_corridor_is_bounded_by_the_orders_life() {
    let (zones, segs, markers) = build(&FrozenOverlay {
        bands: vec![OverlayBand {
            from_ms: 2_000.0,
            to_ms: 5_000.0,
            prices: (1.2, 1.1),
        }],
        trades: Vec::new(),
    });
    assert_eq!(zones.len(), 1);
    assert_eq!((zones[0].price0, zones[0].price1), (1.1, 1.2));
    assert_eq!((zones[0].t0_rel, zones[0].t1_rel), (1_000.0, 4_000.0));
    assert!(segs.is_empty() && markers.is_empty());
    // A micro-priced corridor is a band too, however close its prices sit.
    let (zones, _, _) = build(&FrozenOverlay {
        bands: vec![OverlayBand {
            from_ms: 2_000.0,
            to_ms: 5_000.0,
            prices: (1.2e-7, 1.25e-7),
        }],
        trades: Vec::new(),
    });
    assert_eq!(zones.len(), 1);
    // A zero-width band and a price that is not one draw nothing.
    let (zones, _, _) = build(&FrozenOverlay {
        bands: vec![
            OverlayBand {
                from_ms: 2_000.0,
                to_ms: 5_000.0,
                prices: (1.1, 1.1),
            },
            OverlayBand {
                from_ms: 2_000.0,
                to_ms: 5_000.0,
                prices: (0.0, 1.1),
            },
        ],
        trades: Vec::new(),
    });
    assert!(zones.is_empty());
}

/// A modelled trade draws its two arrows, the exit line and the connector in its own pen; one the
/// model left open at the tape's end draws its entry alone.
#[test]
fn a_modelled_trade_carries_its_pen_and_an_open_one_only_its_entry() {
    let (zones, segs, markers) = build(&FrozenOverlay {
        bands: Vec::new(),
        trades: vec![
            OverlayTrade {
                path: Vec::new(),
                fill_ms: 3_000.0,
                fill_price: 1.0,
                exit: Some((6_000.0, 1.05)),
                exit_path: Vec::new(),
                is_short: false,
                pattern: SEG_PATTERN_DASH,
            },
            OverlayTrade {
                path: Vec::new(),
                fill_ms: 3_500.0,
                fill_price: 1.01,
                exit: None,
                exit_path: Vec::new(),
                is_short: true,
                pattern: SEG_PATTERN_DOT,
            },
        ],
    });
    assert!(zones.is_empty());
    assert_eq!(
        markers.len(),
        3,
        "two ends of the closed trade, the entry of the open one"
    );
    assert_eq!(
        segs.len(),
        2,
        "the exit line and the connector of the closed trade"
    );
    assert!(segs.iter().all(|s| s.pattern == SEG_PATTERN_DASH));
    assert_eq!((segs[0].t0_rel, segs[0].t1_rel), (2_000.0, 5_000.0));
    assert_eq!((segs[0].p0, segs[0].p1), (1.05, 1.05));
    assert_eq!((segs[1].p0, segs[1].p1), (1.0, 1.05));
}

/// The model's entry path is stepped to the fill: a level per placement, a riser at each move,
/// nothing past the fill.
#[test]
fn a_modelled_entry_path_steps_to_the_fill() {
    let (_, segs, markers) = build(&FrozenOverlay {
        bands: Vec::new(),
        trades: vec![OverlayTrade {
            path: vec![(1_500.0, 0.98), (2_000.0, 0.99), (4_000.0, 0.97)],
            fill_ms: 3_000.0,
            fill_price: 0.99,
            exit: None,
            exit_path: Vec::new(),
            is_short: false,
            pattern: SEG_PATTERN_DASH,
        }],
    });
    assert_eq!(markers.len(), 1);
    let levels: Vec<(f32, f32, f32)> = segs
        .iter()
        .filter(|s| s.p0 == s.p1)
        .map(|s| (s.t0_rel, s.t1_rel, s.p0))
        .collect();
    assert_eq!(
        levels,
        vec![(500.0, 1_000.0, 0.98), (1_000.0, 2_000.0, 0.99)],
        "the second level ends at the fill; the move after it is not drawn"
    );
    assert_eq!(
        segs.iter().filter(|s| s.p0 != s.p1).count(),
        1,
        "one riser: the second move falls after the fill"
    );
}

/// The model's sell path is stepped from its placement to the close, like the entry path, and
/// stands in for the flat exit line; the exit arrow keeps the exit's own price.
#[test]
fn a_modelled_sell_path_steps_to_the_close() {
    let (_, segs, markers) = build(&FrozenOverlay {
        bands: Vec::new(),
        trades: vec![OverlayTrade {
            path: Vec::new(),
            fill_ms: 2_000.0,
            fill_price: 1.0,
            exit: Some((5_000.0, 1.02)),
            exit_path: vec![
                (2_000.0, 1.05),
                (3_000.0, 1.03),
                (4_000.0, 1.02),
                (6_000.0, 1.01),
            ],
            is_short: false,
            pattern: SEG_PATTERN_DASH,
        }],
    });
    assert_eq!(markers.len(), 2, "the entry and the exit arrow");
    let levels: Vec<(f32, f32, f32)> = segs
        .iter()
        .filter(|s| s.p0 == s.p1)
        .map(|s| (s.t0_rel, s.t1_rel, s.p0))
        .collect();
    assert_eq!(
        levels,
        vec![
            (1_000.0, 2_000.0, 1.05),
            (2_000.0, 3_000.0, 1.03),
            (3_000.0, 4_000.0, 1.02),
        ],
        "no flat line at the exit price, and nothing past the close"
    );
    assert_eq!(
        segs.iter().filter(|s| s.p0 != s.p1).count(),
        3,
        "two risers and the fill-to-exit connector"
    );
}

/// A sell path none of whose levels stood by the close — a stop inside `SellDelay`, the sell not
/// yet placed — draws the flat exit line rather than nothing.
#[test]
fn a_sell_path_placed_after_the_close_falls_back_to_the_flat_line() {
    let (_, segs, _) = build(&FrozenOverlay {
        bands: Vec::new(),
        trades: vec![OverlayTrade {
            path: Vec::new(),
            fill_ms: 2_000.0,
            fill_price: 1.0,
            exit: Some((3_000.0, 0.98)),
            exit_path: vec![(4_000.0, 1.05)],
            is_short: false,
            pattern: SEG_PATTERN_DASH,
        }],
    });
    let levels: Vec<(f32, f32, f32)> = segs
        .iter()
        .filter(|s| s.p0 == s.p1)
        .map(|s| (s.t0_rel, s.t1_rel, s.p0))
        .collect();
    assert_eq!(levels, vec![(1_000.0, 2_000.0, 0.98)]);
}
