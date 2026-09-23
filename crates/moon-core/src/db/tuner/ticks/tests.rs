//! The model on synthetic tapes: every rule of the spec's §8, one print at a time.

use std::collections::HashMap;

use super::exit::pre_spike_price;
use super::exit::stop_pct as moon_core_stop_pct;
use super::mshot::{DEFAULT_LATENCY_MS, Modifiers, PRE_SPIKE_LOOKBACK_MS};
use super::params::{StrategyValues, exit_params, mshot_params, param_keys, params_for};
use super::verify::share;
use super::*;
use crate::feed::types::Side;

/// The ignored run over a live data root.
mod real_data;
mod required;

fn tick(t_ms: i64, price: f64, side: Side) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side,
    }
}

/// A tape of buy-side prints at `(t_ms, price)`.
fn tape(points: &[(i64, f64)]) -> Vec<Tick> {
    points.iter().map(|&(t, p)| tick(t, p, Side::Buy)).collect()
}

pub(super) fn deal() -> Deal {
    Deal {
        report_uid: 1,
        core_uid: 7,
        core_name: String::new(),
        strategy_id: 42,
        kind: "MoonShot".into(),
        coin: "ACE".into(),
        buy_ms: 10_000,
        close_ms: 20_000,
        buy_price: 99.0,
        sell_price: 100.0,
        spent: 1_000.0,
        is_short: false,
        sell_reason: "Sell Price".into(),
        fact_pnl: 10.0,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        step_lag_ms: 0.0,
        stop_anchor: None,
        delta_track: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
    }
}

fn short_deal() -> Deal {
    Deal {
        is_short: true,
        buy_price: 101.0,
        sell_price: 100.0,
        ..deal()
    }
}

/// A 1 % / 0.5 % corridor with no waits and a 100 ms latency.
fn mshot() -> MshotParams {
    MshotParams::default()
}

fn fill_of(deal: &Deal, ticks: &[Tick], params: &MshotParams) -> Option<Fill> {
    MshotEntry::new(params).fill(deal, ticks, None)
}

// ---- entry: the level and the fill -------------------------------------------------------

#[test]
fn a_spike_exactly_to_the_level_fills_at_the_level() {
    // Placed off the first print at 100: level 99.
    let ticks = tape(&[(0, 100.0), (500, 99.5), (1_000, 99.0)]);
    let fill = fill_of(&deal(), &ticks, &mshot()).expect("filled");
    assert_eq!(fill.t_ms, 1_000);
    assert!((fill.price - 99.0).abs() < 1e-9);
}

#[test]
fn a_spike_one_step_short_does_not_fill() {
    let ticks = tape(&[(0, 100.0), (500, 99.5), (1_000, 99.01)]);
    assert_eq!(fill_of(&deal(), &ticks, &mshot()), None);
}

#[test]
fn the_fill_price_is_the_level_not_the_print() {
    let ticks = tape(&[(0, 100.0), (1_000, 97.0)]);
    let fill = fill_of(&deal(), &ticks, &mshot()).expect("filled");
    assert!(
        (fill.price - 99.0).abs() < 1e-9,
        "a limit fills at its own price"
    );
}

#[test]
fn an_empty_tape_never_fills() {
    assert_eq!(fill_of(&deal(), &[], &mshot()), None);
}

#[test]
fn the_archived_start_places_the_order_where_the_core_did() {
    // The tape says 100 at t=0, but the archive says the order stood at 98.5 from t=200.
    let ticks = tape(&[(0, 100.0), (300, 99.0), (600, 98.5)]);
    let fill = MshotEntry::new(&mshot())
        .fill(&deal(), &ticks, Some(&[(200, 98.5)]))
        .expect("filled");
    assert_eq!(fill.t_ms, 600);
    assert!((fill.price - 98.5).abs() < 1e-9);
}

#[test]
fn the_archived_level_at_the_tape_start_is_the_last_one_before_it() {
    // The archive: 97 from t=-500, moved to 98.5 at t=-100; the tape begins at t=0. The order
    // stands at 98.5, not at the archive's first level: the prints at 99.2 and 99 leave it
    // inside the 1 % / 0.5 % corridor, and the print at 98.5 fills it. Started at 97 it would
    // be 2.2 % off the first print, re-placed at once to 98.2, and never reached.
    let ticks = tape(&[(0, 99.2), (300, 99.0), (600, 98.5)]);
    let fill = MshotEntry::new(&mshot())
        .fill(&deal(), &ticks, Some(&[(-500, 97.0), (-100, 98.5)]))
        .expect("filled");
    assert_eq!(fill.t_ms, 600);
    assert!((fill.price - 98.5).abs() < 1e-9);
    assert_eq!(
        MshotEntry::new(&mshot()).fill(&deal(), &ticks, Some(&[(-500, 97.0)])),
        None,
        "from the first level the order is re-placed below the tape"
    );
}

#[test]
fn the_archived_level_is_the_latest_by_time_whatever_the_archive_order() {
    // The archive files a move as two points a few milliseconds apart, not always in time
    // order: here the 98.5 level's point precedes the 97 level's in the slice while following
    // it in time. The start is the latest by time, 98.5.
    let ticks = tape(&[(0, 99.2), (300, 99.0), (600, 98.5)]);
    let fill = MshotEntry::new(&mshot())
        .fill(&deal(), &ticks, Some(&[(-100, 98.5), (-131, 97.0)]))
        .expect("filled");
    assert_eq!(fill.t_ms, 600);
    assert!((fill.price - 98.5).abs() < 1e-9);
}

#[test]
fn a_move_archived_inside_the_blind_window_is_applied_as_archived() {
    // Raise wait 30 s. The archive: 97 from t=-60 000 (the price ran away long before the
    // tape), re-placed at 99 at t=300 — 0.3 s into the tape, a wait the model cannot see the
    // start of. The print at 99 at t=1000 fills the archived level; on its own the model would
    // wait 30 s from the first print and never fill (the tape ends first).
    let params = MshotParams {
        raise_wait_s: 30.0,
        ..mshot()
    };
    let ticks = tape(&[(0, 100.0), (500, 100.0), (1_000, 99.0), (2_000, 100.0)]);
    let line = [(-60_000, 97.0), (300, 99.0)];
    let fill = MshotEntry::new(&params)
        .fill(&deal(), &ticks, Some(&line))
        .expect("filled at the archived move");
    assert_eq!(fill.t_ms, 1_000);
    assert!((fill.price - 99.0).abs() < 1e-9);
    assert_eq!(
        MshotEntry::new(&params).fill(&deal(), &ticks, Some(&line[..1])),
        None,
        "without the move the model waits its 30 s"
    );
}

#[test]
fn a_move_archived_past_the_blind_window_is_not_applied() {
    // The same, but the archived move sits at t=31 000 — past the 30 s the model can account
    // for on its own; it is left to the model, which by then has re-placed by its own rule
    // (Retreat since the first print, at t=30 000: off the reference 100, level 99, effective
    // at 30 100) — the print at 98 at t=31 500 fills THAT level, not the archived 97.5.
    let params = MshotParams {
        raise_wait_s: 30.0,
        ..mshot()
    };
    let ticks = tape(&[(0, 100.0), (30_000, 100.0), (31_500, 98.0)]);
    let line = [(-60_000, 96.0), (31_000, 97.5)];
    let fill = MshotEntry::new(&params)
        .fill(&deal(), &ticks, Some(&line))
        .expect("filled");
    assert!((fill.price - 99.0).abs() < 1e-9, "{fill:?}");
}

#[test]
fn snap_to_step_keeps_a_level_already_on_the_grid() {
    // 0.3379 / 0.0001 evaluates to 3378.9999999999995: a plain floor loses the step.
    assert!((snap_to_step(0.3379, 0.0001, true) - 0.3379).abs() < 1e-12);
    assert!((snap_to_step(0.33795, 0.0001, true) - 0.3379).abs() < 1e-12);
    assert!((snap_to_step(0.33795, 0.0001, false) - 0.3380).abs() < 1e-12);
    assert_eq!(
        snap_to_step(0.33795, 0.0, true),
        0.33795,
        "no step, no snap"
    );
    assert!((round_to_step(0.196445, 0.0001) - 0.1964).abs() < 1e-12);
    assert!((round_to_step(0.19646, 0.0001) - 0.1965).abs() < 1e-12);
}

// ---- entry: the corridor, the waits and the latency race ----------------------------------

