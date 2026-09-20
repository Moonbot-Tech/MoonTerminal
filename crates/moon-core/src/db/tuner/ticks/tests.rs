//! The model on synthetic tapes: every rule of the spec's §8, one print at a time.

use std::collections::HashMap;

use super::exit::pre_spike_price;
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

fn deal() -> Deal {
    Deal {
        report_uid: 1,
        core_uid: 7,
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
        deltas: Deltas::default(),
        tick: None,
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
        .fill(&deal(), &ticks, Some((200, 98.5)))
        .expect("filled");
    assert_eq!(fill.t_ms, 600);
    assert!((fill.price - 98.5).abs() < 1e-9);
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

    // Fact entry, take never reached: the exit is the fact and answers nothing.
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
    // The core did close it and the model never did: the exit group missed.
    assert_eq!(v.exit, Some(false));
    assert_eq!(v.exit_kind, Some(ExitKind::OpenAtWindowEnd));
}

#[test]
fn verify_reports_the_deviation_of_an_entry_off_the_fact() {
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
    ] {
        assert!(
            keys.iter().any(|k| k == key),
            "{key} missing from TICK_PARAMS"
        );
    }
    assert!(params_for(ParamGroup::Entry, "Spread").next().is_none());
    assert!(params_for(ParamGroup::Entry, "MoonShot").count() > 10);
    let exit_any: Vec<_> = params_for(ParamGroup::Exit, "Spread")
        .map(|p| p.key)
        .collect();
    assert!(exit_any.starts_with(&["SellPrice", "SellDelay", "PriceDownTimer"]));
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
