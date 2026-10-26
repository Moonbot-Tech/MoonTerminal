//! The sell line's shared step on synthetic tapes: the price grid, the latency, the mirror,
//! the chain across a step, and the archive's clock.

use super::*;
use crate::db::tuner::ticks::exit::ExitModel;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape};
use crate::db::tuner::ticks::{EntryParams, ModelSettings, verify};

/// Sparse prints must not reorder different due moments by the rules' same-pass priority.
#[test]
fn overdue_pump_move_precedes_a_later_sell_level() {
    let p = ExitParams {
        pump_move_timer_s: 2.0,
        pump_move_pct: 1.0,
        sell_level_delay_s: 3.0,
        sell_level_time_s: 10.0,
        sell_level_count: 1,
        sell_level_adjust_pct: 10.0,
        ..params()
    };
    let d = deal(false);
    let ticks = tape(&[(0, 110.0), (4_000, 100.0)]);
    let w = walk(&d, &ticks, fill(), 120.0, &p);
    assert_eq!(
        w.points.iter().map(|pt| pt.t_ms).collect::<Vec<_>>(),
        vec![0, 2_501, 3_000]
    );
    assert!((w.points[1].price - 109.9).abs() < 1e-9);
    assert!((w.points[2].price - 121.0).abs() < 1e-9);
}

/// PriceDown must wait for the next print after PumpMove, even when PumpMove sends no change.
#[test]
fn pump_move_ends_the_pass_before_price_down() {
    for (sell_level_on, take) in [(false, 120.0), (true, 120.0), (false, 109.9)] {
        let p = ExitParams {
            pump_move_timer_s: 2.0,
            pump_move_pct: 1.0,
            price_down_timer_s: 2.501,
            price_down_delay_s: 1.0,
            price_down_pct: 50.0,
            price_down_relative: true,
            sell_level_delay_s: if sell_level_on { 2.501 } else { 0.0 },
            sell_level_time_s: 10.0,
            sell_level_count: 1,
            sell_level_adjust_pct: 10.0,
            ..params()
        };
        let d = deal(false);
        let side = Side { long: true };
        let mut pump_move = PumpMove::new(&p, fill(), side, 0);
        let mut price_down = PriceDown::new(&p, &d, fill(), side);
        let mut sell_level = SellLevel::new(&p, &d, fill(), side);
        let mut deferred = false;
        let mut line = Line::new(None, take, 0, 0, 0);
        let seen = tape(&[(0, 110.0), (2_501, 100.0)]);
        Rules {
            pump_move: &mut pump_move,
            price_down: &mut price_down,
            sell_level: &mut sell_level,
            price_down_deferred: &mut deferred,
        }
        .step_through(2_501, &seen, &mut line);
        assert!((line.core() - 109.9).abs() < 1e-9);
        assert_eq!(price_down.due(2_501), Some(2_501));
        if sell_level_on {
            assert_eq!(line.points[1].t_ms, 2_501);
            assert!((line.points[1].price - 121.0).abs() < 1e-9);
        }
        Rules {
            pump_move: &mut pump_move,
            price_down: &mut price_down,
            sell_level: &mut sell_level,
            price_down_deferred: &mut deferred,
        }
        .step_through(3_700, &seen, &mut line);
        assert!((line.core() - 104.95).abs() < 1e-9);
        assert_eq!(line.points.last().unwrap().t_ms, 3_700);
        assert_eq!(price_down.due(4_699), None);
        assert_eq!(price_down.due(4_700), Some(4_700));
    }
}

/// A supported always-true formula with stops off isolates the replay clock from EMA calibration.
fn ema_params(delay_s: f64, latency_ms: f64) -> ExitParams {
    ExitParams {
        use_stop_loss: false,
        custom_ema: Some(std::sync::Arc::new(
            crate::db::tuner::ticks::custom_ema::parse("EMA(2s,1s) > 888").unwrap(),
        )),
        custom_ema_delay_s: delay_s,
        model: ModelSettings {
            latency_ms,
            ..ModelSettings::default()
        },
        ..params()
    }
}

/// Include the whole held run-up; sell-side prints also price a long's market replacement.
fn ema_tape(points: &[(i64, f64)]) -> Vec<Tick> {
    let mut ticks = tape(&[(-10_000, 100.0), (-5_000, 100.0), (0, 100.0)]);
    ticks.extend(tape(points));
    for tick in &mut ticks {
        tick.side = crate::feed::types::Side::Sell;
    }
    ticks
}