#[test]
fn approaching_inside_price_min_moves_the_order_after_the_latency() {
    // Level 99 off 100. At t=1000 the price is 99.6: distance 0.6 % > 0.5 %, inside the
    // corridor. At t=2000 it is 99.3: distance 0.3 % < PriceMin → replace at once (delay 0),
    // effective at t=2100. A print at 99.0 at t=2050 still fills the OLD level; the same print
    // at t=2200 does not — the order has moved to 99.3 · 0.99.
    let raced = tape(&[(0, 100.0), (1_000, 99.6), (2_000, 99.3), (2_050, 99.0)]);
    let fill = fill_of(&deal(), &raced, &mshot()).expect("filled at the old level");
    assert_eq!(fill.t_ms, 2_050);
    assert!((fill.price - 99.0).abs() < 1e-9);

    let missed = tape(&[(0, 100.0), (1_000, 99.6), (2_000, 99.3), (2_200, 99.0)]);
    assert_eq!(
        fill_of(&deal(), &missed, &mshot()),
        None,
        "the order had moved"
    );
}

#[test]
fn without_latency_the_corridor_is_never_reached_by_a_step_down() {
    let params = MshotParams {
        latency_ms: 0.0,
        ..mshot()
    };
    let ticks = tape(&[(0, 100.0), (1_000, 99.3), (1_000, 99.0)]);
    assert_eq!(fill_of(&deal(), &ticks, &params), None);
}

#[test]
fn replace_delay_holds_the_order_while_the_approach_is_shorter_than_it() {
    let params = MshotParams {
        replace_delay_s: 1.0,
        ..mshot()
    };
    // Approach at t=1000 (99.3), still there at t=1500: 500 ms < 1 s, the order is still at
    // 99 and a print at 99.0 fills it.
    let ticks = tape(&[(0, 100.0), (1_000, 99.3), (1_500, 99.3), (1_600, 99.0)]);
    let fill = fill_of(&deal(), &ticks, &params).expect("filled");
    assert_eq!(fill.t_ms, 1_600);
    // Held for 1 s: replaced at t=2000, effective at 2100, and the print at 2200 misses.
    let ticks = tape(&[(0, 100.0), (1_000, 99.3), (2_000, 99.3), (2_200, 99.0)]);
    assert_eq!(fill_of(&deal(), &ticks, &params), None);
}

#[test]
fn raise_wait_follows_the_price_up_only_after_the_wait() {
    let params = MshotParams {
        raise_wait_s: 1.0,
        ..mshot()
    };
    // Level 99 off 100. Price runs to 102 (distance 2.9 % > 1 %) at t=1000; before the wait
    // expires a spike to 99 at t=1500 fills the old order.
    let ticks = tape(&[(0, 100.0), (1_000, 102.0), (1_500, 99.0)]);
    assert!(fill_of(&deal(), &ticks, &params).is_some());
    // After the wait (t=2000) the order moves to 102 · 0.99 = 100.98 (effective 2100); a print
    // at 100.9 at t=2200 fills the NEW level.
    let ticks = tape(&[(0, 100.0), (1_000, 102.0), (2_000, 102.0), (2_200, 100.9)]);
    let fill = fill_of(&deal(), &ticks, &params).expect("filled at the new level");
    assert!((fill.price - 100.98).abs() < 1e-9, "{}", fill.price);
}

#[test]
fn leaving_and_re_entering_the_corridor_resets_the_wait() {
    let params = MshotParams {
        raise_wait_s: 1.0,
        ..mshot()
    };
    // Out at t=1000, back inside at t=1500 (100.5: distance 1.49 %… still out). Use 99.8:
    // distance 0.8 %, inside. Out again at t=1800; at t=2500 only 700 ms have passed since
    // the SECOND breach, so the order has not moved and 99.0 still fills.
    let ticks = tape(&[
        (0, 100.0),
        (1_000, 102.0),
        (1_500, 99.8),
        (1_800, 102.0),
        (2_500, 102.0),
        (2_600, 99.0),
    ]);
    let fill = fill_of(&deal(), &ticks, &params).expect("filled");
    assert!((fill.price - 99.0).abs() < 1e-9);
}

/// The corridor the core saves is symmetric around the placement: a run-away re-places the order
/// only past `2 · far − near` — 1.5 % on the 1 % / 0.5 % corridor — not past `far`.
#[test]
fn a_run_away_re_places_the_order_only_past_the_corridors_far_edge() {
    // Level 99 off 100. At 100.4 the order is 1.39 % off: inside the corridor, it stays, and
    // the spike to 99 fills it there.
    let ticks = tape(&[(0, 100.0), (1_000, 100.4), (2_000, 99.0)]);
    let fill = fill_of(&deal(), &ticks, &mshot()).expect("filled");
    assert!((fill.price - 99.0).abs() < 1e-9, "{fill:?}");
    // At 100.6 it is 1.59 % off: re-placed at 100.6 · 0.99 ≈ 99.594 (the tape's `f32` 100.6),
    // which the spike fills.
    let ticks = tape(&[(0, 100.0), (1_000, 100.6), (2_000, 99.5)]);
    let fill = fill_of(&deal(), &ticks, &mshot()).expect("filled");
    let re_placed = f64::from(100.6_f32) * 0.99;
    assert!((fill.price - re_placed).abs() < 1e-9, "{fill:?}");
}

// ---- entry: the order's whole life, from its creation --------------------------------------

/// A deal the core stamped with its order's creation at `created_ms`, the record proving the
/// order stood at the buy price from then on (`record::entry_placement`).
fn stamped(created_ms: i64) -> Deal {
    let d = deal();
    Deal {
        buy_set_ms: Some(created_ms),
        entry_placed: Some(d.buy_price),
        ..d
    }
}

/// With no archived line the order never moved: it stood at the buy price from its creation,
/// and reached the book a latency after it — a print at the level before then fills nothing.
/// (A 10 s replace delay keeps that print's approach from moving the order.) Unstamped, the same
/// tape places the order off its first print, on the book at once, and the same print fills it.
#[test]
fn a_stamped_order_stands_at_the_buy_price_from_its_creation() {
    let params = MshotParams {
        replace_delay_s: 10.0,
        ..mshot()
    };
    let ticks = tape(&[
        (0, 100.0),
        (1_000, 100.0),
        (2_050, 99.0),
        (3_000, 100.0),
        (9_000, 99.0),
    ]);
    let fill = fill_of(&stamped(2_000), &ticks, &params).expect("filled");
    assert_eq!((fill.t_ms, fill.price), (9_000, 99.0));
    let unstamped = fill_of(&deal(), &ticks, &params).expect("filled");
    assert_eq!((unstamped.t_ms, unstamped.price), (2_050, 99.0));
}

/// The placement is the record's: the order stands where it proves the core placed it, and from
/// then on the corridor is the model's own — the archived line is not replayed on top. Without a
/// proven placement, or with a tape that starts after the creation, the stamp changes nothing.
#[test]
fn a_stamped_order_starts_at_the_records_placement_or_not_at_all() {
    let ticks = tape(&[(0, 100.0), (2_500, 99.2), (3_000, 98.5)]);
    let placed = Deal {
        entry_placed: Some(98.5),
        ..stamped(2_000)
    };
    let fill = fill_of(&placed, &ticks, &mshot()).expect("filled");
    assert_eq!((fill.t_ms, fill.price), (3_000, 98.5));
    let unproven = Deal {
        entry_placed: None,
        ..stamped(2_000)
    };
    assert_eq!(
        fill_of(&unproven, &ticks, &mshot()),
        fill_of(&deal(), &ticks, &mshot())
    );
    let late_tape = tape(&[(2_500, 99.2), (3_000, 98.5)]);
    assert_eq!(
        fill_of(&placed, &late_tape, &mshot()),
        fill_of(&deal(), &late_tape, &mshot())
    );
}

/// A variant is placed at the creation off the reference the fact's level stood on, by its own
/// far bound: the fact at 99 on a 1 % bound stood off 100, so a 2 % variant stands at 98 and
/// fills on the deeper print. Without the fact's own parameters on the deal — the verdict's
/// case, which replays those very parameters — the order stands at the fact's level.
#[test]
fn a_variant_is_placed_at_the_creation_by_its_own_bound() {
    let mut d = stamped(2_000);
    d.own_entry = Some(EntryParams::MoonShot(mshot()));
    let deeper = MshotParams {
        price_pct: 2.0,
        ..mshot()
    };
    let ticks = tape(&[(0, 100.0), (2_500, 99.5), (3_000, 99.0), (4_000, 98.0)]);
    let fill = fill_of(&d, &ticks, &deeper).expect("filled deeper");
    assert_eq!((fill.t_ms, fill.price), (4_000, 98.0));
    let fact = fill_of(&stamped(2_000), &ticks, &deeper).expect("filled");
    assert_eq!((fact.t_ms, fact.price), (3_000, 99.0));
}

