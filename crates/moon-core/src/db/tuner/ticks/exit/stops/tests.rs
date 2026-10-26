//! The Stops section on synthetic tapes: the fast stop, the book stop on its ticker proxy, and
//! how the verdict judges each.

use super::*;
use crate::db::tuner::ticks::exit::level_off_buy;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape, tick};
use crate::db::tuner::ticks::{EntryParams, verify};
use crate::feed::types::Side as TickSide;

/// Excluding a ladder cycle tied with a fast tick delays the newly raised stop by a full tick.
#[test]
fn fast_tick_reads_a_ladder_cycle_at_its_own_moment() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.ticker_period_ms = 1_000;
    p.second_stop = Some(ladder::StopStep {
        after_s: 0.0,
        switch_pct: 1.0,
        level_pct: 2.0,
    });
    let mut d = deal(false);
    d.kind = "EMA".into();
    d.core_name = "CORE_A".into();
    d.coin = "XALPHA".into();
    let mut stops = Stops::new(&d, &[], fill(), &p, Side { long: true });
    let mut first = tick(750, 101.5);
    first.side = TickSide::Sell;
    assert!(stops.on_print(&first, 750, 101.5).is_none());
    let next = tick(2_000, 101.5);
    let fire = stops.on_print(&next, 2_000, 101.5).expect("raised stop");
    assert_eq!(fire.t_ms, 1_000);
    assert_eq!(stops.level(), Some(102.0));
    // Finishing the same tape at its actual last print must read the same ladder cycle.
    let mut exhausted = Stops::new(&d, &[], fill(), &p, Side { long: true });
    assert!(exhausted.on_print(&first, 750, 101.5).is_none());
    assert_eq!(exhausted.after_tape(750).map(|exit| exit.t_ms), Some(1_000));
}

/// Using the initial stop to split the archive would judge a raised stop's panic sell as line moves.
#[test]
fn raised_stop_with_a_truncated_reason_excludes_the_panic_line() {
    let mut d = deal(false);
    d.kind = "EMA".into();
    d.core_name = "CORE_A".into();
    d.coin = "XALPHA".into();
    d.close_ms = 60_250;
    d.sell_price = 100.1;
    d.archived_take = Some(104.0);
    d.sell_reason = "StopLoss AutoActivated on price drop: BID = 100.2 ASK: 100.3 (strategy".into();
    let p = ExitParams {
        sell_price_pct: 4.0,
        stop_loss_pct: -2.4,
        fast_stop_loss: true,
        second_stop: Some(ladder::StopStep {
            after_s: 10.0,
            switch_pct: 1.0,
            level_pct: 0.3,
        }),
        ..params()
    };
    let mut ticks = tape(&[
        (1, 100.0),
        (11_000, 102.0),
        (13_000, 102.0),
        (60_000, 100.2),
        (61_000, 100.1),
    ]);
    for tick in &mut ticks {
        tick.side = TickSide::Sell;
    }
    let archive = [(0, 104.0), (60_250, 99.5), (60_270, 100.1)];
    let verdict = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&archive));
    assert_eq!(verdict.exit_kind, Some(ExitKind::Stop));
    assert_eq!(verdict.line_points, Some((1, 1)), "{verdict:?}");
    assert_eq!(verdict.exit, Some(true), "{verdict:?}");
    // Record consumers cannot infer that the factual ladder took the model's step.
    assert_eq!(
        crate::db::tuner::ticks::verify::stop_jump_level(&d, &p, Some(&archive)),
        None
    );
    assert_eq!(verdict.stop.quote, None);
    assert_eq!(verdict.stop.ticker_age_ms, None);
    let anchor = crate::db::tuner::ticks::record::StopAnchor::of(&d, &p, Some(&archive));
    assert_eq!(anchor.fired, Some((d.close_ms, d.sell_price)));
}

/// Dropping the resting order at fire would lose a take touched before panic arrival.
#[test]
fn market_stop_fill_lets_the_resting_take_fill_during_latency() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.latency_ms = 300.0;
    let mut ticks = tape(&[(510, 98.0), (800, 102.0), (1100, 97.0)]);
    ticks[2].side = TickSide::Sell;
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Take, 800, 101.0)
    );
    assert_eq!(w.stop_fired_ms, None);
}

/// Without a take touch, the replacement still sells at the first arrival-side bid.
#[test]
fn market_stop_fill_without_a_take_touch_sells_at_the_bid() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.latency_ms = 300.0;
    let mut ticks = tape(&[(510, 98.0), (1100, 97.0)]);
    ticks[1].side = TickSide::Sell;
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Stop, 1100, 97.0)
    );
    assert_eq!(w.stop_fired_ms, Some(750));
}