/// Checking inside SellEMADelay would sell a position before its formula is armed.
#[test]
fn custom_ema_not_checked_inside_delay() {
    let early = ema_tape(&[(500, 100.0), (1_000, 100.0)]);
    let w = walk(&deal(false), &early, fill(), 110.0, &ema_params(1.0, 0.0));
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
    assert_eq!(w.custom_ema_fired_ms, None);
    let ticks = ema_tape(&[(1_250, 100.0), (2_500, 101.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &ema_params(1.0, 0.0));
    assert_eq!(w.exit.kind, ExitKind::CustomEma);
    assert_eq!(w.custom_ema_fired_ms, Some(2_000));
    assert_eq!((w.exit.t_ms, w.exit.price), (2_500, 101.0));
    // The jump after the 2000 boundary must not leak back into that check's second.
    let mut p = ema_params(0.0, 0.0);
    p.custom_ema = Some(std::sync::Arc::new(
        crate::db::tuner::ticks::custom_ema::parse("EMA(2s,1s) > 0.1").unwrap(),
    ));
    let ticks = ema_tape(&[(1_250, 100.0), (2_500, 101.0), (3_000, 101.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(w.custom_ema_fired_ms, Some(3_000));
}

/// Tape before the formula's warm-up must not exhaust the dense-series cap.
#[test]
fn custom_ema_series_starts_at_warm_up_not_tape_start() {
    let mut ticks = tape(&[(-4 * 86_400_000, 100.0)]);
    ticks.extend(ema_tape(&[(1_250, 100.0), (2_500, 101.0)]));
    let mut d = deal(false);
    d.close_ms = 1_000;
    let p = ema_params(1.0, 300.0);
    for hold in [None, Some(2_000)] {
        let w = walk_held(&d, &ticks, fill(), 110.0, &p, hold);
        assert_eq!(w.exit.kind, ExitKind::CustomEma);
        assert_eq!(w.custom_ema_fired_ms, Some(2_000));
        assert_eq!(w.exit.t_ms, 2_500);
        assert!(!w.custom_ema_short);
    }
}

/// Adding SellEMADelay after a true check would place the sale a second delay too late.
#[test]
fn custom_ema_sells_at_trigger() {
    let ticks = ema_tape(&[(1_000, 100.0), (2_000, 100.0), (3_000, 98.0), (5_000, 97.0)]);
    let p = ema_params(2.0, 0.0);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(w.exit.kind, ExitKind::CustomEma);
    assert_eq!(w.custom_ema_fired_ms, Some(3_000));
    assert_eq!((w.exit.t_ms, w.exit.price), (3_000, 98.0));
}

/// Delaying formula checks must leave the resting take live before they start.
#[test]
fn take_before_custom_ema_checks_wins() {
    let ticks = ema_tape(&[(1_000, 100.0), (1_500, 111.0), (3_000, 99.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &ema_params(2.0, 0.0));
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Take, 1_500));
    assert_eq!(w.custom_ema_fired_ms, None);
}

/// Pricing at the trigger print would ignore transport latency and overstate the sale.
#[test]
fn custom_ema_sells_at_next_print_after_latency() {
    let ticks = ema_tape(&[(1_000, 100.0), (1_100, 99.0), (1_500, 98.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &ema_params(0.0, 300.0));
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::CustomEma, 1_500, 98.0)
    );
    assert_eq!(w.stop_trigger, None);
    assert_eq!(w.stop_fired_ms, None);
}

/// Equal-time triggers must keep the stop's established priority.
#[test]
fn stop_wins_same_second_over_custom_ema() {
    let mut p = ema_params(0.0, 0.0);
    p.use_stop_loss = true;
    p.fast_stop_loss = true;
    p.stop_loss_pct = -1.0;
    p.model.series_tick_ms = 250;
    let ticks = ema_tape(&[(750, 98.0), (1_000, 98.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(
        (w.exit.kind, w.stop_fired_ms),
        (ExitKind::Stop, Some(1_000))
    );
    assert_eq!(w.custom_ema_fired_ms, None);
}

/// Comparing call order instead of event time would incorrectly choose the 2000 EMA boundary.
#[test]
fn earlier_stop_between_prints_beats_later_custom_ema() {
    let mut p = ema_params(2.0, 0.0);
    p.use_stop_loss = true;
    p.fast_stop_loss = true;
    p.stop_loss_pct = -1.0;
    p.model.series_tick_ms = 250;
    let ticks = ema_tape(&[(1_100, 98.0), (2_500, 98.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(
        (w.exit.kind, w.stop_fired_ms),
        (ExitKind::Stop, Some(1_250))
    );
}

/// The archived stop path must not eclipse an earlier formula boundary between prints.
#[test]
fn earlier_custom_ema_beats_later_stop() {
    let mut d = deal(false);
    let mut p = ema_params(0.0, 0.0);
    p.use_stop_loss = true;
    p.fast_stop_loss = true;
    p.stop_loss_pct = -1.0;
    d.sell_reason = "StopLoss Market Sell".into();
    d.close_ms = 1_800;
    let mut anchor = crate::db::tuner::ticks::record::StopAnchor::of(&d, &p, None);
    anchor.fired = Some((1_800, 98.0));
    d.stop_anchor = Some(anchor);
    let ticks = ema_tape(&[(500, 100.0), (2_500, 98.0)]);
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!(
        (w.exit.kind, w.custom_ema_fired_ms),
        (ExitKind::CustomEma, Some(1_000))
    );
}

/// Forward-filling missing armed seconds would invent a known formula state through the hole.
#[test]
fn custom_ema_across_hole_is_in_gap() {
    let mut d = deal(false);
    d.gap = Some(TapeGap {
        from_ms: 1_500,
        to_ms: 10_000,
        fact_line: None,
        fact_stop: None,
    });
    let mut p = ema_params(1.0, 0.0);
    p.custom_ema = Some(std::sync::Arc::new(
        crate::db::tuner::ticks::custom_ema::parse("EMA(2s,1s) > 100").unwrap(),
    ));
    let ticks = ema_tape(&[(1_000, 100.0), (1_500, 100.0), (10_000, 100.0)]);
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::InGap, 1_500));
    assert!(!w.custom_ema_short);
    // Even a hole between two adjacent check boundaries invalidates an already armed formula.
    p.custom_ema_delay_s = 0.0;
    d.gap = Some(TapeGap {
        from_ms: 1_200,
        to_ms: 1_500,
        fact_line: None,
        fact_stop: None,
    });
    let narrow = ema_tape(&[
        (1_000, 100.0),
        (1_200, 100.0),
        (1_500, 100.0),
        (2_000, 100.0),
    ]);
    let w = walk_held(&d, &narrow, fill(), 110.0, &p, Some(60_000));
    assert_eq!(w.exit.kind, ExitKind::InGap);
    d.gap = Some(TapeGap {
        from_ms: 1_500,
        to_ms: 10_000,
        fact_line: None,
        fact_stop: None,
    });
    // An already fired market sell cannot price its arrival through the hole either.
    let w = walk(&d, &ticks, fill(), 110.0, &ema_params(1.0, 1_000.0));
    assert_eq!(w.exit.kind, ExitKind::InGap);
    assert_eq!(w.custom_ema_fired_ms, None);
}

/// Waiting for later warm-up after a short first check would silently invent an armed exit.
#[test]
fn custom_ema_short_history_disables_and_flags() {
    let ticks = tape(&[(0, 100.0), (1_000, 100.0), (10_000, 100.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &ema_params(1.0, 0.0));
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
    assert!(w.custom_ema_short);
    assert_eq!(w.custom_ema_fired_ms, None);
    // The whole held margin, rather than a 30-second pad, warms a longer formula.
    let mut p = ema_params(0.0, 0.0);
    p.custom_ema = Some(std::sync::Arc::new(
        crate::db::tuner::ticks::custom_ema::parse("EMA(30s,1s) > 888").unwrap(),
    ));
    let ticks = (-120..=1)
        .map(|second| (second * 1_000, 100.0))
        .collect::<Vec<_>>();
    let w = walk(&deal(false), &tape(&ticks), fill(), 110.0, &p);
    assert_eq!(w.exit.kind, ExitKind::CustomEma);
    assert!(!w.custom_ema_short);
    let ticks = tape(&[(-120_000, 100.0), (0, 100.0), (1_000, 100.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert!(
        w.custom_ema_short,
        "a warm-up hole cannot be forward-filled"
    );
}

/// Arrival must not replace the trigger clock, and a take filled during latency cancels it.
#[test]
fn custom_ema_fired_ms_kept_apart_from_fill() {
    let p = ema_params(0.0, 300.0);
    let ticks = ema_tape(&[(1_000, 100.0), (1_500, 98.0)]);
    let w = walk_held(&deal(false), &ticks, fill(), 110.0, &p, Some(60_000));
    assert_eq!(w.custom_ema_fired_ms, Some(1_000));
    assert_eq!(w.exit.t_ms, 1_500);
    let ticks = ema_tape(&[(1_000, 100.0), (1_100, 111.0), (1_500, 98.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(w.exit.kind, ExitKind::Take);
    assert_eq!(w.custom_ema_fired_ms, None);
}

/// Minute and five-minute closes cover three hours before a ten-minute held tape.
fn ema_warm_fixture() -> (Deal, Vec<Tick>, ExitParams) {
    let mut d = deal(false);
    let mut bars = Vec::new();
    let mut from_ms = -10_800_000;
    while from_ms < -600_000 {
        let to_ms = from_ms
            + if from_ms < -3_600_000 {
                300_000
            } else {
                60_000
            };
        bars.push(crate::db::tuner::ticks::deltas::Bar {
            from_ms,
            to_ms,
            open: 90.0,
            high: 90.0,
            low: 90.0,
            close: 90.0,
        });
        from_ms = to_ms;
    }
    // Neither an overlapping bar nor a future bar may override held prints.
    bars.push(crate::db::tuner::ticks::deltas::Bar {
        from_ms: -600_000,
        to_ms: 0,
        open: 1.0,
        high: 1.0,
        low: 1.0,
        close: 1.0,
    });
    d.bars = Some(bars.into());
    let points = (-600..=2).map(|s| (s * 1000, 100.0)).collect::<Vec<_>>();
    let mut ticks = tape(&points);
    for tick in &mut ticks {
        tick.side = crate::feed::types::Side::Sell;
    }
    let mut p = ema_params(0.0, 0.0);
    p.custom_ema = Some(std::sync::Arc::new(
        crate::db::tuner::ticks::custom_ema::parse("EMA(1h,1) > 0.1").unwrap(),
    ));
    (d, ticks, p)
}

/// Losing the bar prefix would disable an otherwise evaluable long-window formula.
#[test]
fn custom_ema_warms_from_bars_when_tape_short() {
    let (d, ticks, p) = ema_warm_fixture();
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!(w.exit.kind, ExitKind::CustomEma);
    assert_eq!(w.custom_ema_fired_ms, Some(1000));
    assert_eq!(w.custom_ema_path, Some(EmaHistoryPath::KlineWarmed));
    assert!(!w.custom_ema_short);
    // No bars in the requested historical hour must remain explicit missing history.
    let mut sparse = d.clone();
    let mut bars = sparse.bars.as_deref().unwrap().to_vec();
    bars.retain(|bar| bar.to_ms <= -3_600_000 || bar.from_ms >= 0);
    sparse.bars = Some(bars.into());
    assert!(walk(&sparse, &ticks, fill(), 110.0, &p).custom_ema_short);
}

/// Ten minutes of tape alone cannot manufacture the historical hour's candle price.
#[test]
fn custom_ema_short_without_bars() {
    let (mut d, ticks, p) = ema_warm_fixture();
    d.bars = None;
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert!(w.custom_ema_short);
    assert_eq!(w.custom_ema_fired_ms, None);
    assert_eq!(w.custom_ema_path, None);
}

/// Kline quote volume is not comparable to tape volume and cannot satisfy a Vol window.
#[test]
fn vol_never_warms_from_bars() {
    let (d, ticks, mut p) = ema_warm_fixture();
    p.custom_ema = Some(std::sync::Arc::new(
        crate::db::tuner::ticks::custom_ema::parse("Vol(1h,1m) < 888").unwrap(),
    ));
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert!(w.custom_ema_short);
    assert_eq!(w.custom_ema_fired_ms, None);
    assert_eq!(w.custom_ema_path, None);
}

/// Moving a sell limit into the prevailing market must not underprice its market fill.
#[test]
fn replacement_marketable_at_arrival_fills_at_the_print() {
    let p = ExitParams {
        sell_level_delay_s: -1.0,
        sell_level_time_s: 1.0,
        sell_level_count: 1,
        sell_level_adjust_pct: -10.0,
        sell_level_allowed_drop_pct: -40.0,
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..params()
    };
    let ticks = tape(&[(0, 100.0), (300, 100.0), (500, 100.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Line, 500, 100.0)
    );
}

/// Treating every crossed replacement as marketable would overprice a resting sell limit.
#[test]
fn replacement_resting_at_arrival_fills_at_its_limit() {
    let p = ExitParams {
        sell_level_delay_s: -1.0,
        sell_level_time_s: 1.0,
        sell_level_count: 1,
        sell_level_adjust_pct: 3.0,
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        use_stop_loss: false,
        ..params()
    };
    let ticks = tape(&[(0, 100.0), (300, 100.0), (500, 100.0), (700, 104.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Line, 700, 103.0)
    );
}

/// Overwriting queued timer moves must not leave the original take live on a sparse tape.
#[test]
fn queued_timer_moves_land_before_the_next_print() {
    let p = ExitParams {
        price_down_timer_s: 0.33,
        price_down_delay_s: 0.33,
        price_down_pct: 50.0,
        price_down_relative: true,
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..params()
    };
    let ticks = tape(&[(0, 100.0), (1_000, 104.0)]);
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!((w.exit.kind, w.exit.price), (ExitKind::Line, 102.5));
    assert_eq!(w.moved_at, Some(430));
}

/// A later replacement still in flight must not erase an earlier executable replacement.
#[test]
fn queued_moves_survive_latency_longer_than_the_interval() {
    let mut line = Line::new(None, 110.0, 0, 0, 500);
    line.place(330, 105.0);
    line.place(660, 102.5);
    line.land(1_000);
    assert_eq!(
        line.fill_on(1_000, 105.0, Side { long: true })
            .unwrap()
            .price,
        105.0
    );
    assert_eq!(line.moved_at, Some(830));
    line.land(1_160);
    assert_eq!(line.exch, 102.5);
}

/// SellLevel must precede a simultaneous relative PriceDown, which chains off its moved level.
#[test]
fn queued_moves_at_the_same_time_keep_placement_order() {
    let p = ExitParams {
        price_down_timer_s: 0.33,
        price_down_delay_s: 10.0,
        price_down_pct: 50.0,
        price_down_relative: true,
        sell_level_delay_s: 0.33,
        sell_level_time_s: 1.0,
        sell_level_count: 1,
        sell_level_adjust_pct: 3.0,
        ..params()
    };
    let mut d = deal(false);
    d.coin = "XALPHA".into();
    d.core_name = "CORE_A".into();
    let side = Side { long: true };
    let fill = fill();
    let mut pump_move = PumpMove::new(&p, fill, side, 0);
    let mut price_down = PriceDown::new(&p, &d, fill, side);
    let mut sell_level = SellLevel::new(&p, &d, fill, side);
    let mut line = Line::new(None, 110.0, 0, 0, 100);
    Rules {
        price_down_deferred: &mut false,
        pump_move: &mut pump_move,
        price_down: &mut price_down,
        sell_level: &mut sell_level,
    }
    .step_through(330, &tape(&[(0, 100.0)]), &mut line);
    line.land(430);
    assert_eq!(line.exch, 101.5);
    assert_eq!(line.moved_at, Some(430));
    assert_eq!(
        line.points
            .iter()
            .map(|p| (p.t_ms, p.price))
            .collect::<Vec<_>>(),
        vec![(0, 110.0), (430, 103.0), (430, 101.5)]
    );
}

#[test]
fn the_placed_level_is_rounded_to_the_step() {
    // ARX, 2026-09-21, step 0.0001: take 0.197802 goes to the book as 0.1978; the 20 %
    // relative steps chain off the placed prices (0.1978 → 0.19714 → 0.1971 → 0.19658), and
    // the floor at +1 % (0.196445) is placed at 0.1964 — which is why the print AT 0.1964
    // sells.
    let p = ExitParams {
        price_down_timer_s: 3.0,
        price_down_pct: 20.0,
        price_down_delay_s: 10.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 1.0,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 0.1945;
    d.tick = Some(0.0001);
    let fill = Fill {
        t_ms: 0,
        price: 0.1945,
    };
    let ticks = tape(&[
        (1_000, 0.1950),
        (4_000, 0.1950),
        (14_000, 0.1950),
        (24_000, 0.1950),
        (34_000, 0.1950),
        (40_000, 0.1964),
    ]);
    let w = walk(&d, &ticks, fill, 0.197802, &p);
    let levels: Vec<f64> = w.points.iter().map(|pt| pt.price).collect();
    let on_grid = |v: f64| (v / 0.0001).round() * 0.0001;
    assert_eq!(levels.len(), 4, "{levels:?}");
    for (level, want) in levels.iter().zip([0.1978, 0.1971, 0.1966, 0.1964]) {
        assert!((level - want).abs() < 1e-9, "{levels:?}");
        assert!(
            (level - on_grid(*level)).abs() < 1e-9,
            "off the grid: {level}"
        );
    }
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Line, 40_000));
    assert!(
        (w.exit.price - 0.1964).abs() < 1e-9,
        "sold at the placed level"
    );
}

#[test]
fn without_a_step_the_placed_level_is_the_exact_one() {
    let p = ExitParams {
        price_down_timer_s: 3.0,
        price_down_pct: 20.0,
        price_down_delay_s: 10.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 1.0,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 0.1945;
    let fill = Fill {
        t_ms: 0,
        price: 0.1945,
    };
    let ticks = tape(&[(1_000, 0.1950), (40_000, 0.1964)]);
    let w = walk(&d, &ticks, fill, 0.197802, &p);
    assert_eq!(
        w.exit.kind,
        ExitKind::OpenAtWindowEnd,
        "0.196445 is above the tape's high"
    );
}

/// Using the MoonShot delay for other kinds would miss their first live take print.
#[test]
fn the_take_is_on_the_book_only_after_the_replacement_latency() {
    let p = ExitParams {
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    };
    let ticks = tape(&[(9, 101.5), (150, 101.2)]);
    let mut d = deal(false);
    d.kind = "Spread".into();
    let w = walk(&d, &ticks, fill(), 101.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Take, 150));
}

/// Reusing the first-placement delay for a move would leave the old sell level live too long.
#[test]
fn a_moved_line_lands_after_the_replacement_latency_not_the_take_latency() {
    let mut line = Line::new(None, 101.0, 0, 220, 100);
    assert!(line.place(1_000, 100.5));
    line.land(1_099);
    assert_eq!(line.exch, 101.0);
    line.land(1_100);
    assert_eq!(line.exch, 100.5);
}

/// Gating an already landed replacement by the take delay would miss its +160 ms fill.
#[test]
fn a_replacement_landing_before_the_take_latency_fills_once_it_lands() {
    let p = ExitParams {
        price_down_timer_s: 0.05,
        price_down_pct: 50.0,
        price_down_relative: true,
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..params()
    };
    let ticks = tape(&[(0, 100.0), (160, 100.75), (230, 100.0)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Line, 160, 100.5)
    );
    assert_eq!(w.marketable_take, None);
}

/// Applying replacement latency to the first take would sell on the spike's early tail.
#[test]
fn a_moonshot_take_is_on_the_book_only_after_the_take_latency() {
    let ticks = tape(&[(0, 100.0), (150, 101.0), (230, 101.0)]);
    let w = walk(
        &deal(false),
        &ticks[..2],
        fill(),
        101.0,
        &ExitParams::default(),
    );
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &ExitParams::default());
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Take, 230));
}

/// Extending MoonShot's placement delay to other kinds would miss their +150 ms fill.
#[test]
fn other_kinds_place_the_take_after_the_replacement_latency() {
    let ticks = tape(&[(0, 100.0), (150, 101.0), (230, 101.0)]);
    for kind in ["Spread", "MoonStrike", "MoonHook"] {
        let mut d = deal(false);
        d.kind = kind.into();
        let w = walk(&d, &ticks, fill(), 101.0, &ExitParams::default());
        assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Take, 150), "{kind}");
    }
}

/// Pricing a marketable take at the early print would use a market it could not trade on.
#[test]
fn a_marketable_moonshot_take_fills_at_the_first_print_after_the_take_latency() {
    let ticks = tape(&[(0, 102.0), (150, 103.0), (230, 104.0)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &ExitParams::default());
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Take, 230, 104.0)
    );
}

// ---- the mirror and the archive -------------------------------------------------------------

/// A short mirrors relative PriceDown steps and places its stop above the buy.
#[test]
fn a_short_mirrors_every_rule() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.1,
        stop_loss_pct: -1.0,
        ..params()
    };
    // Take 99 (1 % below the buy at 100); one step -> 99.5 at t=1000; a print down to 99.4
    // before the next step crosses it.
    let ticks = tape(&[(1_500, 100.0), (1_900, 99.4)]);
    let w = walk(&deal(true), &ticks, fill(), 99.0, &p);
    assert!((w.points[1].price - 99.5).abs() < 1e-9, "{:?}", w.points);
    assert_eq!(w.exit.kind, ExitKind::Line);
    assert!((w.exit.price - 99.5).abs() < 1e-9);
    // The stop is ABOVE a short's buy.
    let ticks = tape(&[(500, 101.2)]);
    let w = walk(&deal(true), &ticks, fill(), 99.0, &p);
    assert_eq!(w.exit.kind, ExitKind::Stop);
}

#[test]
fn a_spike_through_the_old_level_fills_before_a_step_lands() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..params()
    };
    // The step is decided at t=1000 and reaches the book at t=1100; a print at 101 at t=1050
    // fills the old take at 101, not the new line at 100.5.
    let ticks = tape(&[(1_000, 100.0), (1_050, 101.0)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(w.exit.kind, ExitKind::Take);
    assert!((w.exit.price - 101.0).abs() < 1e-9);
}

/// Verification compares every relative step with the archived line, not only its exit price.
#[test]
fn verify_holds_the_line_against_the_archived_points() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.1,
        use_stop_loss: false,
        ..params()
    };
    let mut d = deal(false);
    d.sell_price = 100.25;
    let ticks = tape(&[(0, 100.0), (1_500, 100.0), (2_500, 100.3)]);
    // The archive's points are what the core did: placed at 101, 100.5 at 1 s, 100.25 at 2 s.
    let archived = [(0, 101.0), (1_000, 100.5), (2_000, 100.25)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&archived));
    assert_eq!(v.exit_kind, Some(ExitKind::Line));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert_eq!(v.line_points, Some((3, 3)));
    // A line the core moved differently - a point the model never re-placed at - is a miss
    // even when the exit price agrees.
    let other = [(0, 101.0), (1_000, 100.7), (2_000, 100.25)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&other));
    assert_eq!(v.exit, Some(false));
    assert_eq!(v.line_points, Some((2, 3)));
    // The same walk through `ExitModel`.
    let w = ExitModel::new(&p).walk(&d, &ticks, fill());
    assert_eq!(w.points.len(), 3);
}

// ---- the core's sell price across a step --------------------------------------------------------

/// Relative PriceDown on the grid, flat tape under the line: `(t, level)` of every point.
fn grid_walk(take: f64, step_lag_ms: f64, ticks: &[Tick]) -> Vec<(i64, f64)> {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 10.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.1,
        use_stop_loss: false,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 1_000.0;
    d.tick = Some(1.0);
    d.step_lag_ms = step_lag_ms;
    let fill = Fill {
        t_ms: 0,
        price: 1_000.0,
    };
    walk(&d, ticks, fill, take, &p)
        .points
        .iter()
        .map(|pt| (pt.t_ms, pt.price))
        .collect()
}

/// A step that moved the order chains the next one off the ORDER's price: 1096 → 1086.4, placed
/// 1086; then 1086 → 1077.4, placed 1077 — off the exact 1086.4 it would round to 1078. INDEX
/// 2026-09-22 (Gate, 10 % relative): all twelve archived levels land only this way.
#[test]
fn a_moved_order_chains_off_its_placed_price() {
    let ticks = tape(&[(500, 1_000.0), (1_500, 1_000.0), (2_500, 1_000.0)]);
    assert_eq!(
        grid_walk(1_096.0, 0.0, &ticks),
        vec![(0, 1_096.0), (1_000, 1_086.0), (2_000, 1_077.0)]
    );
}

/// A step that rounds back onto the order sends nothing and carries its exact value into the
/// next: 1004 → 1003.6 (still 1004), → 1003.24 (1003), → 1002.7 (still 1003), → 1002.43 (1002).
/// Off the placed price alone the line would never leave 1004; off the exact chain the second
/// move would come a step late. FATCOIN 2026-09-22 climbs one tick every other step this way.
#[test]
fn a_step_rounding_back_onto_the_order_carries_its_exact_value() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
        (4_500, 1_000.0),
    ]);
    assert_eq!(
        grid_walk(1_004.0, 0.0, &ticks),
        vec![(0, 1_004.0), (2_000, 1_003.0), (4_000, 1_002.0)]
    );
}

/// The core's own replace lag spaces the steps: the first is timed off the take by
/// `PriceDownTimer`, the next one after a step that moved the order off that step plus the delay
/// plus the lag — here every step moves the order.
#[test]
fn the_core_step_lag_spaces_the_price_down_steps() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
    ]);
    let times: Vec<i64> = grid_walk(1_096.0, 50.0, &ticks)
        .iter()
        .map(|&(t, _)| t)
        .collect();
    assert_eq!(times, vec![0, 1_000, 2_050, 3_100]);
}

