//! The corridor's modifier sum (the core developer via LinKvo, 2026-09-24).

use super::*;

/// `MaxModifier` caps the `MShotAdd*` sum — with its sign, no magnitude — before
/// `MShotAddDistance` widens it for the far bound; `MShotAddPriceBug`'s term stands under 30 %.
#[test]
fn the_corridor_sum_stands_under_max_modifier_and_the_pricebug_cap() {
    let p = MshotParams {
        price_pct: 10.0,
        price_min_pct: 7.0,
        modifiers: Modifiers {
            add_1h: 1.0,
            add_pricebug: 1.0,
            distance_pct: 50.0,
            pricebug_cap: MSHOT_PRICEBUG_CAP_PCT,
            ..Modifiers::default()
        },
        max_modifier: 4.0,
        ..MshotParams::default()
    };
    // 6 of the hourly delta, capped at 4; the far bound's addition is 4 · 1.5.
    let d = Deltas {
        d1h: 6.0,
        ..Deltas::default()
    };
    let (near, far) = p.bounds_pct(&d);
    assert!(
        (near - 11.0).abs() < 1e-9 && (far - 16.0).abs() < 1e-9,
        "{near} {far}"
    );
    // A negative sum keeps its sign: the corridor comes nearer.
    let d = Deltas {
        d1h: -2.0,
        ..Deltas::default()
    };
    let (near, _) = p.bounds_pct(&d);
    assert!((near - 5.0).abs() < 1e-9, "{near}");
    // The price bug's own term stands under 30 % whatever the ceiling of the sum.
    let wide = MshotParams {
        max_modifier: 0.0,
        ..p
    };
    let d = Deltas {
        pricebug: 45.0,
        ..Deltas::default()
    };
    let (near, _) = wide.bounds_pct(&d);
    assert!((near - 37.0).abs() < 1e-9, "{near}");
}