/// Applying the latency scan to AtCheck would incorrectly replace its immediate stop.
#[test]
fn at_check_fill_ignores_the_latency_window() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.latency_ms = 300.0;
    p.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::AtCheck;
    let mut ticks = tape(&[(510, 98.0), (800, 102.0), (1100, 97.0)]);
    ticks[2].side = TickSide::Sell;
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Stop, 750, 98.0)
    );
}

/// The default must price a panic sell at the bid after latency, never the rebound ask.
#[test]
fn stop_fill_default_is_market_after_latency() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.latency_ms = 300.0;
    let mut ticks = tape(&[(10, 98.0), (551, 105.0), (555, 97.0)]);
    ticks[2].side = TickSide::Sell;
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Stop, 555, 97.0)
    );
}

/// A missing arrival interval must clear trigger diagnostics instead of claiming a fill.
#[test]
fn market_stop_fill_across_a_hole_is_ingap() {
    let mut d = deal(false);
    d.gap = Some(crate::db::tuner::ticks::gap::TapeGap {
        from_ms: 300,
        to_ms: 700,
        fact_line: None,
        fact_stop: None,
    });
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.latency_ms = 300.0;
    let mut ticks = tape(&[(10, 98.0), (260, 100.0), (800, 97.0)]);
    ticks[2].side = TickSide::Sell;
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::InGap, 300));
    assert_eq!((w.stop_trigger, w.stop_fired_ms), (None, None));
}

/// An after-tape trigger must carry the last post-fill bid and date its arrival after firing.
#[test]
fn market_stop_fill_after_the_tape_end_uses_the_last_post_fill_bid() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.latency_ms = 300.0;
    let mut ticks = tape(&[(10, 97.0), (20, 98.0)]);
    ticks[0].side = TickSide::Sell;
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(
        (w.exit.t_ms, w.exit.price, w.stop_fired_ms),
        (550, 97.0, Some(250))
    );
}

/// A stale pre-fill bid must never replace the trigger price on an exhausted tape.
#[test]
fn market_stop_fill_never_prices_a_prefill_print() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.latency_ms = 300.0;
    let mut ticks = tape(&[(0, 90.0), (10, 98.0)]);
    ticks[0].side = TickSide::Sell;
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!((w.exit.t_ms, w.exit.price), (550, 98.0));
}

/// Shared synthetic fast-stop configuration isolates the compared price.
fn fast_params(rule: FastPrice) -> ExitParams {
    let mut p = params();
    p.fast_stop_loss = true;
    p.stop_loss_pct = -1.0;
    p.model.fast_price = rule;
    p
}

/// Zero-quantity tapes must still fire from the tick price when no estimator trade exists.
#[test]
fn weighted_fast_stop_falls_back_to_last_without_trades() {
    let mut ticks = tape(&[(10, 98.0), (300, 100.0)]);
    for tick in &mut ticks {
        tick.qty = 0.0;
    }
    let w = walk(
        &deal(false),
        &ticks,
        fill(),
        110.0,
        &fast_params(FastPrice::VwapLastTrades(5)),
    );
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Stop, 250, 98.0)
    );
}

/// Fact anchoring must not depend on an estimator whose missing middle is never read.
#[test]
fn anchored_weighted_fast_stop_crosses_a_hole_on_the_fact() {
    use crate::db::tuner::ticks::{gap::TapeGap, record::StopAnchor};
    let mut d = deal(false);
    d.close_ms = 3000;
    d.sell_reason = "StopLoss".into();
    d.sell_price = 98.0;
    let mut p = fast_params(FastPrice::LastOfTick);
    d.stop_anchor = Some(StopAnchor::of(&d, &p, None));
    d.gap = Some(TapeGap {
        from_ms: 500,
        to_ms: 2000,
        fact_line: Some(std::sync::Arc::from([(0, 110.0)])),
        fact_stop: None,
    });
    let ticks = tape(&[(10, 100.0), (500, 100.0), (2000, 100.0), (3500, 98.0)]);
    let last = walk(&d, &ticks, fill(), 110.0, &p);
    p.model.fast_price = FastPrice::VwapLastTrades(5);
    let weighted = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!(weighted.exit, last.exit);
    assert_eq!(
        (weighted.exit.kind, weighted.exit.t_ms, weighted.exit.price),
        (ExitKind::Stop, 3000, 98.0)
    );
}

