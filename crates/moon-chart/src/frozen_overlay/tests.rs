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
                is_short: false,
                pattern: SEG_PATTERN_DASH,
            },
            OverlayTrade {
                path: Vec::new(),
                fill_ms: 3_500.0,
                fill_price: 1.01,
                exit: None,
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
