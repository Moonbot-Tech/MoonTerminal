//! Market-stop fills retain fire-time verdict routing and price the walk at arrival.

use super::*;
use crate::db::tuner::ticks::{Deltas, ModelSettings};
use crate::feed::types::{Side, Tick};

/// A plausible sell level from a truncated hour must not become a judged hit or miss.
#[test]
fn sell_level_short_lookback_is_unjudged() {
    let mut d = ema_deal();
    d.kind = "PumpsDetection".into();
    d.sell_reason = "Auto Price Down".into();
    d.close_ms = 5_000;
    d.archived_take = None;
    let p = ExitParams {
        sell_price_pct: 30.0,
        sell_at_last_price: false,
        use_stop_loss: false,
        sell_level_delay_s: 5.0,
        sell_level_time_s: 3600.0,
        sell_level_count: 1,
        sell_level_adjust_pct: 0.0,
        model: ModelSettings {
            latency_ms: 0.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let ticks = [-295_000, 0, 5_000, 6_000]
        .into_iter()
        .map(|at| Tick {
            time_ms: f64::from(at),
            price: 100.0,
            qty: 1.0,
            side: Side::Buy,
        })
        .collect::<Vec<_>>();
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(
        v.exit_finding,
        ExitFinding::Unjudged(Unjudged::SellLevelShortLookback)
    );
    assert_eq!(v.exit, None);
    assert!(!crate::db::tuner::ticks::record::fit_for_search(&v));
}

/// A later short look-back must not erase a fact sold before SellLevel first ran.
#[test]
fn sell_level_short_after_the_fact_sale_keeps_its_verdict() {
    let mut d = ema_deal();
    d.kind = "PumpsDetection".into();
    d.sell_reason = "Sell Price".into();
    d.sell_price = 101.0;
    d.archived_take = None;
    let p = ExitParams {
        sell_price_pct: 1.0,
        sell_at_last_price: false,
        use_stop_loss: false,
        sell_level_delay_s: 5.0,
        sell_level_time_s: 3600.0,
        sell_level_count: 1,
        model: ModelSettings {
            latency_ms: 0.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let ticks = [
        (-300_000, 100.0),
        (0, 100.0),
        (1_000, 101.0),
        (5_000, 100.0),
    ]
    .into_iter()
    .map(|(at, price)| Tick {
        time_ms: f64::from(at),
        price,
        qty: 1.0,
        side: Side::Buy,
    })
    .collect::<Vec<_>>();
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(v.exit_finding, ExitFinding::Hit);
    assert!(v.exit.is_some());
}

/// Neutral factual entry with an EMA close and a recorded take unrelated to its market sale.
fn ema_deal() -> Deal {
    Deal {
        report_uid: 1,
        core_uid: 7,
        core_name: "CORE_A".into(),
        strategy_id: 42,
        kind: "MoonShot".into(),
        coin: "XALPHA".into(),
        buy_ms: 0,
        close_ms: 1_000,
        buy_price: 100.0,
        sell_price: 100.0,
        spent: 1_000.0,
        is_short: false,
        sell_reason: "Sell by FilterCheck:EMA(2s,1s) = 0.0%".into(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: Some(110.0),
        fact_modifier: None,
        hook_depth_pct: None,
        drops_detect: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        quotes: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    }
}

/// Build EMA-only parameters through the real strategy reader, including refusals, with stops off.
fn ema_exit(text: &str) -> ExitParams {
    use crate::db::tuner::ticks::params::{StrategyValues, exit_params};
    let values = std::collections::HashMap::from([
        ("SellByCustomEMA".into(), text.into()),
        ("SellEMADelay".into(), "0".into()),
        ("UseStopLoss".into(), "NO".into()),
        ("MShotSellAtLastPrice".into(), "NO".into()),
        ("SellPrice".into(), "10".into()),
    ]);
    exit_params(
        &StrategyValues {
            values: &values,
            defaults: &Default::default(),
        },
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    )
}

/// A warm sell-side tape whose flat market gives a hand-known EMA deviation of zero.
fn ema_ticks() -> Vec<Tick> {
    [-10_000, -5_000, 0, 1_000, 2_000]
        .into_iter()
        .map(|at| Tick {
            time_ms: f64::from(at),
            price: 100.0,
            qty: 1.0,
            side: Side::Sell,
        })
        .collect()
}

/// Demanding take-line agreement for a formula exit would turn this same-price sale into a miss.
#[test]
fn filtercheck_ema_deal_judged_hit_within_tolerance() {
    let p = ema_exit("EMA(2s,1s) > 888");
    let archive = [(0, 110.0), (500, 109.0)];
    let v = verify(
        &ema_deal(),
        &ema_ticks(),
        &EntryParams::Fact,
        &p,
        Some(&archive),
        None,
    );
    assert_eq!(v.exit_kind, Some(ExitKind::CustomEma));
    assert_eq!(v.exit_finding, ExitFinding::Hit);
    assert_eq!(v.line_points, None);
    let mut d = ema_deal();
    d.sell_price = 90.0;
    let v = verify(&d, &ema_ticks(), &EntryParams::Fact, &p, None, None);
    assert!(matches!(
        v.exit_finding,
        ExitFinding::Miss(ExitMiss::Off(MissParts { level: true, .. }))
    ));
}

/// An on-time EMA sale must not admit variants whose resting take cannot be reconstructed.
#[test]
fn custom_ema_hit_with_unknown_take_is_unjudged() {
    let mut d = ema_deal();
    d.kind = "MoonHook".into();
    let p = ema_exit("EMA(2s,1s) > 888");
    assert!(!ExitModel::new(&p).take_known(&d));
    let v = verify(&d, &ema_ticks(), &EntryParams::Fact, &p, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::CustomEma));
    assert_eq!(v.exit_finding, ExitFinding::Unjudged(Unjudged::TakeUnknown));
}

/// An unplaced take must not hide a covered EMA formula that never closes the model.
#[test]
fn custom_ema_open_at_window_end_is_not_fired() {
    let mut p = ema_exit("EMA(2s,1s) > 100");
    p.sell_delay_ms = 60_000.0;
    for kind in ["MoonShot", "MoonHook"] {
        let mut d = ema_deal();
        d.kind = kind.into();
        let v = verify(&d, &ema_ticks(), &EntryParams::Fact, &p, None, None);
        assert_eq!(v.exit_kind, Some(ExitKind::OpenAtWindowEnd));
        assert_eq!(
            v.exit_finding,
            ExitFinding::Miss(ExitMiss::CustomEmaNotFired)
        );
    }
}

/// Price-only judging would accept a formula firing forty minutes after the factual sale.
#[test]
fn custom_ema_late_trigger_same_price_is_miss_with_late_ms() {
    let mut p = ema_exit("EMA(2s,1s) > 888");
    p.custom_ema_delay_s = 2_400.0;
    let ticks = (-10..=2_402)
        .map(|second| Tick {
            time_ms: f64::from(second * 1_000),
            price: 100.0,
            qty: 1.0,
            side: Side::Sell,
        })
        .collect::<Vec<_>>();
    let v = verify(&ema_deal(), &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::CustomEma));
    assert_eq!(
        v.exit_finding,
        ExitFinding::Miss(ExitMiss::Off(MissParts {
            level: false,
            late_ms: Some(2_400_000),
            first_unmatched: None,
        }))
    );
}

/// Comparing market arrival instead of the formula clock would reject an on-time sale.
#[test]
fn custom_ema_on_time_trigger_same_price_is_hit() {
    let mut p = ema_exit("EMA(2s,1s) > 888");
    p.model.point_time_ms = 1;
    p.model.latency_ms = 500.0;
    let mut d = ema_deal();
    d.close_ms = 1_100;
    let v = verify(&d, &ema_ticks(), &EntryParams::Fact, &p, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::CustomEma));
    assert_eq!(v.exit_finding, ExitFinding::Hit);
}

/// Adding SellEMADelay to the trigger would reject a correctly timed market sale.
#[test]
fn custom_ema_timing_compares_trigger_moment() {
    let mut p = ema_exit("EMA(2s,1s)>888");
    p.custom_ema_delay_s = 5.0;
    p.model.point_time_ms = 1;
    let ticks = (-10..=8)
        .map(|s| Tick {
            time_ms: f64::from(s * 1000),
            price: 100.0,
            qty: 1.0,
            side: Side::Sell,
        })
        .collect::<Vec<_>>();
    let mut d = ema_deal();
    d.close_ms = 6_000;
    assert_eq!(
        verify(&d, &ticks, &EntryParams::Fact, &p, None, None).exit_finding,
        ExitFinding::Hit
    );
    d.close_ms = 11_000;
    assert!(matches!(
        verify(&d, &ticks, &EntryParams::Fact, &p, None, None).exit_finding,
        ExitFinding::Miss(ExitMiss::Off(MissParts {
            level: false,
            late_ms: Some(-5_000),
            ..
        }))
    ));
    d.close_ms = 1_000;
    assert!(matches!(
        verify(&d, &ticks, &EntryParams::Fact, &p, None, None).exit_finding,
        ExitFinding::Miss(ExitMiss::Off(MissParts {
            level: false,
            late_ms: Some(5_000),
            ..
        }))
    ));
}

/// Falling back to OtherRule would hide a covered formula that never became true.
#[test]
fn filtercheck_ema_deal_miss_when_model_never_fires() {
    let p = ema_exit("EMA(2s,1s) > 100");
    let v = verify(
        &ema_deal(),
        &ema_ticks(),
        &EntryParams::Fact,
        &p,
        None,
        None,
    );
    assert_eq!(
        v.exit_finding,
        ExitFinding::Miss(ExitMiss::CustomEmaNotFired)
    );
}

/// Treating every FilterCheck as a formula would misjudge ordinary filter-bound exits.
#[test]
fn filtercheck_bounds_text_stays_other_rule() {
    let mut d = ema_deal();
    d.sell_reason = "Sell by FilterCheck: Vol Minute delta (45.64) doesnt match".into();
    let v = verify(
        &d,
        &ema_ticks(),
        &EntryParams::Fact,
        &ema_exit(""),
        None,
        None,
    );
    assert_eq!(v.exit_finding, ExitFinding::Unjudged(Unjudged::OtherRule));
}

/// Unsupported terms and broken syntax must remain explicit refusals instead of model misses.
#[test]
fn unsupported_term_formula_is_unjudged_with_reason() {
    for (text, why) in [
        ("Avg(2s,1s) > 0", Unjudged::CustomEmaUnsupported),
        ("Max(2s,1s) > 0", Unjudged::CustomEmaUnparsed),
        ("garbage", Unjudged::CustomEmaUnparsed),
    ] {
        let v = verify(
            &ema_deal(),
            &ema_ticks(),
            &EntryParams::Fact,
            &ema_exit(text),
            None,
            None,
        );
        assert_eq!(v.exit_finding, ExitFinding::Unjudged(why));
    }
}

/// Missing initial history must not turn into either an ordinary-rule refusal or a miss.
#[test]
fn short_history_is_unjudged_with_reason() {
    let ticks = ema_ticks()
        .into_iter()
        .filter(|t| t.time_ms >= 0.0)
        .collect::<Vec<_>>();
    let v = verify(
        &ema_deal(),
        &ticks,
        &EntryParams::Fact,
        &ema_exit("EMA(2s,1s) > 888"),
        None,
        None,
    );
    assert_eq!(
        v.exit_finding,
        ExitFinding::Unjudged(Unjudged::CustomEmaShortHistory)
    );
}

/// Routing on fill time would discard an on-time trigger when arrival exceeds point tolerance.
#[test]
fn custom_ema_trigger_before_close_judged_despite_latency() {
    let mut p = ema_exit("EMA(2s,1s) > 888");
    p.model.latency_ms = 500.0;
    p.model.point_time_ms = 1;
    let mut d = ema_deal();
    d.sell_reason = "Sell Price".into();
    d.close_ms = 1_100;
    let ticks = ema_ticks();
    let walked = ExitModel::new(&p).walk_held(
        &d,
        &ticks,
        Fill {
            t_ms: 0,
            price: 100.0,
        },
        d.close_ms,
    );
    assert_eq!(walked.custom_ema_fired_ms, Some(1_000));
    assert_eq!(walked.exit.t_ms, 2_000);
    // A non-EMA fact removes the fact_is_ema override and tests the trigger clock itself.
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::CustomEma));
    assert_eq!(v.exit_finding, ExitFinding::Unjudged(Unjudged::OtherRule));
    d.sell_reason = ema_deal().sell_reason;
    assert_eq!(
        verify(&d, &ticks, &EntryParams::Fact, &p, None, None).exit_finding,
        ExitFinding::Hit
    );
}