/// On a price grid the fact's level is its placement snapped away from the price, so the
/// reference is read back from half a step toward it: the fact at 99 on a 1-step grid stood off
/// 100 … 101, 100.5 at the middle, and a 1.3 % variant stands at 99.2 → 99 — not at the 98 a
/// reference read off the snapped 99 itself would give.
#[test]
fn a_variants_reference_is_read_back_from_the_middle_of_the_step() {
    let mut d = stamped(2_000);
    d.tick = Some(1.0);
    d.own_entry = Some(EntryParams::MoonShot(mshot()));
    let variant = MshotParams {
        price_pct: 1.3,
        ..mshot()
    };
    let ticks = tape(&[(0, 100.5), (2_500, 99.0)]);
    let fill = fill_of(&d, &ticks, &variant).expect("filled at the variant's level");
    assert_eq!((fill.t_ms, fill.price), (2_500, 99.0));
}

// ---- entry: modifiers, the price grid, the reference --------------------------------------

#[test]
fn delta_modifiers_deepen_a_pumping_coin_and_lift_a_falling_one() {
    // FAQ: MShotAdd3hDelta 0.05, MShotPrice 10, a coin up 20 % on 3 h -> -10 + (-20 * 0.05) =
    // -11, i.e. one per cent deeper.
    let params = MshotParams {
        price_pct: 10.0,
        price_min_pct: 7.0,
        modifiers: Modifiers {
            add_3h: 0.05,
            ..Modifiers::default()
        },
        ..mshot()
    };
    let pumping = Deltas {
        d3h: 20.0,
        ..Deltas::default()
    };
    let (near, far) = params.bounds_pct(&pumping);
    assert!(
        (near - 8.0).abs() < 1e-9 && (far - 11.0).abs() < 1e-9,
        "{near} {far}"
    );
    let falling = Deltas {
        d3h: -20.0,
        ..Deltas::default()
    };
    let (near, far) = params.bounds_pct(&falling);
    assert!(
        (near - 6.0).abs() < 1e-9 && (far - 9.0).abs() < 1e-9,
        "{near} {far}"
    );
}

#[test]
fn add_distance_scales_only_the_far_bound() {
    let params = MshotParams {
        price_pct: 10.0,
        price_min_pct: 7.0,
        modifiers: Modifiers {
            add_1h: 0.05,
            distance_pct: 100.0,
            ..Modifiers::default()
        },
        ..mshot()
    };
    let d = Deltas {
        d1h: 20.0,
        ..Deltas::default()
    };
    let (near, far) = params.bounds_pct(&d);
    assert!((near - 8.0).abs() < 1e-9, "{near}");
    assert!(
        (far - 12.0).abs() < 1e-9,
        "far gets 1 · (1 + 100/100) = 2: {far}"
    );
}

#[test]
fn price_bug_deepens_the_order() {
    let params = MshotParams {
        modifiers: Modifiers {
            add_pricebug: 0.2,
            ..Modifiers::default()
        },
        ..mshot()
    };
    let d = Deltas {
        pricebug: 2.0,
        ..Deltas::default()
    };
    let (_, far) = params.bounds_pct(&d);
    assert!((far - 1.4).abs() < 1e-9, "{far}");
}