/// Both fill modes share one trigger; buys after arrival cannot fill a long panic sell.
#[test]
fn market_stop_fill_waits_latency_and_takes_the_bid_side() {
    use crate::db::tuner::ticks::settings::StopFill;
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.stop_fill = StopFill::MarketAfterLatency;
    p.model.latency_ms = 300.0;
    let mut ticks = tape(&[(10, 98.0), (551, 105.0), (555, 97.0)]);
    ticks[2].side = TickSide::Sell;
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Stop, 555, 97.0)
    );
    // A short's panic buy takes the ask side instead.
    ticks[0].price = 102.0;
    ticks[1].side = TickSide::Sell;
    ticks[2].side = TickSide::Buy;
    ticks[2].price = 103.0;
    let short = walk(&deal(true), &ticks, fill(), 90.0, &p);
    assert_eq!((short.exit.t_ms, short.exit.price), (555, 103.0));
    // A bid not yet consumed at firing cannot supply the fallback before arrival.
    let mut last_bid = tape(&[(10, 98.0), (400, 97.0)]);
    last_bid[1].side = TickSide::Sell;
    for (ticks, price) in [(last_bid, 98.0), (tape(&[(10, 98.0)]), 98.0)] {
        let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
        assert_eq!(
            (w.exit.t_ms, w.exit.price, w.stop_fired_ms),
            (550, price, Some(250))
        );
    }
    // Even a known fire cannot price a panic sell through an unseen interval.
    let mut d = deal(false);
    d.gap = Some(crate::db::tuner::ticks::gap::TapeGap {
        from_ms: 300,
        to_ms: 700,
        fact_line: None,
        fact_stop: None,
    });
    let mut ticks = tape(&[(10, 98.0), (260, 100.0), (800, 97.0)]);
    ticks[2].side = TickSide::Sell;
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::InGap, 300));
    assert_eq!((w.stop_fired_ms, w.stop_trigger), (None, None));
    // The fallback's scan also crosses the hole, even with arrival before its start.
    p.model.latency_ms = 0.0;
    ticks[0].side = TickSide::Sell;
    ticks[2].side = TickSide::Buy;
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::InGap, 300));
}

/// Selecting the comparison rule must retain the instantaneous historical replay result.
#[test]
fn stop_fill_at_check_when_selected() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::AtCheck;
    let w = walk(
        &deal(false),
        &tape(&[(10, 98.0), (551, 105.0), (555, 97.0)]),
        fill(),
        110.0,
        &p,
    );
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Stop, 250, 98.0)
    );
}

/// A delayed market exit must preserve the trigger time and rule for timing diagnostics.
#[test]
fn market_stop_fill_keeps_the_fire_moment_for_diagnostics() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::MarketAfterLatency;
    p.model.latency_ms = 300.0;
    let mut ticks = tape(&[(10, 98.0), (555, 97.0)]);
    ticks[1].side = TickSide::Sell;
    let w = walk(&deal(false), &ticks, fill(), 110.0, &p);
    assert_eq!(w.stop_fired_ms, Some(250));
    assert_eq!(w.stop_trigger, Some(StopTrigger::FastTick));
    assert_eq!(w.exit.t_ms, 555);
    let mut d = deal(false);
    d.close_ms = 250;
    d.sell_reason = "StopLoss Market Sell".into();
    p.model.point_time_ms = 1;
    let verdict = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(verdict.exit, Some(true), "{verdict:?}");
}

/// The recorded sale is already a fill and must never receive a second order latency.
#[test]
fn market_stop_fill_ignores_the_anchored_fact_exit() {
    let mut d = deal(false);
    d.close_ms = 400;
    d.sell_reason = "StopLoss".into();
    d.sell_price = 98.0;
    let mut p = fast_params(FastPrice::LastOfTick);
    d.stop_anchor = Some(crate::db::tuner::ticks::record::StopAnchor::of(
        &d, &p, None,
    ));
    p.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::MarketAfterLatency;
    p.model.latency_ms = 300.0;
    // Cover both fired_by and after_tape's fact path.
    for ticks in [tape(&[(10, 100.0), (800, 90.0)]), tape(&[(10, 100.0)])] {
        let w = walk(&d, &ticks, fill(), 110.0, &p);
        assert_eq!((w.exit.t_ms, w.exit.price), (400, 98.0));
        assert_eq!(w.stop_trigger, Some(StopTrigger::Fact));
    }
}

/// A positive stop after a shot low cannot sell a rebound buy that never reached the bid.
#[test]
fn positive_stop_does_not_sell_the_rebound_print() {
    let mut p = fast_params(FastPrice::LastOfTick);
    p.stop_loss_pct = 2.0;
    p.model.latency_ms = 300.0;
    p.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::AtCheck;
    let f = Fill {
        t_ms: 0,
        price: 1.0,
    };
    let mut ticks = tape(&[
        (10, 1.0),
        (90, 1.015),
        (300, 1.001),
        (551, 1.020),
        (555, 1.002),
    ]);
    ticks[0].side = TickSide::Sell;
    ticks[2].side = TickSide::Sell;
    ticks[4].side = TickSide::Sell;
    let at_check = walk(&deal(false), &ticks, f, 1.1, &p);
    assert!((at_check.exit.price - 1.015).abs() < 1e-6);
    p.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::MarketAfterLatency;
    let market = walk(&deal(false), &ticks, f, 1.1, &p);
    assert_eq!(market.exit.kind, ExitKind::Stop);
    assert!(market.exit.price <= 1.002);
    assert_eq!((market.stop_fired_ms, market.exit.t_ms), (Some(250), 555));
}