/// A step rounding kept in place sent nothing, so the next one waits for no round trip: the
/// lag follows only the steps that moved the order (1004 stays at 1 s, 1003 at 2 s, 1003 stays
/// at 3.05 s, 1002 at 4.05 s).
#[test]
fn a_step_kept_in_place_adds_no_lag() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
        (4_500, 1_000.0),
    ]);
    assert_eq!(
        grid_walk(1_004.0, 50.0, &ticks),
        vec![(0, 1_004.0), (2_000, 1_003.0), (4_050, 1_002.0)]
    );
}

// ---- the verdict's clock ---------------------------------------------------------------------

/// PriceDown 50 % relative every second, 100 ms to the book: the take and its steps.
fn clock_params(take_pct: f64) -> ExitParams {
    ExitParams {
        use_stop_loss: false,
        sell_price_pct: take_pct,
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.1,
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..params()
    }
}

/// The core stepped onto 100.5 at 900 ms and sold there 100 ms before the close — the archive
/// files the step and the fill as one point. The model's step to 100.5 is stamped 1 100 ms: late
/// by 200 ms, inside the point tolerance. On the model's own clock the fill found the take still
/// standing and the verdict had nothing to say; on the archive's the line stood at 100.5.
#[test]
fn the_level_at_the_fill_is_read_on_the_archive_clock() {
    let mut d = deal(false);
    d.sell_price = 100.5;
    d.close_ms = 1_000;
    let ticks = tape(&[(0, 100.0), (500, 100.0), (1_500, 100.0)]);
    let archived = [(0, 101.0), (900, 100.5)];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::Fact,
        &clock_params(1.0),
        None,
        Some(&archived),
    );
    assert_eq!(v.exit_kind, Some(ExitKind::Line), "{v:?}");
    assert_eq!(v.exit, Some(true), "{v:?}");
}