/// Routing on arrival instead of fire would replace this on-time stop with the take line.
#[test]
fn default_market_stop_routes_on_fire_and_prices_the_market_print() {
    let mut d = Deal {
        report_uid: 1,
        core_uid: 7,
        core_name: String::new(),
        strategy_id: 42,
        kind: "MoonShot".into(),
        coin: "TEST".into(),
        buy_ms: 0,
        close_ms: 250,
        buy_price: 100.0,
        sell_price: 97.0,
        spent: 1000.0,
        is_short: false,
        sell_reason: "StopLoss".into(),
        fact_pnl: -30.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        drops_detect: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        quotes: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    };
    let p = ExitParams {
        fast_stop_loss: true,
        stop_loss_pct: -1.0,
        model: ModelSettings {
            latency_ms: 300.0,
            point_time_ms: 1,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    };
    let ticks = [
        Tick {
            time_ms: 10.0,
            price: 98.0,
            qty: 1.0,
            side: Side::Buy,
        },
        Tick {
            time_ms: 555.0,
            price: 97.0,
            qty: 1.0,
            side: Side::Sell,
        },
    ];
    let walked = ExitModel::new(&p).walk(
        &d,
        &ticks,
        Fill {
            t_ms: 0,
            price: 100.0,
        },
    );
    assert_eq!((walked.exit.t_ms, walked.exit.price), (555, 97.0));
    let verdict = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(verdict.exit_kind, Some(ExitKind::Stop));
    assert_eq!(verdict.exit, Some(true), "{verdict:?}");
    // A non-stop fact removes the fact_stopped routing override.
    d.sell_reason = "Auto Price Down".into();
    let verdict = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(verdict.exit_kind, Some(ExitKind::Stop));
}

/// A long bought at 100 at 0 whose Exit line holds the take placement at 101 first.
fn time_stop_deal(is_short: bool) -> Deal {
    Deal {
        core_name: "CORE_A".into(),
        coin: "XALPHA".into(),
        buy_ms: 0,
        close_ms: 61_000,
        buy_price: 100.0,
        sell_price: 100.0,
        is_short,
        sell_reason: "StopLoss".into(),
        archived_take: Some(if is_short { 99.0 } else { 101.0 }),
        ..crate::db::tuner::ticks::tests::deal()
    }
}

/// `StopLoss` +50 after `StopLossDelay` 60: a book-watching time stop.
fn time_stop(stop_loss_pct: f64, delay_s: f64) -> ExitParams {
    ExitParams {
        stop_loss_pct,
        stop_loss_delay_s: delay_s,
        fast_stop_loss: false,
        ..ExitParams::default()
    }
}

/// The take placement at +1 % stands under a +50 % stop level and so reaches it already; the
/// panic replacement at the buy-side price after the delay is the activation.
#[test]
fn time_stop_activation_skips_the_initial_take() {
    let d = time_stop_deal(false);
    let line = [(50, 101.0), (60_100, 99.9)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 60.0), 150.0, Some(&line)),
        Some(60_100)
    );
    // No delay: the take-placement skip alone holds the first move back.
    let line = [(50, 101.0), (2_000, 99.9)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 0.0), 150.0, Some(&line)),
        Some(2_000)
    );
    // The immediate fast stop's first placement is its own order.
    let immediate = ExitParams {
        fast_stop_loss: true,
        ..time_stop(50.0, 0.0)
    };
    assert_eq!(
        archived_stop_jump(&d, &immediate, 150.0, Some(&line)),
        Some(50)
    );
}