#[test]
fn the_sell_family_reads_the_market_as_a_magnitude_and_the_corridor_with_its_sign() {
    // FAQ :1171, :1172 — AddMarketDelta / AddMarket24Delta "по модулю"; MShotAddMarketDelta has
    // no such word.
    let d = Deltas {
        market1h: -2.0,
        market24h: -3.0,
        ..Deltas::default()
    };
    let values: HashMap<String, String> = [
        ("AddMarketDelta", "0.1"),
        ("AddMarket24Delta", "0.1"),
        ("MShotAddMarketDelta", "0.1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let defaults = HashMap::new();
    let sv = StrategyValues {
        values: &values,
        defaults: &defaults,
    };
    let sell = exit_params(&sv).sell_mods;
    assert!((sell.near_addition(&d) - 0.5).abs() < 1e-9);
    let corridor = mshot_params(&sv, DEFAULT_LATENCY_MS).modifiers;
    assert!((corridor.near_addition(&d) - -0.2).abs() < 1e-9);
    assert!(param_keys().iter().any(|k| k == "AddMarket24Delta"));
}

#[test]
fn a_dump_cannot_push_the_bounds_below_the_floor_or_cross_them() {
    let params = MshotParams {
        price_pct: 1.0,
        price_min_pct: 0.5,
        modifiers: Modifiers {
            add_1h: 0.1,
            ..Modifiers::default()
        },
        ..mshot()
    };
    let d = Deltas {
        d1h: -50.0,
        ..Deltas::default()
    };
    let (near, far) = params.bounds_pct(&d);
    assert!(near > 0.0 && far >= near);
}

#[test]
fn the_level_snaps_to_the_price_grid_away_from_the_price() {
    let mut d = deal();
    d.tick = Some(0.05);
    // 100 · 0.99 = 99.0 exactly; use 100.07 → 99.0693 → floored to 99.05.
    let ticks = tape(&[(0, 100.07), (1_000, 99.05)]);
    let fill = fill_of(&d, &ticks, &mshot()).expect("filled");
    assert!((fill.price - 99.05).abs() < 1e-9, "{}", fill.price);
}

#[test]
fn minus_satoshi_keeps_two_steps_off_the_reference() {
    let mut d = deal();
    d.tick = Some(0.5);
    let params = MshotParams {
        price_pct: 0.1,
        price_min_pct: 0.05,
        minus_satoshi: true,
        ..mshot()
    };
    // 0.1 % of 100 is 0.1, under two steps (1.0): the order goes to 99.0.
    let ticks = tape(&[(0, 100.0), (1_000, 99.0)]);
    let fill = fill_of(&d, &ticks, &params).expect("filled");
    assert!((fill.price - 99.0).abs() < 1e-9, "{}", fill.price);
}

#[test]
fn an_ask_reference_follows_buy_side_prints_only() {
    let params = MshotParams {
        use_price: UsePrice::Ask,
        ..mshot()
    };
    // A BUY at 100 is the reference: level 99. A SELL at 101 must not move an ASK-referenced
    // order (the distance to 99 is still 1 % off the buy print), so 99.9 does not fill.
    let ticks = vec![
        tick(0, 100.0, Side::Buy),
        tick(1_000, 101.0, Side::Sell),
        tick(1_200, 99.9, Side::Sell),
    ];
    assert_eq!(
        fill_of(&deal(), &ticks, &params),
        None,
        "sell prints do not move an ASK order"
    );
    // The same 101 as a BUY is a retreat (1.98 % > 1 %): the order moves to 99.99 at once,
    // effective +100 ms, and the print at 99.9 fills the new level.
    let ticks = vec![
        tick(0, 100.0, Side::Buy),
        tick(1_000, 101.0, Side::Buy),
        tick(1_200, 99.9, Side::Sell),
    ];
    let fill = fill_of(&deal(), &ticks, &params).expect("moved off the buy print, then filled");
    assert!((fill.price - 99.99).abs() < 1e-9, "{}", fill.price);
}

#[test]
fn fast_algo_measures_the_corridor_from_the_extreme_print_of_the_last_100_ms() {
    // An 80 ms replace delay and no latency, so the move lands between prints and the two
    // references can be told apart.
    let params = MshotParams {
        fast_algo: true,
        raise_wait_s: 30.0,
        replace_delay_s: 0.08,
        latency_ms: 0.0,
        ..mshot()
    };
    let plain = MshotParams {
        fast_algo: false,
        ..params.clone()
    };
    // Level 99 off 100. A dip to 99.4 at t=1000 (0.40 % < 0.5 %: an approach) followed by 99.6
    // prints at t=1050 and t=1090. The plain reference is the last print, 99.6 → 0.60 %, inside
    // the corridor: the approach is forgotten and 99.0 at t=1100 fills. The fast reference is
    // the lowest print of the last 100 ms — still the 99.4 at t=1090 — so the approach has
    // held for 90 ms ≥ 80 ms and the order moves off 99 before the 99.0 arrives.
    let dip = tape(&[
        (0, 100.0),
        (1_000, 99.4),
        (1_050, 99.6),
        (1_090, 99.6),
        (1_100, 99.0),
    ]);
    assert!(
        fill_of(&deal(), &dip, &plain).is_some(),
        "plain: still at 99"
    );
    assert_eq!(
        fill_of(&deal(), &dip, &params),
        None,
        "fast: moved off the 99.4"
    );
    // The 99.4 falls out of the window after 100 ms: at t=1150 the reference is 99.6 again, the
    // approach is forgotten, and 99.0 fills.
    let back = tape(&[(0, 100.0), (1_000, 99.4), (1_150, 99.6), (1_200, 99.0)]);
    let fill = fill_of(&deal(), &back, &params).expect("still at 99 after the dip aged out");
    assert!((fill.price - 99.0).abs() < 1e-9);
    // Without a raise wait the fast algo is algorithm 1, which the model reads as the plain
    // last print.
    let algo1 = MshotParams {
        raise_wait_s: 0.0,
        ..params
    };
    assert!(fill_of(&deal(), &dip, &algo1).is_some());
}

#[test]
fn use_price_parses_the_strategy_spellings() {
    assert_eq!(UsePrice::parse("Trade"), UsePrice::Trade);
    assert_eq!(UsePrice::parse("ask"), UsePrice::Ask);
    assert_eq!(UsePrice::parse(" BID "), UsePrice::Bid);
    assert_eq!(UsePrice::parse("whatever"), UsePrice::Trade);
}

// ---- short: the mirror -------------------------------------------------------------------

#[test]
fn a_short_places_above_and_fills_on_a_spike_up() {
    let ticks = tape(&[(0, 100.0), (1_000, 101.0)]);
    let fill = fill_of(&short_deal(), &ticks, &mshot()).expect("filled");
    assert!((fill.price - 101.0).abs() < 1e-9);
    let ticks = tape(&[(0, 100.0), (1_000, 100.99)]);
    assert_eq!(fill_of(&short_deal(), &ticks, &mshot()), None);
}

#[test]
fn a_short_approach_is_a_rise_and_moves_the_order_up() {
    // Level 101 off 100; the price rises to 100.7 (distance 0.3 % < 0.5 %) → the order moves to
    // 101.707 after 100 ms; a print at 101.0 at +200 ms no longer fills.
    let ticks = tape(&[(0, 100.0), (1_000, 100.7), (1_200, 101.0)]);
    assert_eq!(fill_of(&short_deal(), &ticks, &mshot()), None);
}

// ---- exit: the take and the fallback -----------------------------------------------------

#[test]
fn the_take_is_sell_price_above_the_fill() {
    let exit = ExitParams {
        sell_price_pct: 1.0,
        ..ExitParams::default()
    };
    let fill = Fill {
        t_ms: 1_000,
        price: 99.0,
    };
    let ticks = tape(&[(0, 100.0), (1_000, 99.0), (2_000, 99.98), (3_000, 99.99)]);
    let out = ExitModel::new(&exit).exit(&deal(), &ticks, fill);
    assert_eq!(out.kind, ExitKind::Take);
    assert_eq!(out.t_ms, 3_000);
    assert!((out.price - 99.99).abs() < 1e-9);
}

#[test]
fn sell_at_last_price_lifts_the_take_to_the_pre_spike_price_less_the_adjustment() {
    // Pre-spike print (≥ 4 s before the fill): 100 at t=0; adjust 1 % → 99.0 … which is
    // below the plain take 99.99, so the plain one wins. With adjust 0 the pre-spike 100
    // wins over 99.99.
    let ticks = tape(&[(0, 100.0), (5_000, 99.0), (6_000, 99.995), (7_000, 100.0)]);
    let fill = Fill {
        t_ms: 5_000,
        price: 99.0,
    };
    let plain = ExitParams {
        sell_price_pct: 1.0,
        sell_at_last_price: true,
        sell_price_adjust_pct: 1.0,
        ..ExitParams::default()
    };
    let model = ExitModel::new(&plain);
    assert!((model.take_level(&deal(), &ticks, fill) - 99.99).abs() < 1e-9);
    let lifted = ExitParams {
        sell_price_adjust_pct: 0.0,
        ..plain
    };
    let model = ExitModel::new(&lifted);
    assert!((model.take_level(&deal(), &ticks, fill) - 100.0).abs() < 1e-9);
    let out = model.exit(&deal(), &ticks, fill);
    assert_eq!((out.kind, out.t_ms), (ExitKind::Take, 7_000));
}

#[test]
fn the_pre_spike_price_is_the_last_print_at_least_four_seconds_back() {
    let ticks = tape(&[(0, 100.0), (900, 100.5), (1_000, 101.0), (4_000, 95.0)]);
    let at = 5_000;
    assert_eq!(pre_spike_price(&ticks, at), Some(101.0));
    assert_eq!(pre_spike_price(&ticks, PRE_SPIKE_LOOKBACK_MS - 1), None);
}

#[test]
fn a_take_the_tape_never_reaches_leaves_the_position_open() {
    let fill = Fill {
        t_ms: 10_000,
        price: 99.0,
    };
    let ticks = tape(&[
        (9_000, 100.0),
        (10_000, 99.0),
        (20_000, 99.5),
        (25_000, 99.5),
    ]);
    let out = ExitModel::new(&ExitParams::default()).exit(&deal(), &ticks, fill);
    assert_eq!(out.kind, ExitKind::OpenAtWindowEnd);
    assert_eq!(out.t_ms, 25_000, "the tape's end");
}

#[test]
fn a_fill_after_the_fact_closed_is_open_at_the_window_end() {
    let fill = Fill {
        t_ms: 21_000,
        price: 99.0,
    };
    let ticks = tape(&[(9_000, 100.0), (21_000, 99.0), (25_000, 99.5)]);
    let out = ExitModel::new(&ExitParams::default()).exit(&deal(), &ticks, fill);
    assert_eq!(out.kind, ExitKind::OpenAtWindowEnd);
}

#[test]
fn sell_delay_arms_the_take_late() {
    let exit = ExitParams {
        sell_delay_ms: 500.0,
        ..ExitParams::default()
    };
    let fill = Fill {
        t_ms: 1_000,
        price: 99.0,
    };
    // 100.5 at +300 ms is inside the delay; 100.2 at +700 ms is the exit.
    let ticks = tape(&[
        (1_000, 99.0),
        (1_300, 100.5),
        (1_700, 100.2),
        (20_000, 99.0),
    ]);
    let out = ExitModel::new(&exit).exit(&deal(), &ticks, fill);
    assert_eq!((out.kind, out.t_ms), (ExitKind::Take, 1_700));
}

#[test]
fn a_short_take_is_below_the_fill() {
    let fill = Fill {
        t_ms: 1_000,
        price: 101.0,
    };
    let ticks = tape(&[(1_000, 101.0), (2_000, 100.0), (3_000, 99.9)]);
    let out = ExitModel::new(&ExitParams::default()).exit(&short_deal(), &ticks, fill);
    assert_eq!((out.kind, out.t_ms), (ExitKind::Take, 3_000));
    assert!((out.price - 99.99).abs() < 1e-9);
}

// ---- simulate: the whole trade -----------------------------------------------------------

#[test]
fn simulate_chains_entry_and_exit_and_signs_the_result() {
    let ticks = tape(&[(0, 100.0), (1_000, 99.0), (2_000, 100.0)]);
    let out = simulate(
        &deal(),
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
    );
    let pct = out.profit_pct.expect("a trade");
    assert!((pct - 1.0).abs() < 1e-9, "{pct}");
    assert!((out.profit_money(&deal()).unwrap() - 10.0).abs() < 1e-9);

    let short = simulate(
        &short_deal(),
        &tape(&[(0, 100.0), (1_000, 101.0), (2_000, 99.9)]),
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
    );
    assert!((short.profit_pct.unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn a_degenerate_price_is_no_trade_not_a_break_even_one() {
    assert_eq!(profit_pct(&deal(), 0.0, 100.0), None);
    assert_eq!(profit_pct(&deal(), 99.0, f64::NAN), None);
    assert_eq!(profit_pct(&deal(), -1.0, 100.0), None);
    assert!((profit_pct(&deal(), 100.0, 101.0).unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn a_fact_entry_uses_the_report_fill() {
    let ticks = tape(&[(9_000, 100.0), (10_000, 99.0), (11_000, 100.5)]);
    let out = simulate(
        &deal(),
        &ticks,
        &EntryParams::Fact,
        &ExitParams::default(),
        None,
    );
    assert_eq!(
        out.fill,
        Some(Fill {
            t_ms: 10_000,
            price: 99.0
        })
    );
    assert_eq!(out.exit.map(|e| e.kind), Some(ExitKind::Take));
}

#[test]
fn an_unfilled_variant_is_not_a_trade() {
    let ticks = tape(&[(0, 100.0), (1_000, 99.5)]);
    let out = simulate(
        &deal(),
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
    );
    assert!(!out.is_trade());
    assert_eq!(out.profit_money(&deal()), None);
}

// ---- verify: reproducing the fact --------------------------------------------------------

#[test]
fn verify_marks_an_entry_inside_the_tolerance() {
    // Fact buy at 99.0; the model fills at 99.0 exactly → ✓, and the take at 99.99 → ✓ against
    // a fact sell of 100.0? No: 0.01 % off is inside 0.05 % → ✓.
    let ticks = tape(&[(0, 100.0), (10_000, 99.0), (20_000, 100.0)]);
    let v = verify(
        &deal(),
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.entry, Some(true));
    assert_eq!(v.exit, Some(true));
    assert!(v.exit_dev_pct.unwrap().abs() < 0.05);
}

#[test]
fn verify_marks_a_missed_entry_and_judges_the_exit_from_the_fact() {
    let ticks = tape(&[(0, 100.0), (10_000, 99.5), (20_000, 100.0)]);
    let v = verify(
        &deal(),
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.entry, Some(false));
    assert_eq!(v.fill, None);
    // The exit is judged from the factual entry, so a missed entry does not silence it: the
    // take off the fact's 99.0 is reached at t=20000.
    assert_eq!(v.exit, Some(true));

    // Fact entry, and no print reaches the take: the line still STOOD at 99.99 when the core
    // sold at 100.0 — which print would have filled it is the queue's business, not the
    // verdict's — so the exit is reproduced.
    let ticks = tape(&[
        (9_000, 100.0),
        (10_000, 99.0),
        (20_000, 99.5),
        (25_000, 99.5),
    ]);
    let v = verify(
        &deal(),
        &ticks,
        &EntryParams::Fact,
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.entry, None);
    assert_eq!(v.exit, Some(true));
    assert_eq!(v.exit_kind, Some(ExitKind::Take));
    // A sell delay that outlives the trade leaves no line at the close: a miss.
    let late = ExitParams {
        sell_delay_ms: 30_000.0,
        ..ExitParams::default()
    };
    let v = verify(&deal(), &ticks, &EntryParams::Fact, &late, None, None);
    assert_eq!(v.exit, Some(false));
    assert_eq!(v.exit_kind, Some(ExitKind::OpenAtWindowEnd));
}

#[test]
fn verify_reports_the_deviation_of_an_entry_off_the_fact() {
    // The 1 % / 0.5 % corridor is 0.5 % wide: a fill 1.02 % off the fact is another order.
    let mut d = deal();
    d.buy_price = 98.0;
    let ticks = tape(&[(0, 100.0), (10_000, 99.0)]);
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.entry, Some(false));
    let dev = v.entry_dev_pct.unwrap();
    assert!((dev - 99.0 / 98.0 * 100.0 + 100.0).abs() < 1e-6, "{dev}");
}

#[test]
fn verify_holds_the_entry_to_the_corridors_width() {
    // The same 0.5 %-wide corridor: a fill 0.3 % off the fact is the same order re-placed off
    // a neighbouring print, and passes; the floor is the 0.05 % step for a corridor narrower
    // than it.
    let mut d = deal();
    d.buy_price = 99.0 / 1.003;
    let ticks = tape(&[(0, 100.0), (10_000, 99.0), (20_000, 100.0)]);
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.entry, Some(true), "{:?}", v.entry_dev_pct);
    assert!((verify::entry_tolerance_pct(&mshot(), &d) - 0.5).abs() < 1e-9);
    let narrow = MshotParams {
        price_pct: 1.0,
        price_min_pct: 0.99,
        ..mshot()
    };
    assert!((verify::entry_tolerance_pct(&narrow, &d) - 0.05).abs() < 1e-9);
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(narrow),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.entry, Some(false), "0.3 % off on a 0.01 % corridor");
}

#[test]
fn verify_leaves_a_take_unanswered_against_a_fact_another_rule_closed() {
    // The tape reaches the take, but the core closed by Auto Price Down: not the same rule,
    // so the exit neither hits nor misses.
    let mut d = deal();
    d.sell_reason = "Auto Price Down".into();
    let ticks = tape(&[(0, 100.0), (10_000, 99.0), (20_000, 100.0)]);
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.exit_kind, Some(ExitKind::Take));
    assert_eq!(v.exit, None);
    assert_eq!(v.exit_dev_pct, None);
}

#[test]
fn verify_takes_a_limits_better_fill_and_ignores_the_archived_fill_point() {
    // Fact: buy 99.0, a 1 % take at 99.99 placed and never moved, sold at 100.2 — a gap fill
    // 0.21 % ABOVE the limit. The archive files the take and then the fill itself at the
    // close; the fill is not a move the model has to make.
    let mut d = deal();
    d.sell_price = 100.2;
    d.close_ms = 20_000;
    let ticks = tape(&[(0, 100.0), (10_000, 99.0), (20_000, 100.2)]);
    let archived = [
        (10_000, 99.99),
        (20_000, 99.99),
        (19_990, 100.2),
        (20_000, 100.2),
    ];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        Some(&archived),
    );
    assert_eq!(v.exit_kind, Some(ExitKind::Take));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert_eq!(v.line_points, Some((1, 1)), "the fill point is not a move");
    // The same better fill without the archive to say the line was the same line: not taken.
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(
        v.exit,
        Some(false),
        "a better fill needs the archive behind it"
    );
    // A fact far beyond the level is another exit, not a better fill of this one — with the
    // archive corroborating the line, so it is the bound that refuses it.
    d.sell_price = 100.5;
    let ticks = tape(&[(0, 100.0), (10_000, 99.0), (20_000, 100.5)]);
    let archived = [
        (10_000, 99.99),
        (20_000, 99.99),
        (19_990, 100.5),
        (20_000, 100.5),
    ];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        Some(&archived),
    );
    assert_eq!(v.line_points, Some((1, 1)));
    assert_eq!(v.exit, Some(false), "0.51 % beyond the level, {v:?}");
    // Worse than the level is never a fill of it, archive or not — the sign, not the bound.
    d.sell_price = 99.9;
    let archived = [
        (10_000, 99.99),
        (20_000, 99.99),
        (19_990, 99.9),
        (20_000, 99.9),
    ];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        Some(&archived),
    );
    assert_eq!(v.line_points, Some((1, 1)));
    assert_eq!(v.exit, Some(false), "{v:?}");
}

/// A level placed THROUGH the market is taken at once by the book: the archive files the fill
/// a moment after the move, at the sale price and better than the level, and up to a second
/// before `closedatems` — the report books the close later. The fill is not a move, and the
/// improvement is the book's, not another exit's.
#[test]
fn verify_takes_a_level_placed_through_the_market() {
    let mut d = deal();
    d.sell_price = 100.5;
    d.close_ms = 10_500;
    let ticks = tape(&[(0, 100.0), (10_000, 99.0), (10_030, 100.5)]);
    // The take at 99.99 placed at the fill; the core's fill point 30 ms later at 100.5, 470 ms
    // before the close — past the latency window the close stamp alone allowed.
    let archived = [(10_000, 99.99), (10_030, 100.5)];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::Fact,
        &ExitParams::default(),
        None,
        Some(&archived),
    );
    assert_eq!(v.line_points, Some((1, 1)), "the fill point is not a move");
    assert_eq!(v.exit, Some(true), "{v:?}");
    // Worse than the level it follows is never its fill: a move the model did not make.
    d.sell_price = 99.5;
    let archived = [(10_000, 99.99), (10_030, 99.5)];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::Fact,
        &ExitParams::default(),
        None,
        Some(&archived),
    );
    assert_eq!(v.line_points, Some((1, 2)));
    assert_eq!(v.exit, Some(false), "{v:?}");
}

