//! The corridor's modifier sum and timing match the core's rules.

use super::*;

/// Losing the width floor or upper clamp would change the core's run-away threshold.
#[test]
fn retreat_depth_keeps_the_core_width_clamps() {
    assert_eq!(retreat_pct(2.0, 3.0), 4.0);
    assert_eq!(retreat_pct(1.0, 5.0), 6.0);
    assert!((retreat_pct(98.5, 98.9) - 99.0).abs() < 1e-12);
    assert_eq!(retreat_pct(3.0, 3.0), 3.001);
}

/// A 100 ms or inclusive window would retain a stale low and place the buy too deep.
#[test]
fn placement_uses_only_records_younger_than_74_ms() {
    let ticks = super::super::tests::tape(&[(20, 90.0), (26, 92.0), (50, 95.0)]);
    let mut reference = Reference::new(UsePrice::Trade, true, FAST_ALGO_WINDOW_MS, 1_000);
    for tick in &ticks {
        reference.observe(tick);
    }
    assert_eq!(reference.placement(100), Some(95.0));
    assert_eq!(reference.window_extreme(100), Some(90.0));
    reference.observe(&super::super::tests::tape(&[(100, 96.0)])[0]);
    assert_eq!(reference.recent.front().map(|(t, _)| *t), Some(50));
}

/// Reusing the short placement window for Retreat would follow a rebound too early.
#[test]
fn retreat_window_uses_the_wanted_side_and_mirrors_for_shorts() {
    let ticks = super::super::tests::tape(&[(0, 100.0), (200, 102.0)]);
    for (is_long, expected) in [(true, 100.0), (false, 102.0)] {
        let mut reference = Reference::new(UsePrice::Trade, is_long, 74, 1_000);
        for tick in &ticks {
            reference.observe(tick);
        }
        assert_eq!(reference.window_extreme(1_000), Some(expected));
        assert_eq!(reference.window_extreme(1_201), Some(102.0));
    }
    let mut reference = Reference::new(UsePrice::Ask, true, 74, 1_000);
    reference.observe(&ticks[0]);
    let mut sell = ticks[1];
    sell.side = Side::Sell;
    sell.price = 90.0;
    reference.observe(&sell);
    assert_eq!(reference.window_extreme(200), Some(100.0));
    reference.observe(&ticks[1]);
    assert_eq!(reference.placement(200), Some(102.0));
    let mut immediate = Reference::new(UsePrice::Trade, true, 74, 0);
    for tick in &ticks {
        immediate.observe(tick);
    }
    assert_eq!(immediate.window_extreme(200), Some(102.0));
}

/// Waiting for another print would postpone the command and let the old order fill.
#[test]
fn approach_timer_fires_between_prints_before_a_recovery() {
    let mut deal = super::super::tests::deal();
    deal.tick = Some(1.0);
    deal.buy_set_ms = Some(0);
    deal.entry_placed = Some(99.0);
    let params = MshotParams {
        replace_delay_s: 0.5,
        raise_wait_s: 30.0,
        ..Default::default()
    };
    let ticks = super::super::tests::tape(&[(0, 100.0), (1_000, 99.3), (3_000, 100.0)]);
    let (_, moves) = MshotEntry::new(&params).trace(&deal, &ticks, None);
    assert_eq!(moves, vec![(0, 99.0), (1_498, 98.0)]);
}

/// Starting RaiseWait at a breach would delay the upward move beyond the command clock.
#[test]
fn retreat_wait_is_strictly_since_the_last_command() {
    let mut deal = super::super::tests::deal();
    deal.tick = Some(1.0);
    deal.buy_set_ms = Some(0);
    deal.entry_placed = Some(99.0);
    let params = MshotParams {
        raise_wait_s: 1.0,
        ..Default::default()
    };
    // A low at creation ages out at t=1001; without it the strict wait opens then too.
    for initial in [100.0, 102.0] {
        let mut points = vec![(0, initial)];
        points.extend((2..=12).map(|i| (i * 100, 102.0)));
        let ticks = super::super::tests::tape(&points);
        let (_, moves) = MshotEntry::new(&params).trace(&deal, &ticks, None);
        assert_eq!(moves, vec![(0, 99.0), (1_001, 100.0)]);
    }
}