/// Moving the default estimator would change the first firing tick.
#[test]
fn fast_default_is_last_of_tick() {
    let p = fast_params(crate::db::tuner::ticks::ModelSettings::default().fast_price);
    let w = walk(
        &deal(false),
        &tape(&[(10, 100.0), (240, 98.0), (500, 100.0)]),
        fill(),
        110.0,
        &p,
    );
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Stop, 250, 98.0)
    );
}

/// Warming the legacy pending tick would invent a stop before the first post-fill print.
#[test]
fn fast_default_unchanged_by_prefill_prints() {
    let w = walk(
        &deal(false),
        &tape(&[(90, 98.0), (1000, 100.0)]),
        Fill {
            t_ms: 100,
            price: 100.0,
        },
        110.0,
        &fast_params(FastPrice::LastOfTick),
    );
    assert_ne!(w.exit.kind, ExitKind::Stop);
}

/// A small last print must not outweigh the preceding four large trades.
#[test]
fn fast_vwap_last_trades_ignores_a_lone_tick_end_print() {
    let mut ticks = tape(&[
        (10, 100.0),
        (50, 100.0),
        (100, 100.0),
        (150, 100.0),
        (240, 98.0),
        (500, 100.0),
    ]);
    for t in &mut ticks[..4] {
        t.qty = 10.0;
    }
    let run = |rule| walk(&deal(false), &ticks, fill(), 110.0, &fast_params(rule));
    assert_eq!(run(FastPrice::LastOfTick).exit.kind, ExitKind::Stop);
    assert_ne!(run(FastPrice::VwapLastTrades(5)).exit.kind, ExitKind::Stop);
}

/// Sustained adverse trades must eventually overcome the EMA's initial price.
#[test]
fn fast_trade_ema_fires_after_sustained_prints() {
    let ticks = tape(&[
        (10, 100.0),
        (260, 98.0),
        (510, 98.0),
        (760, 98.0),
        (1010, 100.0),
    ]);
    let w = walk(
        &deal(false),
        &ticks,
        fill(),
        110.0,
        &fast_params(FastPrice::TradeEma {
            alpha: 0.5,
            qty_weighted: false,
        }),
    );
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 500));
}

/// Counting split prints as trades would evict the earlier trade from this two-trade VWAP.
#[test]
fn fast_price_groups_same_ms_prints_into_one_trade() {
    let mut state = FastPriceState::new(FastPrice::VwapLastTrades(2));
    for t in tape(&[(1, 100.0), (2, 96.0), (2, 98.0), (2, 100.0)]) {
        state.see(&t);
    }
    assert_eq!(state.value(), Some(98.5));
    state.see(&Tick {
        side: TickSide::Sell,
        ..tick(2, 96.0)
    });
    assert_eq!(
        state.value(),
        Some(97.5),
        "a side change opens a new trade at the same timestamp"
    );
}

/// Tentative EMA steps must replace rather than accumulate the open group's contribution.
#[test]
fn fast_trade_ema_steps_once_per_trade() {
    for qty_weighted in [false, true] {
        let mut split = FastPriceState::new(FastPrice::TradeEma {
            alpha: 0.25,
            qty_weighted,
        });
        let mut whole = FastPriceState::new(FastPrice::TradeEma {
            alpha: 0.25,
            qty_weighted,
        });
        let initial = tick(1, 100.0);
        split.see(&initial);
        whole.see(&initial);
        split.see(&tick(2, 96.0));
        let _ = split.value();
        split.see(&tick(2, 100.0));
        whole.see(&Tick {
            qty: 2.0,
            ..tick(2, 98.0)
        });
        assert_eq!(split.value(), whole.value());
        assert_eq!(
            split.value(),
            Some(if qty_weighted { 99.0 + 1.0 / 3.0 } else { 99.5 })
        );
        split.see(&tick(3, 94.0));
        whole.see(&tick(3, 94.0));
        assert_eq!(split.value(), whole.value());
    }
}