#[test]
fn share_counts_only_answered_verdicts() {
    assert_eq!(share([Some(true), None, Some(false), Some(true)]), (2, 3));
    assert_eq!(share([None, None]), (0, 0));
}

// ---- parameters out of a strategy ----------------------------------------------------------

fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn mshot_params_read_the_strategy_then_the_schema_then_the_model_default() {
    let v = values(&[
        ("MShotPrice", "1.4"),
        ("MShotPriceMin", "1,1"),
        ("MShotUsePrice", "Trade"),
        ("MShotMinusSatoshi", "YES"),
        ("MShotAdd3hDelta", "0.0005"),
        ("MShotAddDistance", "50"),
    ]);
    let defaults: HashMap<String, f64> = [("mshotreplacedelay".to_string(), 0.3)].into();
    let p = mshot_params(
        &StrategyValues {
            values: &v,
            defaults: &defaults,
        },
        DEFAULT_LATENCY_MS,
    );
    assert!((p.price_pct - 1.4).abs() < 1e-9);
    assert!(
        (p.price_min_pct - 1.1).abs() < 1e-9,
        "a comma decimal parses"
    );
    assert!(p.minus_satoshi);
    assert!(
        (p.replace_delay_s - 0.3).abs() < 1e-9,
        "the schema default fills a missing key"
    );
    assert!(
        (p.raise_wait_s - 0.0).abs() < 1e-9,
        "the model default fills the rest"
    );
    assert!((p.modifiers.add_3h - 0.0005).abs() < 1e-12);
    assert!((p.modifiers.distance_pct - 50.0).abs() < 1e-9);
}