/// Discarding the next-best candidate would lose the far reference when the best ages out.
#[test]
fn retreat_window_surfaces_next_best_after_expiry() {
    for (long, prices, expected) in [
        (true, [90.0, 95.0, 100.0], 95.0),
        (false, [110.0, 105.0, 100.0], 105.0),
    ] {
        let mut reference = Reference::new(UsePrice::Trade, long, 74, 1_000);
        for tick in super::super::tests::tape(&[(0, prices[0]), (200, prices[1]), (400, prices[2])])
        {
            reference.observe(&tick);
        }
        assert_eq!(reference.window_extreme(1_001), Some(expected));
        assert_eq!(reference.raise_recent.len(), 2);
    }
}

/// An extreme wait must neither overflow the command clock nor move within the capped wait.
#[test]
fn huge_raise_wait_does_not_overflow_or_move() {
    let mut deal = super::super::tests::deal();
    deal.tick = Some(1.0);
    deal.buy_set_ms = Some(0);
    deal.entry_placed = Some(99.0);
    let params = MshotParams {
        raise_wait_s: 1e12,
        replace_delay_s: 1e12,
        ..Default::default()
    };
    let ticks = super::super::tests::tape(&[(0, 102.0), (1_000, 102.0), (3_000, 102.0)]);
    let (_, moves) = MshotEntry::new(&params).trace(&deal, &ticks, None);
    assert_eq!(moves, vec![(0, 99.0)]);
    let model = MshotEntry::new(&params);
    assert_eq!(model.due_ms(Breach::Retreat, 0, 0), ORDER_WAIT_CAP_MS + 1);
    assert_eq!(model.due_ms(Breach::Approach, i64::MAX - 1, 0), i64::MAX);
    assert_eq!(model.due_ms(Breach::Retreat, 0, i64::MAX - 1), i64::MAX);
}

/// Applying a gap command immediately would lose fills during exchange latency.
#[test]
fn approach_gap_timer_preserves_the_old_level_until_exchange_apply() {
    let params = MshotParams {
        replace_delay_s: 0.5,
        raise_wait_s: 30.0,
        ..Default::default()
    };
    for (at, expected) in [(1_550, Some(99.0)), (1_600, None)] {
        let ticks = super::super::tests::tape(&[(0, 100.0), (1_000, 99.3), (at, 99.0)]);
        let (fill, moves) =
            MshotEntry::new(&params).trace(&super::super::tests::deal(), &ticks, None);
        assert_eq!(moves[1].0, 1_498);
        assert_eq!(fill.map(|fill| fill.price), expected);
    }
}

/// Ignoring expiry (or resetting it on a corridor move) would fill an already canceled buy.
#[test]
fn auto_cancel_buy_expires_from_creation_for_model_and_shift() {
    let mut deal = super::super::tests::deal();
    deal.buy_ms = 30_000;
    deal.buy_set_ms = Some(10_000);
    deal.entry_placed = Some(99.0);
    let ticks: Vec<Tick> = [(0, 100.0), (20_000, 102.0), (30_000, 99.0)]
        .into_iter()
        .map(|(t, price)| Tick {
            time_ms: t as f64,
            price,
            qty: 1.0,
            side: Side::Buy,
        })
        .collect();
    let disabled = MshotParams::default();
    let enabled = MshotParams {
        auto_cancel_buy_s: 15.0,
        ..disabled.clone()
    };
    assert_eq!(MshotEntry::new(&enabled).run(&deal, &ticks, None), None);
    assert_eq!(
        MshotEntry::new(&enabled).shifted_fill(&deal, &ticks, &disabled, None),
        None
    );
    let fill = MshotEntry::new(&disabled)
        .run(&deal, &ticks, None)
        .expect("timer disabled");
    assert_eq!(fill.t_ms, 30_000);
    assert_eq!(
        MshotEntry::new(&disabled).shifted_fill(&deal, &ticks, &disabled, None),
        Some(Fill {
            t_ms: 30_000,
            price: 99.0
        })
    );
    // Unknown creation supplies no origin: the timer cannot be inferred from the tape start.
    deal.buy_set_ms = None;
    assert!(MshotEntry::new(&enabled).run(&deal, &ticks, None).is_some());
    assert!(
        MshotEntry::new(&enabled)
            .shifted_fill(&deal, &ticks, &disabled, None)
            .is_some()
    );
}