/// Discarding pre-fill trades would miss the weighted crossing on the first pending tick.
#[test]
fn fast_price_is_warm_at_the_fill() {
    let w = walk(
        &deal(false),
        &tape(&[(10, 96.0), (50, 96.0), (110, 100.0), (500, 100.0)]),
        Fill {
            t_ms: 100,
            price: 100.0,
        },
        110.0,
        &fast_params(FastPrice::VwapLastTrades(3)),
    );
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 250));
}

/// Selling at the estimator would turn a trigger estimate into an invented market fill.
#[test]
fn fast_exit_price_is_the_print_not_w() {
    let w = walk(
        &deal(false),
        &tape(&[(10, 96.0), (100, 100.0), (500, 100.0)]),
        fill(),
        110.0,
        &fast_params(FastPrice::VwapLastTrades(2)),
    );
    assert_eq!(
        (w.exit.kind, w.exit.t_ms, w.exit.price),
        (ExitKind::Stop, 250, 100.0)
    );
}

/// A rung after a pending tick closes must not retroactively make that tick fire.
#[test]
fn fast_price_respects_ladder_at_tick_end() {
    for (after_s, expected) in [(0.0, Some(1000)), (1.0, None)] {
        let mut p = fast_params(FastPrice::VwapLastTrades(2));
        p.stop_loss_pct = -10.0;
        p.model.ticker_period_ms = 100;
        p.model.series_tick_ms = 1000;
        p.second_stop = Some(ladder::StopStep {
            after_s,
            switch_pct: -1.0,
            level_pct: 1.0,
        });
        let ticks = [sold(10, 100.0), sold(2000, 100.0)];
        let mut stops = Stops::new(&deal(false), &ticks, fill(), &p, Side { long: true });
        assert!(stops.on_print(&ticks[0], 10, 100.0).is_none());
        assert_eq!(
            stops.on_print(&ticks[1], 2000, 100.0).map(|e| e.t_ms),
            expected
        );
        if expected.is_none() {
            assert_eq!(stops.after_tape(2000).map(|e| e.t_ms), Some(3000));
        }
    }
}

// ---- StopLoss ---------------------------------------------------------------------------------

#[test]
fn the_stop_fires_on_the_print_after_its_delay() {
    let p = ExitParams {
        stop_loss_pct: -1.0,
        stop_loss_delay_s: 2.0,
        ..params()
    };
    // A print through the stop inside the delay does not fire; one after it does as its series
    // tick closes, at the print's price (a market exit).
    let ticks = tape(&[(1_000, 98.5), (3_000, 98.7)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 3_000 + SERIES_TICK_MS)
    );
    assert!((w.exit.price - 98.7).abs() < 1e-4);
}

/// The fast stop holds the LAST price of a series tick against the level: a sweep through it
/// that prints back inside the same tick does not fire; a tick that closes past it does.
#[test]
fn the_fast_stop_reads_the_last_price_of_its_tick() {
    let p = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: true,
        ..params()
    };
    let ticks = tape(&[
        (1_000, 98.5),
        (1_100, 99.6),
        (2_000, 99.5),
        (3_010, 98.9),
        (3_200, 98.8),
        (4_000, 99.5),
    ]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 3_000 + SERIES_TICK_MS),
        "{w:?}"
    );
    assert!((w.exit.price - 98.8).abs() < 1e-4);
}

/// A short's stop is the buy DIVIDED by `1 + StopLoss/100`, not the long's product mirrored:
/// `-2.5 %` off 100 is 102.564, so a print at 102.53 is still inside it. The core prints it that
/// way on 11 099 short stops of the report (`StopLoss fixed: X`) against 25 for the mirror.
#[test]
fn a_short_stop_divides_the_buy() {
    let p = ExitParams {
        stop_loss_pct: -2.5,
        ..params()
    };
    let inside = tape(&[(1_000, 102.53), (2_000, 102.57)]);
    let w = walk(&deal(true), &inside, fill(), 99.0, &p);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 2_000 + SERIES_TICK_MS),
        "{w:?}"
    );
    assert!((level_off_buy(100.0, -2.5, false) - 100.0 / 0.975).abs() < 1e-9);
    assert!((level_off_buy(100.0, -2.5, true) - 97.5).abs() < 1e-9);
    // A stop on the profit side of a short sits below the buy, by the same division.
    assert!((level_off_buy(100.0, 0.4, false) - 100.0 / 1.004).abs() < 1e-9);
    // A loss of 100 % or more leaves a short no finite price to stop at.
    assert_eq!(level_off_buy(100.0, -100.0, false), f64::INFINITY);
}

fn sold(t_ms: i64, price: f64) -> Tick {
    Tick {
        side: TickSide::Sell,
        ..tick(t_ms, price)
    }
}