#[test]
fn exit_params_read_the_sell_fields() {
    let v = values(&[
        ("SellPrice", "0.8%"),
        ("MShotSellAtLastPrice", "NO"),
        ("MShotSellPriceAdjust", "1"),
    ]);
    let defaults = HashMap::new();
    let p = exit_params(&StrategyValues {
        values: &v,
        defaults: &defaults,
    });
    assert!((p.sell_price_pct - 0.8).abs() < 1e-9);
    assert!(!p.sell_at_last_price);
    assert!((p.sell_price_adjust_pct - 1.0).abs() < 1e-9);
}

/// The stop's switch and trigger: `StopLoss` stays in the dump of a strategy whose stop is off,
/// and a dump without `FastStopLoss` is at the core's default — the book-watching stop.
#[test]
fn exit_params_read_the_stop_switch_and_trigger() {
    let defaults = HashMap::new();
    let read = |pairs: &[(&str, &str)]| {
        let v = values(pairs);
        exit_params(&StrategyValues {
            values: &v,
            defaults: &defaults,
        })
    };
    let off = read(&[("UseStopLoss", "NO"), ("StopLoss", "-2")]);
    assert_eq!(off.stop_loss_pct, 0.0, "a switched-off stop arms nothing");
    let on = read(&[
        ("UseStopLoss", "YES"),
        ("StopLoss", "-2"),
        ("StopLossEMA", "3"),
    ]);
    assert_eq!(on.stop_loss_pct, -2.0);
    assert!(!on.fast_stop_loss, "absent is the core default, NO");
    assert_eq!(on.stop_loss_ema, 3.0);
    let unswitched = read(&[("StopLoss", "-2"), ("FastStopLoss", "YES")]);
    assert_eq!(unswitched.stop_loss_pct, -2.0, "no switch keeps the stop");
    assert!(unswitched.fast_stop_loss);
}

#[test]
fn the_stated_stop_level_is_read_only_when_it_is_a_usable_price() {
    use super::verify::stated_stop_level;
    let reason = "StopLoss AutoActivated on price drop: BID = 0.025326 ASK: 0.025999 \
                  (strategy <HookTestN1>); StopLoss fixed: 0.025334 Allow";
    assert_eq!(stated_stop_level(reason), Some(0.025334));
    // A zero, or a level printed too coarsely for its price.
    assert_eq!(
        stated_stop_level("StopLoss fixed: 0.00000 AllowedDrop"),
        None
    );
    assert_eq!(
        stated_stop_level("StopLoss fixed: 0.00012 AllowedDrop"),
        None
    );
    // Cut off by the column's length — live reasons end on `0.` — the digits are incomplete.
    assert_eq!(stated_stop_level("StopLoss fixed: 0."), None);
    assert_eq!(stated_stop_level("StopLoss fixed: 0.0253"), None);
    assert_eq!(stated_stop_level("StopLoss Market Sell"), None);
}

#[test]
fn the_descriptor_keys_every_field_the_builders_read_and_splits_the_groups() {
    let keys = param_keys();
    for key in [
        "MShotPrice",
        "MShotPriceMin",
        "MShotUsePrice",
        "MShotRaiseWait",
        "MShotReplaceDelay",
        "MShotMinusSatoshi",
        "MShotAddHourlyDelta",
        "MShotAddPriceBug",
        "MShotAddDistance",
        "SellPrice",
        "MShotSellAtLastPrice",
        "MShotSellPriceAdjust",
        "SellDelay",
        // Read by the builders, not shown in the grid — and just as fatal when unfetched: an
        // absent key reads as the model's fallback, silently.
        "HookSellLevel",
        "HookSellFixed",
        "SellModifier",
        "MaxModifier",
        "Add5minDelta",
        "AddHourlyDelta",
        "AddBTC1mDelta",
    ] {
        assert!(
            keys.iter().any(|k| k == key),
            "{key} missing from the keys the models read"
        );
    }
    // The allowlist above cannot see a field that is missing from BOTH lists, which is exactly
    // how `SellShotPriceDown`/`SellShotPriceDownDelay` ran on their fallback from the day the
    // axis was written. So the builders' own source is the authority: every strategy field
    // `mshot_params`/`exit_params` reads must be a key the load asks the database for.
    let source = include_str!("params.rs");
    for call in [".num(\"", ".bool(\"", ".text(\""] {
        let mut rest = source;
        while let Some(at) = rest.find(call) {
            rest = &rest[at + call.len()..];
            let key = &rest[..rest.find('"').expect("a closing quote")];
            assert!(
                keys.iter().any(|k| k == key),
                "{key} is read by a builder but never fetched: add it to TICK_PARAMS or to                  MODEL_ONLY_KEYS, or it silently reads as the model's fallback"
            );
        }
    }
    assert!(params_for(ParamGroup::Entry, "Spread").next().is_none());
    assert!(params_for(ParamGroup::Entry, "MoonShot").count() > 10);
    let exit_any: Vec<_> = params_for(ParamGroup::Exit, "PumpsDetection")
        .map(|p| p.key)
        .collect();
    assert!(exit_any.starts_with(&["SellPrice", "SellDelay", "PriceDownTimer"]));
    // A Spread's take is the spread it detected, not `SellPrice` (`exit::take_is_recorded`).
    let spread: Vec<_> = params_for(ParamGroup::Exit, "Spread")
        .map(|p| p.key)
        .collect();
    assert!(spread.starts_with(&["SellDelay", "PriceDownTimer"]));
    assert!(
        !exit_any.contains(&"MShotSellAtLastPrice"),
        "a MoonShot-only field"
    );
    assert!(exit_any.contains(&"StopLoss") && exit_any.contains(&"SellShotDistance"));
    assert!(entry_model_for("MoonShot") && !entry_model_for("Spread"));
}

// ---- the price step off the tape -----------------------------------------------------------

#[test]
fn infer_tick_reads_the_grid_and_snaps_float_noise() {
    let ticks = tape(&[(0, 1.2345), (1, 1.2346), (2, 1.2349), (3, 1.2346)]);
    let step = infer_tick(&ticks).expect("a step");
    assert!((step - 0.0001).abs() < 1e-12, "{step}");
    assert_eq!(infer_tick(&tape(&[(0, 1.0), (1, 1.0)])), None);
    assert_eq!(infer_tick(&[]), None);
}

// ---- MoonHook: the take is a share of the detect depth, not `SellPrice` -------------------

/// A hook deal: 4 % detect depth, bought at 100, the core's own take stated at 2 %.
fn hook_deal() -> Deal {
    Deal {
        kind: KIND_MOONHOOK.into(),
        buy_price: 100.0,
        hook_depth_pct: Some(4.0),
        hook_stated_take_pct: Some(2.0),
        step_lag_ms: 0.0,
        stop_anchor: None,
        delta_track: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        ..deal()
    }
}