/// Re-clamping an already parsed timer or canceling at equality would change its expiry boundary.
#[test]
fn auto_cancel_buy_keeps_the_parsed_timer_and_expiry_boundary() {
    let mut deal = super::super::tests::deal();
    deal.buy_ms = 11_000;
    deal.buy_set_ms = Some(10_000);
    deal.entry_placed = Some(99.0);
    let ticks: Vec<Tick> = [(0, 100.0), (11_000, 99.0)]
        .into_iter()
        .map(|(t, price)| Tick {
            time_ms: t as f64,
            price,
            qty: 1.0,
            side: Side::Buy,
        })
        .collect();
    let params = MshotParams {
        auto_cancel_buy_s: 1.0,
        ..Default::default()
    };
    let expected = Some(Fill {
        t_ms: 11_000,
        price: 99.0,
    });
    assert_eq!(MshotEntry::new(&params).run(&deal, &ticks, None), expected);
    assert_eq!(
        MshotEntry::new(&params).shifted_fill(&deal, &ticks, &params, None),
        expected
    );
    let mut late = ticks;
    late.last_mut().unwrap().time_ms = 11_001.0;
    assert_eq!(MshotEntry::new(&params).run(&deal, &late, None), None);
    assert_eq!(
        MshotEntry::new(&params).shifted_fill(&deal, &late, &params, None),
        None
    );
}

/// Using the placement reference for the guard would put this order one tick too deep.
#[test]
fn minus_satoshi_measures_two_steps_from_ask() {
    let params = MshotParams {
        price_pct: 0.5,
        minus_satoshi: true,
        ..Default::default()
    };
    let mut deal = super::super::tests::deal();
    deal.tick = Some(1.0);
    assert_eq!(
        MshotEntry::new(&params).place(100.0, 0.5, &deal, Some(103.0)),
        99.0
    );
    deal.is_short = true;
    assert_eq!(
        MshotEntry::new(&params).place(100.0, 0.5, &deal, Some(103.0)),
        105.0
    );
}

/// A stale taker-buy below the current reference must not move either guard below the ASK floor.
#[test]
fn minus_satoshi_rejects_a_stale_lower_ask_proxy() {
    let own = MshotParams {
        price_pct: 1.0,
        price_min_pct: 0.25,
        ..Default::default()
    };
    let variant = MshotParams {
        price_pct: 0.5,
        minus_satoshi: true,
        ..own.clone()
    };
    let mut deal = super::super::tests::deal();
    deal.tick = Some(1.0);
    let ticks = super::super::tests::tape(&[(0, 97.0)]);
    let entry = MshotEntry::new(&variant);
    assert_eq!(entry.place(100.0, 0.5, &deal, Some(97.0)), 98.0);
    assert_eq!(
        entry.anchored_level(&deal, &own, (1_000, 99.0), &ticks),
        Some(98.0)
    );
    deal.is_short = true;
    assert_eq!(entry.place(100.0, 0.5, &deal, Some(97.0)), 102.0);
    assert_eq!(
        entry.anchored_level(&deal, &own, (1_000, 101.0), &ticks),
        Some(102.0)
    );
}