/// A book-watching stop (`FastStopLoss` off) that reads the bare ticker price: `StopLossEMA`
/// neither 0 (which adds the series) nor 3, 5, 10 (which average).
fn bare_book_stop() -> ExitParams {
    ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        stop_loss_ema: 7.0,
        ..params()
    }
}

/// A later rung must not rewrite earlier ticker or series decisions across a print gap.
#[test]
fn ladder_book_events_keep_their_own_levels() {
    let mut moments = Vec::new();
    for ema in [7.0, 0.0] {
        for at_tail in [false, true] {
            let p = ExitParams {
                stop_loss_pct: -10.0,
                stop_loss_ema: ema,
                second_stop: Some(ladder::StopStep {
                    after_s: 5.0,
                    switch_pct: -1.0,
                    level_pct: 1.0,
                }),
                ..bare_book_stop()
            };
            let ticks = [sold(1, 100.0), tick(9_500, 100.0)];
            let d = Deal {
                core_name: "CORE_A".into(),
                coin: "XALPHA".into(),
                ..deal(false)
            };
            let mut stops = Stops::new(&d, &ticks, fill(), &p, Side { long: true });
            assert!(stops.on_print(&ticks[0], 1, 100.0).is_none());
            let exit = if at_tail {
                stops.after_tape(9_500)
            } else {
                stops.on_print(&ticks[1], 9_500, 100.0)
            };
            moments.push((ema, at_tail, exit.map(|e| e.t_ms)));
        }
    }
    assert_eq!(
        moments,
        vec![
            (7.0, false, Some(6_450)),
            (7.0, true, Some(6_450)),
            (0.0, false, Some(6_450)),
            (0.0, true, Some(6_450))
        ]
    );
}

/// A warm averaged strike stop cannot fire on a rung before its qualifying ticker cycle.
#[test]
fn ladder_averaged_book_waits_for_the_qualifying_cycle() {
    let p = ExitParams {
        stop_loss_pct: -5.0,
        stop_loss_ema: 3.0,
        second_stop: Some(ladder::StopStep {
            after_s: 5.0,
            switch_pct: -2.0,
            level_pct: 0.0,
        }),
        ..bare_book_stop()
    };
    let ticks = [sold(-1_000, 100.0), sold(100, 99.0), tick(10_000, 100.0)];
    let d = Deal {
        core_name: "CORE_A".into(),
        coin: "XALPHA".into(),
        ..deal(false)
    };
    let w = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 6_450));
}

/// A series tick on a print boundary must not consume the ticker sample that reads that print.
#[test]
fn a_series_boundary_preserves_the_tickers_print_cutoff() {
    let mut p = bare_book_stop();
    p.stop_loss_ema = 0.0;
    p.model.series_tick_ms = TICKER_PERIOD_MS;
    let mut book = BookStop::new(true, 99.0, 0, 0, 1, &p, i64::MIN);
    book.see(&sold(1, 100.0));
    assert!(book.before(TICKER_PERIOD_MS).is_none());
    book.see(&sold(TICKER_PERIOD_MS, 98.0));
    let (exit, trigger) = book
        .before(TICKER_PERIOD_MS + 1)
        .expect("ticker reads the boundary print");
    assert_eq!(
        (exit.t_ms, trigger),
        (TICKER_PERIOD_MS, StopTrigger::Ticker)
    );
}

/// The book-watching stop (`FastStopLoss` off) reads the ticker's BID through the prints that
/// hit it — taker sells — on the ticker's clock, not every print through the level.
#[test]
fn the_book_stop_fires_on_a_sample_of_the_bid_not_on_a_print() {
    let mut book = bare_book_stop();
    book.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::AtCheck;
    // A taker BUY through the level says nothing about the BID; the taker sell at 98.8 does,
    // and the next arrival after it — 4.3 s — fires, at the proxy's price.
    let ticks = vec![
        tick(1_000, 98.5),
        sold(1_500, 99.5),
        sold(2_500, 98.8),
        tick(5_000, 100.0),
    ];
    let w = walk(&deal(false), &ticks, fill(), 101.0, &book);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 2 * TICKER_PERIOD_MS)
    );
    assert!((w.exit.price - 98.8).abs() < 1e-4);
    // The fast stop takes the first series tick closing through the level, whichever side its
    // last print hit.
    let fast = ExitParams {
        fast_stop_loss: true,
        ..book.clone()
    };
    let w = walk(&deal(false), &ticks, fill(), 101.0, &fast);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 1_000 + SERIES_TICK_MS)
    );
}