/// A step the model took with no archived move to match: well before the fill it is a step the
/// core never took, and its level is the answer; inside the point tolerance of the fill it is
/// the model's timing, and the core's last level is.
#[test]
fn a_stray_step_counts_only_outside_the_point_tolerance_of_the_fill() {
    // Take 102; the model steps to 101 (1 100 ms), 100.5 (2 100 ms), 100.25 (3 100 ms). The
    // core stepped once, to 101, and filled a touch above it.
    let mut d = deal(false);
    d.sell_price = 101.02;
    let ticks = tape(&[
        (0, 100.0),
        (500, 100.0),
        (1_500, 100.0),
        (2_500, 100.0),
        (3_500, 100.0),
    ]);
    let p = clock_params(2.0);
    // Filled at 3 450 ms: the model's 100.5 at 2 100 ms is more than a second before it.
    d.close_ms = 3_600;
    let late = [(0, 102.0), (1_000, 101.0), (3_450, 101.02)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&late));
    assert_eq!(v.exit, Some(false), "{v:?}");
    // Filled at 2 500 ms: the model's 100.5 came 400 ms before it.
    d.close_ms = 2_600;
    let soon = [(0, 102.0), (1_000, 101.0), (2_500, 101.02)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&soon));
    assert_eq!(v.exit, Some(true), "{v:?}");
}