/// `HookSellLevel` = 50 of a 4 % depth is a 2 % take — and `SellPrice` is not consulted at all.
#[test]
fn a_hook_takes_a_share_of_its_detect_depth() {
    let params = ExitParams {
        sell_price_pct: 1.0,
        hook_sell_level_pct: 50.0,
        ..ExitParams::default()
    };
    let fill = Fill {
        t_ms: 10_000,
        price: 100.0,
    };
    let take = ExitModel::new(&params).take_level(&hook_deal(), &[], fill);
    assert!((take - 102.0).abs() < 1e-9, "{take}");
    // The level scales with the parameter — that is what makes it searchable.
    let doubled = ExitParams {
        hook_sell_level_pct: 100.0,
        ..params.clone()
    };
    let take = ExitModel::new(&doubled).take_level(&hook_deal(), &[], fill);
    assert!((take - 104.0).abs() < 1e-9, "{take}");
}

/// A short hook sells below the entry, by the same share.
#[test]
fn a_short_hook_takes_below_the_entry() {
    let params = ExitParams {
        hook_sell_level_pct: 50.0,
        ..ExitParams::default()
    };
    let d = Deal {
        is_short: true,
        ..hook_deal()
    };
    let fill = Fill {
        t_ms: 10_000,
        price: 100.0,
    };
    let take = ExitModel::new(&params).take_level(&d, &[], fill);
    assert!((take - 98.0).abs() < 1e-9, "{take}");
}

/// Without a depth (or without a level) the rule cannot be computed — the model still needs a
/// line to walk, so it falls back, but it must SAY that it does not know.
#[test]
fn a_hook_without_its_depth_is_not_a_known_take() {
    let params = ExitParams {
        hook_sell_level_pct: 50.0,
        ..ExitParams::default()
    };
    let model = ExitModel::new(&params);
    assert!(model.take_known(&hook_deal()));
    let no_depth = Deal {
        hook_depth_pct: None,
        ..hook_deal()
    };
    assert!(!model.take_known(&no_depth));
    let no_level = ExitParams {
        hook_sell_level_pct: 0.0,
        ..params.clone()
    };
    assert!(!ExitModel::new(&no_level).take_known(&hook_deal()));
    // `HookSellFixed` is the other branch of the rule, and it is not modelled.
    let fixed = ExitParams {
        hook_sell_fixed: true,
        ..params.clone()
    };
    assert!(!ExitModel::new(&fixed).take_known(&hook_deal()));
    // The kinds that take by `SellPrice` have it — with or without an archive.
    assert!(model.take_known(&deal()), "MoonShot");
    for kind in ["PumpsDetection", "Combo"] {
        let d = Deal {
            kind: kind.into(),
            ..deal()
        };
        assert!(model.take_known(&d), "{kind} takes by SellPrice");
    }
    // A variant runs the hook's FORMULA: the level the core recorded is the fact's answer, and
    // a variant of a hook whose depth is unknown has nowhere to put its take.
    assert!(!model.take_known(&Deal {
        archived_take: Some(101.0),
        ..no_depth
    }));
}

/// The take of a Spread is the spread it detected, a level the core recorded, not a rule: the
/// record or nothing, for the fact and for every variant — and never `SellPrice`.
#[test]
fn a_spread_takes_the_level_its_core_recorded() {
    let spread = Deal {
        kind: "Spread".into(),
        archived_take: Some(102.3),
        ..deal()
    };
    let far = ExitParams {
        sell_price_pct: 5.0,
        ..ExitParams::default()
    };
    let model = ExitModel::new(&far);
    let fill = Fill {
        t_ms: 10_000,
        price: 100.0,
    };
    assert!(model.take_known(&spread));
    assert!((model.take_level(&spread, &[], fill) - 102.3).abs() < 1e-9);
    let unrecorded = Deal {
        archived_take: None,
        ..spread
    };
    assert!(!model.take_known(&unrecorded));
}

/// A MoonShot lifted to the pre-spike ask places its take off the ask the core's record gives
/// back; a variant with nothing but the tape's print would place it lower, and on a stopped
/// trade sell there before the stop (30 of 88 live, 2026-09-23).
#[test]
fn a_moonshot_lifted_to_the_ask_needs_the_recorded_ask() {
    let lifted = ExitParams {
        sell_at_last_price: true,
        sell_price_adjust_pct: 0.1,
        ..ExitParams::default()
    };
    let model = ExitModel::new(&lifted);
    assert!(!model.take_known(&deal()));
    assert!(model.take_known(&Deal {
        pre_spike_ask: Some(103.0),
        ..deal()
    }));
    // Without the lift the take is `SellPrice`, known either way.
    assert!(ExitModel::new(&ExitParams::default()).take_known(&deal()));
}

/// A modifier deep enough to drive the distance negative must not put the take on the losing
/// side of the entry — the line steps DOWN from the take, and a take below the fill inverts it.
#[test]
fn a_negative_modifier_cannot_push_the_take_through_the_fill() {
    let mut mods = Modifiers::default();
    mods.add_1h = 1.0;
    let params = ExitParams {
        sell_price_pct: 1.0,
        sell_modifier: 1.0,
        sell_mods: mods,
        ..ExitParams::default()
    };
    let d = Deal {
        deltas: Deltas {
            d1h: -50.0,
            ..Deltas::default()
        },
        ..deal()
    };
    let fill = Fill {
        t_ms: 10_000,
        price: 100.0,
    };
    let take = ExitModel::new(&params).take_level(&d, &[], fill);
    assert!(
        (take - 100.0).abs() < 1e-9,
        "floored at the fill, got {take}"
    );
}

/// The grid must not offer a knob that moves nothing: `SellPrice` is not a MoonHook's take.
#[test]
fn the_grid_hides_sell_price_from_a_hook_and_offers_its_own_level() {
    let hook: Vec<&str> = params_for(ParamGroup::Exit, KIND_MOONHOOK)
        .map(|p| p.key)
        .collect();
    assert!(!hook.contains(&"SellPrice"), "the hook has no such field");
    assert!(hook.contains(&"HookSellLevel"));
    assert!(
        !hook.contains(&"HookSellFixed"),
        "read, but not modelled — so not a knob"
    );
    let pump: Vec<&str> = params_for(ParamGroup::Exit, "PumpsDetection")
        .map(|p| p.key)
        .collect();
    assert!(pump.contains(&"SellPrice"));
    assert!(!pump.contains(&"HookSellLevel"), "a hook-only field");
    // Nor is `SellPrice` a Spread's take: the core places it on the spread it detected.
    let spread: Vec<&str> = params_for(ParamGroup::Exit, "Spread")
        .map(|p| p.key)
        .collect();
    assert!(!spread.contains(&"SellPrice"));
}

/// The verdict on a take it cannot place is nothing, not a miss — the whole point of the
/// exercise: data we hold must not be filed as "the model was wrong".
#[test]
fn an_unknown_take_leaves_the_exit_unanswered() {
    let ticks = tape(&[(10_000, 100.0), (15_000, 101.0), (20_000, 102.0)]);
    let params = ExitParams {
        hook_sell_level_pct: 50.0,
        take_from_archive: true,
        ..ExitParams::default()
    };
    let known = verify(
        &hook_deal(),
        &ticks,
        &EntryParams::Fact,
        &params,
        None,
        None,
    );
    assert!(
        known.exit.is_some(),
        "a depth is a level the model can place"
    );
    let blind = Deal {
        hook_depth_pct: None,
        ..hook_deal()
    };
    let v = verify(&blind, &ticks, &EntryParams::Fact, &params, None, None);
    assert_eq!(v.exit, None, "no level, no verdict");
    assert_eq!(v.exit_dev_pct, None);
    // A stopped trade is no exception: its stop may fire right, but a variant of it walks a line
    // off a take it cannot place, and on live trades sold there before the stop (2026-09-23,
    // 30 of 88 stopped MoonShot trades) — not a trade the search can run.
    let stopped = Deal {
        sell_reason: "StopLoss Market Sell".into(),
        sell_price: 97.0,
        ..blind.clone()
    };
    let stop_params = ExitParams {
        stop_loss_pct: -2.0,
        ..params.clone()
    };
    let down = tape(&[(10_000, 100.0), (15_000, 97.9), (20_000, 97.0)]);
    let v = verify(
        &stopped,
        &down,
        &EntryParams::Fact,
        &stop_params,
        None,
        None,
    );
    assert_eq!(
        v.exit, None,
        "a stop on an unknown take is still an unknown take"
    );
    // With the depth the take is placeable and the stop is judged as a stop.
    let v = verify(
        &Deal {
            hook_depth_pct: hook_deal().hook_depth_pct,
            ..stopped
        },
        &down,
        &EntryParams::Fact,
        &stop_params,
        None,
        None,
    );
    assert!(v.exit.is_some(), "{v:?}");
}