/// Missing an archived placement must not let pre-creation prints place the order early.
#[test]
fn creation_without_a_usable_placement_starts_at_the_first_later_print() {
    let mut deal = super::super::tests::deal();
    deal.buy_ms = 2_000;
    deal.buy_set_ms = Some(1_000);
    let params = MshotParams {
        minus_satoshi: true,
        ..Default::default()
    };
    deal.tick = Some(1.0);
    let ticks: Vec<Tick> = [
        (0, 103.0, Side::Buy),
        (500, 90.0, Side::Sell),
        (1_100, 100.0, Side::Sell),
        (2_000, 99.0, Side::Sell),
    ]
    .into_iter()
    .map(|(t, price, side)| Tick {
        time_ms: t as f64,
        price,
        qty: 1.0,
        side,
    })
    .collect();
    for placement in [None, Some(0.0), Some(f64::NAN)] {
        deal.entry_placed = placement;
        let (fill, moves) = MshotEntry::new(&params).trace(&deal, &ticks, None);
        assert_eq!(moves, vec![(1_100, 99.0)]);
        assert_eq!(
            fill,
            Some(Fill {
                t_ms: 2_000,
                price: 99.0
            })
        );
    }
}

/// Observing pre-creation prints too late loses the ASK proxy and misplaces creation and Shift.
#[test]
fn creation_uses_the_preceding_buy_print_as_ask() {
    let own = MshotParams {
        price_pct: 0.5,
        price_min_pct: 0.25,
        ..Default::default()
    };
    let variant = MshotParams {
        minus_satoshi: true,
        ..own.clone()
    };
    let mut deal = super::super::tests::deal();
    deal.coin = "XALPHA".into();
    deal.core_name = "CORE_A".into();
    deal.tick = Some(1.0);
    deal.buy_set_ms = Some(1_000);
    deal.entry_placed = Some(99.0);
    deal.own_entry = Some(EntryParams::MoonShot(own.clone()));
    let ticks: Vec<Tick> = [
        (0, 103.0, Side::Buy),
        (500, 100.0, Side::Sell),
        (1_100, 99.0, Side::Sell),
    ]
    .into_iter()
    .map(|(t, price, side)| Tick {
        time_ms: t as f64,
        price,
        qty: 1.0,
        side,
    })
    .collect();
    let entry = MshotEntry::new(&variant);
    let (fill, moves) = entry.trace(&deal, &ticks, None);
    assert_eq!(moves[0], (1_000, 99.0));
    assert_eq!(
        fill,
        Some(Fill {
            t_ms: 1_100,
            price: 99.0
        })
    );
    assert_eq!(
        entry.anchored_level(&deal, &own, (1_000, 99.0), &ticks),
        Some(99.0)
    );
    assert_eq!(
        entry.placement_at_creation(&deal, 1_000, 0.5, None),
        Some(98.0)
    );
}

/// Raising the floor to hundredths would erase valid fine MoonShot corridors.
#[test]
fn fine_corridor_keeps_the_documented_floors() {
    let params = MshotParams {
        price_pct: 0.003,
        price_min_pct: 0.002,
        ..Default::default()
    };
    assert_eq!(params.bounds_pct(&Deltas::default()), (0.002, 0.003));
    let collapsed = MshotParams {
        price_pct: 0.0,
        price_min_pct: 0.0,
        ..params
    };
    assert_eq!(collapsed.bounds_pct(&Deltas::default()), (0.002, 0.003));
}

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
    assert_eq!(entry.anchored_level(&d, &own, (0, 99.0), &[]), Some(98.0));
    assert_eq!(entry.placement_at_creation(&d, 0, 0.5, None), Some(98.0));
    let ticks = [Tick {
        time_ms: 10_000.0,
        price: 99.0,
        qty: 1.0,
        side: Side::Buy,
    }];
    assert_eq!(entry.shifted_fill(&d, &ticks, &own, None), None);
    let fact = MshotEntry::new(&own);
    assert_eq!(fact.anchored_level(&d, &own, (0, 99.0), &[]), Some(99.0));
    assert_eq!(fact.placement_at_creation(&d, 0, 0.5, None), Some(99.0));
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