/// The fact's sell timers run from the take the core placed — the archive's first point, less
/// `SellDelay` — when that is later than the report's buy stamp.
#[test]
fn the_fact_sell_starts_at_the_archived_take() {
    let d = deal(false);
    let p = params();
    let start =
        |points: Option<&[(i64, f64)]>, p: &ExitParams| verify::fact_sell_start(&d, p, points).t_ms;
    assert_eq!(start(Some(&[(2_000, 101.0), (3_000, 100.5)]), &p), 2_000);
    let delayed = ExitParams {
        sell_delay_ms: 500.0,
        ..params()
    };
    assert_eq!(start(Some(&[(2_000, 101.0)]), &delayed), 1_500);
    assert_eq!(start(Some(&[(-7, 101.0)]), &p), 0, "never before the buy");
    assert_eq!(start(None, &p), 0);
}

// ---- a take placed through the market -------------------------------------------------------

/// A long's take at 101 goes live with the market already at 102: it fills at the market, on the
/// first live print 102.5, not at its level; a short's take at 99 under a market at 98 fills at
/// 97.5.
#[test]
fn a_take_through_the_market_when_it_goes_live_fills_at_the_market() {
    let ticks = tape(&[(0, 102.0), (500, 102.5)]);
    let long = walk(&deal(false), &ticks, fill(), 101.0, &params());
    assert_eq!(
        (long.exit.kind, long.exit.t_ms),
        (ExitKind::Take, 500),
        "{long:?}"
    );
    assert!(
        (long.exit.price - 102.5).abs() < 1e-9,
        "the market, {long:?}"
    );
    let ticks = tape(&[(0, 98.0), (500, 97.5)]);
    let short = walk(&deal(true), &ticks, fill(), 99.0, &params());
    assert_eq!(
        (short.exit.kind, short.exit.t_ms),
        (ExitKind::Take, 500),
        "{short:?}"
    );
    assert!(
        (short.exit.price - 97.5).abs() < 1e-9,
        "the market, {short:?}"
    );
}

/// A take resting short of the market when it went live and crossed later by a jump is a resting
/// limit: it fills at its level, 101, not at the jump's 102.
#[test]
fn a_take_crossed_only_after_it_rests_fills_at_its_level() {
    let ticks = tape(&[(0, 100.2), (500, 102.0)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &params());
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Take, 500), "{w:?}");
    assert!((w.exit.price - 101.0).abs() < 1e-9, "its level, {w:?}");
}

/// The trade's own settings, its take through the market when it went live: the core sold at the
/// market's 102.5 and the verdict prices the take there too — the exit is reproduced.
#[test]
fn verify_prices_a_marketable_take_at_the_market() {
    let mut d = deal(false);
    d.sell_reason = verify::REASON_TAKE.into();
    d.close_ms = 500;
    d.sell_price = 102.5;
    let ticks = tape(&[(0, 102.0), (500, 102.5), (5_000, 102.5)]);
    let v = verify(&d, &ticks, &EntryParams::Fact, &params(), None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::Take), "{v:?}");
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert!(v.exit_dev_pct.is_some_and(|dev| dev.abs() < 1e-6), "{v:?}");
}