/// A sample due exactly at the tape's last print reads that print — the loop only reaches the
/// samples before a print, so the tape's end is where it must not be forgotten.
#[test]
fn the_book_stop_takes_the_sample_at_the_last_print() {
    let book = bare_book_stop();
    let w = walk(
        &deal(false),
        &[sold(TICKER_PERIOD_MS, 98.8)],
        fill(),
        101.0,
        &book,
    );
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, TICKER_PERIOD_MS)
    );
    // A sample the tape ends before is not taken: nothing is known past the last print.
    let w = walk(&deal(false), &[sold(1_500, 98.8)], fill(), 101.0, &book);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
}

/// `StopLossEMA` 3 averages the ticker's arrivals as the core does, `(avg·2 + bid)/3`, so a BID
/// just past the level fires only once the average is past it too.
#[test]
fn the_stop_ema_waits_for_the_average() {
    let ticks = vec![
        sold(1_500, 99.5),
        sold(2_500, 98.9),
        sold(9_000, 98.9),
        sold(13_000, 98.9),
    ];
    let mut bare = bare_book_stop();
    bare.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::AtCheck;
    let w = walk(&deal(false), &ticks, fill(), 101.0, &bare);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 2 * TICKER_PERIOD_MS)
    );
    // Arrivals 99.5, then 98.9 from the second on: the average reads 99.5, 99.3, 99.167,
    // 99.078, 99.019, 98.979 — past 99 at the sixth. An EMA of 2/(N + 1) = 0.5 would have
    // crossed at the fourth.
    let smoothed = ExitParams {
        stop_loss_ema: 3.0,
        ..bare
    };
    let w = walk(&deal(false), &ticks, fill(), 101.0, &smoothed);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 6 * TICKER_PERIOD_MS)
    );
}

/// The core keeps the average from its start, so at the fill it remembers the prices before it:
/// the prints before the fill warm it, and can never fire it.
#[test]
fn the_stop_average_is_warm_at_the_fill() {
    let mut smoothed = ExitParams {
        stop_loss_ema: 3.0,
        ..bare_book_stop()
    };
    smoothed.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::AtCheck;
    let after = [sold(1_500, 98.9), sold(30_000, 98.9)];
    let cold = walk(&deal(false), &after, fill(), 101.0, &smoothed);
    assert_eq!(
        (cold.exit.kind, cold.exit.t_ms),
        (ExitKind::Stop, TICKER_PERIOD_MS)
    );
    // A minute at 101 before the fill: the average starts there, seven arrivals of 98.9 leave
    // it at 101 · (2/3)^7 + 98.9 · (1 − (2/3)^7) = 99.02, and the eighth brings it to 98.98.
    let mut warm: Vec<Tick> = (0..30).map(|i| sold(-60_000 + i * 2_000, 101.0)).collect();
    warm.extend(after);
    let w = walk(&deal(false), &warm, fill(), 101.0, &smoothed);
    assert_eq!(w.exit.kind, ExitKind::Stop);
    assert_eq!(w.exit.t_ms, 8 * TICKER_PERIOD_MS, "{w:?}");
    // The prints before the fill, past the level, fire nothing before it.
    let early = vec![sold(-3_000, 98.0), tick(500, 100.0)];
    let w = walk(&deal(false), &early, fill(), 101.0, &bare_book_stop());
    assert!(w.exit.t_ms > 0, "{w:?}");
}

/// A short's book stop watches the bare ASK: `StopLossEMA` averages a long's BID only.
#[test]
fn a_short_book_stop_does_not_average() {
    let mut smoothed = ExitParams {
        stop_loss_ema: 3.0,
        ..bare_book_stop()
    };
    smoothed.model.stop_fill = crate::db::tuner::ticks::settings::StopFill::AtCheck;
    // A taker buy prints at the ASK: 100 before the fill, then 101.1 past a short's stop at 101
    // — which an average of the two, 100.37, would not be.
    let ticks = vec![tick(-1_000, 100.0), tick(1_500, 101.1), tick(5_000, 100.0)];
    let w = walk(&deal(true), &ticks, fill(), 99.0, &smoothed);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, TICKER_PERIOD_MS)
    );
}

/// At `StopLossEMA` 0 the core's price series fires the stop too: a lone print past the level
/// is its tick's point, and fires when the tick closes; a spike among prints near the last
/// point is not the point, and waits for the ticker.
#[test]
fn a_stop_without_averaging_fires_on_the_series_point() {
    let series = ExitParams {
        stop_loss_ema: 0.0,
        ..bare_book_stop()
    };
    let lone = vec![tick(1_000, 98.5), tick(5_000, 100.0)];
    let w = walk(&deal(false), &lone, fill(), 101.0, &series);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 1_250));
    assert!((w.exit.price - 98.5).abs() < 1e-4);
    // The same print after a point at 100 and beside 99.9 in its tick: the point is 99.9.
    let spike = vec![
        tick(900, 100.0),
        tick(1_000, 98.5),
        tick(1_100, 99.9),
        tick(5_000, 100.0),
    ];
    let w = walk(&deal(false), &spike, fill(), 101.0, &series);
    assert_ne!(w.exit.kind, ExitKind::Stop, "{w:?}");
    // At 7 the core reads the bare ticker price and no series.
    let w = walk(&deal(false), &lone, fill(), 101.0, &bare_book_stop());
    assert_ne!(w.exit.kind, ExitKind::Stop, "{w:?}");
}