/// Every re-placement of the take at its archived price is skipped, not only the first: the
/// panic replacement after them is the activation.
#[test]
fn time_stop_activation_skips_a_later_take_replacement() {
    let d = time_stop_deal(false);
    // A move off the take and back onto it after the delay: that re-placement is no stop.
    let line = [(0, 101.0), (30_000, 100.5), (61_000, 101.0), (62_000, 99.9)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 60.0), 150.0, Some(&line)),
        Some(62_000)
    );
}

/// A positive stop with no archived take has nothing to skip: the first move past the level
/// once the delay ran out is the activation.
#[test]
fn positive_stop_without_an_archived_take_uses_the_delay_gate_only() {
    let mut d = time_stop_deal(false);
    d.archived_take = None;
    let line = [(50, 101.0), (61_000, 100.8), (62_000, 99.9)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 60.0), 150.0, Some(&line)),
        Some(61_000)
    );
}

/// A move reaching the level before `StopLossDelay` ran out is not the activation.
#[test]
fn time_stop_activation_respects_stop_loss_delay() {
    let d = time_stop_deal(false);
    let p = time_stop(-1.0, 60.0);
    let line = [(50, 101.0), (30_000, 98.5), (61_000, 98.0)];
    assert_eq!(archived_stop_jump(&d, &p, 99.0, Some(&line)), Some(61_000));
}