// ---- the stop and its modifier -------------------------------------------------------------

/// The core's FAQ spells the stop's adjustment as `StopLoss adjusted [-1.00% - (10.00*0.98=9.75%)
/// => -10.75%]`: the configured stop, deepened by `StopLossModifier · Σ`.
#[test]
fn the_stop_modifier_deepens_the_stop_by_the_summed_deltas() {
    let mut mods = Modifiers::default();
    mods.add_1h = 1.0;
    let params = ExitParams {
        stop_loss_pct: -2.0,
        stop_loss_modifier: 0.2,
        sell_mods: mods,
        ..ExitParams::default()
    };
    let d = Deal {
        deltas: Deltas {
            d1h: 1.86,
            ..Deltas::default()
        },
        ..deal()
    };
    let pct = moon_core_stop_pct(&params, &d, d.buy_ms);
    assert!((pct - -2.372).abs() < 1e-9, "{pct}");
    // No coefficient, no movement; no stop, nothing to move.
    let off = ExitParams {
        stop_loss_modifier: 0.0,
        ..params.clone()
    };
    assert_eq!(moon_core_stop_pct(&off, &d, d.buy_ms), -2.0);
    let no_stop = ExitParams {
        stop_loss_pct: 0.0,
        ..params.clone()
    };
    assert_eq!(moon_core_stop_pct(&no_stop, &d, d.buy_ms), 0.0);
    // `MaxModifier` caps the sum before the coefficient, as it does for the sell.
    let capped = ExitParams {
        max_modifier: 1.0,
        ..params
    };
    assert!((moon_core_stop_pct(&capped, &d, d.buy_ms) - -2.2).abs() < 1e-9);
}

/// The adjustment may pull the stop toward the entry — live strategies carry a negative
/// `StopLossModifier` — but one that pulls it THROUGH the entry leaves no stop at all, rather
/// than one a hair from the entry that the next print would trip.
#[test]
fn an_adjustment_through_the_entry_leaves_no_stop() {
    let mut mods = Modifiers::default();
    mods.add_1h = 1.0;
    let base = ExitParams {
        stop_loss_pct: -2.0,
        stop_loss_modifier: -0.3,
        sell_mods: mods,
        ..ExitParams::default()
    };
    let far = Deal {
        deltas: Deltas {
            d1h: 70.0,
            ..Deltas::default()
        },
        ..deal()
    };
    // −2 − 70·(−0.3) = +19 unguarded: a "stop" nineteen per cent in profit.
    assert_eq!(
        moon_core_stop_pct(&base, &far, far.buy_ms),
        0.0,
        "no stop, not a near one"
    );
    // A negative delta sum with a positive coefficient reaches the same place from the other
    // side.
    let other = ExitParams {
        stop_loss_modifier: 0.3,
        ..base.clone()
    };
    let down = Deal {
        deltas: Deltas {
            d1h: -70.0,
            ..Deltas::default()
        },
        ..deal()
    };
    assert_eq!(moon_core_stop_pct(&other, &down, down.buy_ms), 0.0);
    // A modifier that only moves the stop within its own side is applied as it is.
    let mild = Deal {
        deltas: Deltas {
            d1h: 2.0,
            ..Deltas::default()
        },
        ..deal()
    };
    assert!((moon_core_stop_pct(&base, &mild, mild.buy_ms) - -1.4).abs() < 1e-9);
    // A stop the strategy itself put on the profit side stays where it put it — that is its own
    // setting, not something the adjustment did.
    let positive = ExitParams {
        stop_loss_pct: 1.0,
        stop_loss_modifier: 0.3,
        ..base.clone()
    };
    let up = Deal {
        deltas: Deltas {
            d1h: 2.0,
            ..Deltas::default()
        },
        ..deal()
    };
    assert!((moon_core_stop_pct(&positive, &up, up.buy_ms) - 0.4).abs() < 1e-9);
}

/// An adjustment that exactly cancels the stop must not leave one armed at the fill price,
/// where the next print fires it.
#[test]
fn a_cancelled_stop_does_not_fire_at_the_entry() {
    let mut mods = Modifiers::default();
    mods.add_1h = 1.0;
    let params = ExitParams {
        stop_loss_pct: -2.0,
        stop_loss_modifier: 0.2,
        sell_price_pct: 5.0,
        sell_mods: mods,
        ..ExitParams::default()
    };
    // Σ = −10, so −2 − (−10·0.2) = 0 exactly without the clamp.
    let d = Deal {
        deltas: Deltas {
            d1h: -10.0,
            ..Deltas::default()
        },
        ..deal()
    };
    // The adjustment cancels the stop exactly, so there is none — and the print below the
    // entry must not read as one.
    assert_eq!(moon_core_stop_pct(&params, &d, d.buy_ms), 0.0);
    let walk = ExitModel::new(&params).walk(
        &d,
        // A real move down, not float noise: without the guard the stop sits ON the entry and
        // this print trips it.
        &tape(&[(10_000, 100.0), (11_000, 99.99), (12_000, 100.02)]),
        Fill {
            t_ms: 10_000,
            price: 100.0,
        },
    );
    assert_ne!(
        walk.exit.kind,
        ExitKind::Stop,
        "a print a hundredth of a per cent away is not a stop: {:?}",
        walk.exit
    );
}

/// A short's stop sits ABOVE the entry, and the same distance mirrors there.
#[test]
fn a_short_stop_mirrors_with_the_modifier() {
    let mut mods = Modifiers::default();
    mods.add_1h = 1.0;
    let params = ExitParams {
        stop_loss_pct: -2.0,
        stop_loss_modifier: 0.2,
        sell_mods: mods,
        ..ExitParams::default()
    };
    let d = Deal {
        deltas: Deltas {
            d1h: 5.0,
            ..Deltas::default()
        },
        ..short_deal()
    };
    // −2 − 0.2·5 = −3 per cent, and a short's stop is that far ABOVE the fill.
    let walk = ExitModel::new(&params).walk(
        &d,
        &tape(&[(10_000, 100.0), (15_000, 103.5)]),
        Fill {
            t_ms: 10_000,
            price: 100.0,
        },
    );
    assert_eq!(walk.exit.kind, ExitKind::Stop, "the price crossed 103");
    assert!((walk.exit.price - 103.5).abs() < 1e-9, "{:?}", walk.exit);
}

// ---- the sell-side delta modifiers ---------------------------------------------------------

/// FAQ: a summed delta of 5 % with `SellModifier = 0.2` places the sell 1 % higher.
#[test]
fn sell_modifiers_lift_the_take_by_the_faq_example() {
    let mut mods = Modifiers::default();
    mods.add_1h = 1.0;
    let params = ExitParams {
        sell_price_pct: 1.0,
        sell_modifier: 0.2,
        sell_mods: mods,
        ..ExitParams::default()
    };
    let d = Deal {
        deltas: Deltas {
            d1h: 5.0,
            ..Deltas::default()
        },
        ..deal()
    };
    let fill = Fill {
        t_ms: 10_000,
        price: 100.0,
    };
    // 1 % of SellPrice plus 5 % * 0.2 = 2 % in all.
    let take = ExitModel::new(&params).take_level(&d, &[], fill);
    assert!((take - 102.0).abs() < 1e-9, "{take}");
    // `MaxModifier` caps the SUM before the coefficient: min(2, 5) * 0.2 = 0.4.
    let capped = ExitParams {
        max_modifier: 2.0,
        ..params.clone()
    };
    let take = ExitModel::new(&capped).take_level(&d, &[], fill);
    assert!((take - 101.4).abs() < 1e-9, "{take}");
    // No coefficient, no movement — whatever the deltas.
    let off = ExitParams {
        sell_modifier: 0.0,
        ..params
    };
    let take = ExitModel::new(&off).take_level(&d, &[], fill);
    assert!((take - 101.0).abs() < 1e-9, "{take}");
}