/// A book stop's sale is a panic sell walked through the book: the verdict holds the model's
/// stop level against the one the core printed, and the moment against the close — never the
/// sale price. A stop the core fired and the model never did is a miss.
#[test]
fn verify_judges_a_book_stop_by_its_level_and_moment() {
    let book = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        ..params()
    };
    let mut d = deal(false);
    d.sell_reason = "StopLoss AutoActivated on price drop: BID = 98.800 ASK: 98.900 \
                     (strategy <S>); StopLoss fixed: 99.000 AllowedDrop: BUY -15.0%"
        .into();
    d.sell_price = 97.0;
    d.close_ms = 4_050;
    let ticks = vec![sold(1_500, 99.5), sold(2_500, 98.8), tick(5_000, 100.0)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::Stop));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert!(v.exit_dev_pct.is_some_and(|dev| dev.abs() < 1e-9));
    // The stored reason cut inside the level: still the book stop, judged by its moment.
    let full = d.sell_reason.clone();
    d.sell_reason = full[..full.find("99.000").expect("level") + 3].to_string();
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(true), "{v:?}");
    d.close_ms = 9_000;
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "five seconds off the moment, {v:?}");
    d.close_ms = 4_050;
    d.sell_reason = full;
    // The core's level elsewhere: not this stop.
    d.sell_reason = d.sell_reason.replace("99.000", "98.000");
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "{v:?}");
    // A tape whose BID never reaches the level: the core stopped, the model did not.
    let calm = vec![sold(1_500, 99.5), tick(5_000, 100.0)];
    let v = verify(&d, &calm, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "{v:?}");
}

/// A market stop's sale sweeps our size through the book: with no level on record — its reason
/// never carries one — the verdict holds the moment it fired, never the sweep's price.
#[test]
fn verify_judges_a_market_stop_without_a_level_by_its_moment() {
    let fast = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: true,
        ..params()
    };
    let mut d = deal(false);
    d.sell_reason = "StopLoss Market Sell".into();
    // 1.4 % past the print that fired it: the sweep, which no print on the tape shows.
    d.sell_price = 97.5;
    d.close_ms = 3_100;
    let ticks = tape(&[(1_000, 99.5), (3_000, 98.9), (5_000, 99.0)]);
    let v = verify(&d, &ticks, &EntryParams::Fact, &fast, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::Stop));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert_eq!(v.exit_dev_pct, None);
    d.close_ms = 9_000;
    let v = verify(&d, &ticks, &EntryParams::Fact, &fast, None, None);
    assert_eq!(v.exit, Some(false), "six seconds off the moment, {v:?}");
}

/// The walk names the rule that fired its stop — the diagnostic report counts them per segment —
/// and names none when the walk closed otherwise.
#[test]
fn the_walk_names_the_rule_that_fired_its_stop() {
    let by_ticker = vec![sold(1_500, 99.5), sold(2_500, 98.8), tick(5_000, 100.0)];
    let w = walk(&deal(false), &by_ticker, fill(), 101.0, &bare_book_stop());
    assert_eq!(w.stop_trigger, Some(StopTrigger::Ticker), "{w:?}");
    let fast = ExitParams {
        fast_stop_loss: true,
        ..bare_book_stop()
    };
    let w = walk(&deal(false), &by_ticker, fill(), 101.0, &fast);
    assert_eq!(w.stop_trigger, Some(StopTrigger::FastTick), "{w:?}");
    // A lone print past the level at `StopLossEMA` 0: its series tick fires 2 s before the
    // ticker's first arrival could.
    let series = ExitParams {
        stop_loss_ema: 0.0,
        ..bare_book_stop()
    };
    let lone = vec![tick(1_000, 98.5), tick(5_000, 100.0)];
    let w = walk(&deal(false), &lone, fill(), 101.0, &series);
    assert_eq!(w.stop_trigger, Some(StopTrigger::Series), "{w:?}");
    // No stop fired: nothing is named.
    let w = walk(&deal(false), &lone, fill(), 101.0, &bare_book_stop());
    assert_ne!(w.exit.kind, ExitKind::Stop, "{w:?}");
    assert_eq!(w.stop_trigger, None);
}
