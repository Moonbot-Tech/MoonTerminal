//! The corridor's modifier sum (the core developer via LinKvo, 2026-09-24).

use super::*;

/// Repricing archived fallback levels would miss fills observed with a deeper variant bound.
fn assert_fallback_keeps_archived_levels(line: &[(i64, f64)]) {
    let own = MshotParams {
        price_pct: 1.0,
        price_min_pct: 0.5,
        minus_satoshi: false,
        replace_delay_s: 30.0,
        raise_wait_s: 30.0,
        ..MshotParams::default()
    };
    let variant = MshotParams {
        price_pct: 2.0,
        ..own.clone()
    };
    let mut d = super::super::tests::deal();
    d.coin = "XALPHA".into();
    d.core_name = "CORE_A".into();
    d.buy_set_ms = Some(-700_000);
    d.entry_placed = Some(99.0);
    d.own_entry = Some(EntryParams::MoonShot(own.clone()));
    assert_eq!(
        d.order_open_ms(),
        None,
        "creation is older than ten minutes"
    );
    let ticks: Vec<Tick> = [(0, 100.0), (4_000, 100.0), (10_000, 99.0), (11_000, 100.0)]
        .into_iter()
        .map(|(t, p)| Tick {
            time_ms: t as f64,
            price: p,
            qty: 1.0,
            side: Side::Buy,
        })
        .collect();
    assert_eq!(
        MshotEntry::new(&own).run(&d, &ticks, Some(line)),
        Some(Fill {
            t_ms: 10_000,
            price: 99.0
        })
    );
    assert_eq!(
        MshotEntry::new(&variant).run(&d, &ticks, Some(line)),
        Some(Fill {
            t_ms: 10_000,
            price: 99.0
        }),
        "the variant's far bound does not move an archived level"
    );
}

/// Repricing the archived start would miss the fill that a deeper variant made there.
#[test]
fn fallback_start_stays_archived_for_the_variant() {
    assert_fallback_keeps_archived_levels(&[(0, 99.0)]);
}

/// Repricing a blind-window hint would miss the fill that a deeper variant made there.
#[test]
fn fallback_blind_hint_stays_archived_for_the_variant() {
    assert_fallback_keeps_archived_levels(&[(0, 98.5), (5_000, 99.0)]);
}

/// Reapplying the two-tick minimum to archived fallback levels would lose their recorded fill.
#[test]
fn fallback_keeps_archived_levels_when_only_minus_satoshi_changes() {
    let own = MshotParams {
        price_pct: 0.5,
        price_min_pct: 0.25,
        minus_satoshi: false,
        replace_delay_s: 30.0,
        raise_wait_s: 30.0,
        ..MshotParams::default()
    };
    let variant = MshotParams {
        minus_satoshi: true,
        ..own.clone()
    };
    let mut d = super::super::tests::deal();
    d.coin = "XALPHA".into();
    d.core_name = "CORE_A".into();
    d.buy_set_ms = Some(-700_000);
    d.entry_placed = Some(99.0);
    d.tick = Some(1.0);
    d.own_entry = Some(EntryParams::MoonShot(own.clone()));
    assert_eq!(
        d.order_open_ms(),
        None,
        "creation is older than ten minutes"
    );
    let ticks: Vec<Tick> = [(0, 100.0), (4_000, 100.0), (10_000, 99.0), (11_000, 100.0)]
        .into_iter()
        .map(|(t, p)| Tick {
            time_ms: t as f64,
            price: p,
            qty: 1.0,
            side: Side::Buy,
        })
        .collect();
    for line in [&[(0, 99.0)][..], &[(0, 98.0), (5_000, 99.0)][..]] {
        for params in [&own, &variant] {
            assert_eq!(
                MshotEntry::new(params).run(&d, &ticks, Some(line)),
                Some(Fill {
                    t_ms: 10_000,
                    price: 99.0
                }),
                "the two-tick minimum does not move an archived start or blind-window hint"
            );
        }
    }
}

/// Equal far bounds still apply a changed two-tick minimum to creation placement and Shift entry.
#[test]
fn minus_satoshi_change_reprices_equal_far_bounds() {
    let own = MshotParams {
        price_pct: 0.5,
        price_min_pct: 0.25,
        minus_satoshi: false,
        use_price: UsePrice::Ask,
        ..MshotParams::default()
    };
    let variant = MshotParams {
        minus_satoshi: true,
        ..own.clone()
    };
    let mut d = super::super::tests::deal();
    d.coin = "XALPHA".into();
    d.core_name = "CORE_A".into();
    d.tick = Some(1.0);
    d.entry_placed = Some(99.0);
    d.buy_set_ms = Some(0);
    d.own_entry = Some(EntryParams::MoonShot(own.clone()));
    let entry = MshotEntry::new(&variant);
    assert_eq!(entry.anchored_level(&d, &own, (0, 99.0)), Some(98.0));
    assert_eq!(entry.placement_at_creation(&d, 0, 0.5), Some(98.0));
    let ticks = [Tick {
        time_ms: 10_000.0,
        price: 99.0,
        qty: 1.0,
        side: Side::Buy,
    }];
    assert_eq!(entry.shifted_fill(&d, &ticks, &own, None), None);
    let fact = MshotEntry::new(&own);
    assert_eq!(fact.anchored_level(&d, &own, (0, 99.0)), Some(99.0));
    assert_eq!(fact.placement_at_creation(&d, 0, 0.5), Some(99.0));
    assert_eq!(
        fact.shifted_fill(&d, &ticks, &own, None),
        Some(Fill {
            t_ms: 10_000,
            price: 99.0
        })
    );
}

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