/// A plain negative stop without a delay fires at the first move past it, as before.
#[test]
fn negative_stop_activation_unchanged() {
    let mut d = time_stop_deal(false);
    d.archived_take = Some(101.0);
    let line = [(0, 101.0), (1_000, 100.8), (3_600, 94.9), (4_040, 94.0)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(-5.0, 0.0), 95.0, Some(&line)),
        Some(3_600)
    );
}

/// A short's time stop: the take sits below the buy, the level too.
#[test]
fn short_time_stop_activation_mirrored() {
    let d = time_stop_deal(true);
    let line = [(50, 99.0), (60_100, 100.1)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 60.0), 50.0, Some(&line)),
        Some(60_100)
    );
    let line = [(50, 99.0), (2_000, 100.1)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 0.0), 50.0, Some(&line)),
        Some(2_000)
    );
}

/// A long fast stop -1 % fired at 250 whose market sale lands on a 90 bid, under the panic
/// sell's `AllowedDrop` -2 floor (98): the fact's stop, and the tape.
fn past_floor_case() -> (Deal, ExitParams, Vec<Tick>) {
    let d = Deal {
        close_ms: 400,
        sell_price: 90.0,
        ..time_stop_deal(false)
    };
    let p = ExitParams {
        fast_stop_loss: true,
        stop_loss_pct: -1.0,
        allowed_drop_pct: -2.0,
        use_market_order: false,
        model: ModelSettings {
            latency_ms: 0.0,
            point_time_ms: 1,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    };
    let ticks = vec![
        Tick {
            time_ms: 10.0,
            price: 98.0,
            qty: 1.0,
            side: Side::Buy,
        },
        Tick {
            time_ms: 300.0,
            price: 90.0,
            qty: 1.0,
            side: Side::Sell,
        },
    ];
    (d, p, ticks)
}

/// A stop sale past the floor is no sale the core could make: neither a hit nor a miss.
#[test]
fn past_floor_stop_is_unjudged_sale_unknown() {
    let (d, p, ticks) = past_floor_case();
    let verdict = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(
        verdict.exit_finding,
        ExitFinding::Unjudged(Unjudged::SaleUnknown),
        "{verdict:?}"
    );
    assert_eq!(verdict.exit, None);
}
