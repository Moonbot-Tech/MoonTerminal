//! The core's own record as the model's inputs: the stop anchor, the fact's entry, the rule for
//! the search's sample.

use super::*;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::mshot::MshotParams;
use crate::db::tuner::ticks::{Deltas, ExitKind, ModelSettings, simulate, verify};
use crate::feed::types::{Side, Tick};
use crate::market::trade_replay::Coverage;

fn tick(t_ms: i64, price: f64) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// A taker sell — the print a long's book stop reads its BID proxy off.
fn sold(t_ms: i64, price: f64) -> Tick {
    Tick {
        side: Side::Sell,
        ..tick(t_ms, price)
    }
}

const BOOK_REASON: &str = "StopLoss AutoActivated on price drop: BID = 98.800 ASK: 98.900 \
                           (strategy <S>); StopLoss fixed: 99.000 AllowedDrop: BUY -15.0%";

/// A long bought at 100 at 0, stopped by the book, sold at 97 at 4 050.
fn stopped() -> Deal {
    Deal {
        report_uid: 1,
        core_uid: 7,
        core_name: "C".into(),
        strategy_id: 42,
        kind: "PumpsDetection".into(),
        coin: "ACE".into(),
        buy_ms: 0,
        close_ms: 4_050,
        buy_price: 100.0,
        sell_price: 97.0,
        spent: 1_000.0,
        is_short: false,
        sell_reason: BOOK_REASON.into(),
        fact_pnl: -3.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    }
}

/// A 1 % book stop without latency.
fn book() -> ExitParams {
    ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    }
}

fn fact_fill() -> Fill {
    Fill {
        t_ms: 0,
        price: 100.0,
    }
}

/// The archived Exit line of [`stopped`]: the take, then the panic sell's first price past the
/// stop's level at 3 800.
const PANIC_LINE: [(i64, f64); 2] = [(0, 101.0), (3_800, 98.1)];

/// The archive names the moment: the anchor fires the core's stop there, at the report's price
/// — not at the sample the proxy would have fired on, nor at the proxy's price.
#[test]
fn the_anchor_fires_the_facts_own_stop_at_its_moment_and_price() {
    let mut d = stopped();
    d.stop_anchor = Some(StopAnchor::of(&d, &book(), Some(&PANIC_LINE)));
    let ticks = vec![sold(1_500, 99.5), sold(2_500, 98.8), tick(5_000, 100.0)];
    let w = walk(&d, &ticks, fact_fill(), 101.0, &book());
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 3_800));
    assert!((w.exit.price - 97.0).abs() < 1e-9);
    // A variant whose line a print reaches before it sells there: the line is the model's.
    let w = walk(&d, &ticks, fact_fill(), 99.5, &book());
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Take, 1_500));
    // A variant with another stop is back on the proxy: at 2 % the BID never gets there.
    let deeper = ExitParams {
        stop_loss_pct: -2.0,
        ..book()
    };
    let w = walk(&d, &ticks, fact_fill(), 101.0, &deeper);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
}

/// A trade the stop never fired on proves it quiet to the close: the fast stop's print inside
/// that span is not a stop under the trade's own settings, one after it is.
#[test]
fn a_trade_its_stop_never_fired_on_keeps_it_quiet_until_the_close() {
    let mut d = stopped();
    d.sell_reason = "Auto Price Down".into();
    d.close_ms = 6_000;
    let fast = ExitParams {
        fast_stop_loss: true,
        ..book()
    };
    let ticks = vec![tick(1_000, 98.5), tick(7_000, 98.5)];
    let tick_ms = crate::db::tuner::ticks::exit::stops::SERIES_TICK_MS;
    let w = walk(&d, &ticks, fact_fill(), 101.0, &fast);
    assert_eq!(
        w.exit.t_ms,
        1_000 + tick_ms,
        "without the anchor the first print's tick fires"
    );
    d.stop_anchor = Some(StopAnchor::of(&d, &fast, None));
    let w = walk(&d, &ticks, fact_fill(), 101.0, &fast);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 7_000 + tick_ms)
    );
}

/// The anchor is the trade's own stop and entry, nothing near them.
#[test]
fn the_anchor_holds_only_for_the_trades_own_entry_and_stop() {
    let d = stopped();
    let anchor = StopAnchor::of(&d, &book(), None);
    assert!(anchor.holds(&d, fact_fill(), &book()));
    let late = Fill {
        t_ms: 1_500,
        ..fact_fill()
    };
    let deeper_entry = Fill {
        price: 99.9,
        ..fact_fill()
    };
    assert!(!anchor.holds(&d, late, &book()));
    assert!(!anchor.holds(&d, deeper_entry, &book()));
    for other in [
        ExitParams {
            stop_loss_delay_s: 5.0,
            ..book()
        },
        ExitParams {
            fast_stop_loss: true,
            ..book()
        },
        ExitParams {
            stop_loss_ema: 3.0,
            ..book()
        },
        ExitParams {
            stop_loss_pct: -1.5,
            ..book()
        },
    ] {
        assert!(!anchor.holds(&d, fact_fill(), &other), "{other:?}");
    }
    // The take and the sell line are not the stop's: a variant moving them keeps the anchor.
    let other_take = ExitParams {
        sell_price_pct: 3.0,
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        ..book()
    };
    assert!(anchor.holds(&d, fact_fill(), &other_take));
}

/// The stop's moment: the archive's jump past the level, else the close.
#[test]
fn the_stop_moment_comes_from_the_archive_then_the_close() {
    let d = stopped();
    assert_eq!(StopAnchor::of(&d, &book(), None).fired, Some((4_050, 97.0)));
    // The archived line: the take, a step, and the panic sell's first price past 99.
    let line = [(0, 101.0), (1_000, 100.8), (3_600, 98.1), (4_040, 97.0)];
    let anchor = StopAnchor::of(&d, &book(), Some(&line));
    assert_eq!(anchor.fired, Some((3_600, 97.0)));
    assert_eq!(anchor.quiet_until_ms, 3_600);
}

/// The verdict tests the proxy, never the anchor: the core's activation 5 s after the proxy's
/// is a miss of the model, even though a variant would sell exactly where the core did.
#[test]
fn the_verdict_never_leans_on_the_anchor() {
    let mut d = stopped();
    d.close_ms = 8_100;
    let line = [(0, 101.0), (8_000, 98.1)];
    d.stop_anchor = Some(StopAnchor::of(&d, &book(), Some(&line)));
    let ticks = vec![sold(1_500, 99.5), sold(2_500, 98.8), tick(9_000, 100.0)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &book(), None, None);
    assert_eq!(
        v.exit,
        Some(false),
        "the series fired at 2.75 s, the core at 8 s: {v:?}"
    );
    let w = walk(&d, &ticks, fact_fill(), 101.0, &book());
    assert_eq!((w.exit.t_ms, w.exit.price), (8_000, 97.0));
}

/// The entry order's placement at its creation: the archived line's first point when the line
/// starts at the stamp; the buy price when the archive answered with lines and none is the
/// entry's — the core files a line only when the order moved; nothing when the archive gave no
/// lines, when the line starts elsewhere, or without a stamp.
#[test]
fn the_entry_placement_is_what_the_record_proves() {
    let mut d = stopped();
    d.buy_ms = 10_000;
    d.buy_set_ms = Some(2_000);
    let line = [(2_000, 98.5), (6_000, 99.4), (10_000, 100.0)];
    let answered = |entry| OwnLines {
        entry,
        exit: None,
        answered: true,
    };
    assert_eq!(entry_placement(&d, answered(Some(&line))), Some(98.5));
    assert_eq!(
        entry_placement(&d, answered(None)),
        Some(100.0),
        "never moved"
    );
    assert_eq!(
        entry_placement(&d, OwnLines::default()),
        None,
        "no lines: no proof"
    );
    let late = [(5_000, 98.5), (10_000, 100.0)];
    assert_eq!(
        entry_placement(&d, answered(Some(&late))),
        None,
        "not the stamp's line"
    );
    d.buy_set_ms = None;
    assert_eq!(entry_placement(&d, answered(Some(&line))), None, "no stamp");
}

/// A variant running the trade's own entry settings filled where the report says — the entry
/// model is for the settings the core never ran.
#[test]
fn a_variant_running_the_trades_own_entry_fills_at_the_fact() {
    let mut d = stopped();
    d.kind = "MoonShot".into();
    let own = EntryParams::MoonShot(MshotParams::default());
    prepare_deal(
        &mut d,
        &own,
        &book(),
        OwnLines::default(),
        &Coverage::none(),
    );
    // A tape the corridor never reaches: the model alone would not fill at all.
    let ticks = vec![
        tick(-20_000, 100.0),
        tick(-10_000, 100.0),
        tick(5_000, 100.0),
    ];
    let outcome = simulate(&d, &ticks, &own, &book(), None);
    assert_eq!(outcome.fill, Some(fact_fill()));
    let other = EntryParams::MoonShot(MshotParams {
        price_pct: 9.0,
        ..MshotParams::default()
    });
    assert_eq!(simulate(&d, &ticks, &other, &book(), None).fill, None);
}

#[test]
fn only_a_reproduced_trade_is_searched() {
    let verdict = |entry, exit| Verdict {
        entry,
        entry_dev_pct: None,
        exit,
        exit_dev_pct: None,
        fill: None,
        exit_kind: None,
        line_points: None,
        entry_finding: match entry {
            None => crate::db::tuner::ticks::verify::EntryFinding::Fact,
            Some(true) => crate::db::tuner::ticks::verify::EntryFinding::Hit,
            Some(false) => crate::db::tuner::ticks::verify::EntryFinding::Off,
        },
        exit_finding: match exit {
            None => crate::db::tuner::ticks::verify::ExitFinding::Unjudged(
                crate::db::tuner::ticks::verify::Unjudged::OtherRule,
            ),
            Some(true) => crate::db::tuner::ticks::verify::ExitFinding::Hit,
            Some(false) => crate::db::tuner::ticks::verify::ExitFinding::Miss(
                crate::db::tuner::ticks::verify::ExitMiss::NoLevel,
            ),
        },
        fill_clock_ms: None,
        rules: crate::db::tuner::ticks::verify::RuleFlags::default(),
    };
    assert!(fit_for_search(&verdict(None, Some(true))));
    assert!(fit_for_search(&verdict(Some(true), Some(true))));
    assert!(!fit_for_search(&verdict(Some(false), Some(true))));
    assert!(!fit_for_search(&verdict(Some(true), Some(false))));
    // An exit the model has no rule for is not a pass.
    assert!(!fit_for_search(&verdict(Some(true), None)));
}

// ---- the trailing's own anchor ---------------------------------------------------------------

/// A spread quoted at each moment: the bid by a taker sell, the ask by a taker buy.
fn quotes(points: &[(i64, f64, f64)]) -> Vec<Tick> {
    points
        .iter()
        .flat_map(|&(t, bid, ask)| [sold(t, bid), tick(t, ask)])
        .collect()
}

/// A 1 % trailing, the take far away, no stop, no latency.
fn trailing() -> ExitParams {
    ExitParams {
        sell_price_pct: 50.0,
        trailing_pct: -1.0,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    }
}

/// Up to a middle of 102.1, then down to 101.0: a 1 % trailing fires at the 6 450 arrival, a 2 %
/// one never does.
fn peak_and_fall() -> Vec<Tick> {
    quotes(&[
        (100, 100.0, 100.2),
        (3_000, 102.0, 102.2),
        (5_000, 100.9, 101.1),
        (9_000, 100.9, 101.1),
    ])
}

/// A trade the trailing closed hands its firing only to a variant running the same trailing: a
/// 2 % trailing, or none at all, sells where its own walk says — not at the fact's moment and price.
#[test]
fn a_different_trailing_does_not_inherit_the_facts_exit() {
    let mut d = stopped();
    d.sell_reason = "TrailingStop AutoActivated on price drop: ASK = 101.10 LastPrice = 101.00; \
                     PeakPrice = 102.10"
        .into();
    d.close_ms = 3_900;
    d.sell_price = 100.5;
    d.stop_anchor = Some(StopAnchor::of(&d, &trailing(), None));
    let fired = d.stop_anchor.and_then(|a| a.fired).expect("a stopped fact");
    let ticks = peak_and_fall();
    let own = walk(&d, &ticks, fact_fill(), 150.0, &trailing());
    assert_eq!((own.exit.t_ms, own.exit.price), fired, "{own:?}");
    for other in [
        ExitParams {
            trailing_pct: -2.0,
            ..trailing()
        },
        ExitParams {
            trailing_pct: 0.0,
            ..trailing()
        },
    ] {
        let w = walk(&d, &ticks, fact_fill(), 150.0, &other);
        assert_ne!((w.exit.t_ms, w.exit.price), fired, "{other:?}: {w:?}");
        assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd, "{other:?}: {w:?}");
    }
}

/// A trade no stop closed proves quiet only the stops it ran: a variant with the same stop and a
/// different trailing keeps its trailing live — it fires at 6 450 into the bid — while the
/// fact's own trailing stays quiet to the close.
#[test]
fn the_stop_and_the_trailing_are_quiet_each_under_its_own_settings() {
    let mut d = stopped();
    d.sell_reason = "Auto Price Down".into();
    d.close_ms = 60_000;
    let fact = ExitParams {
        trailing_pct: -3.0,
        ..trailing()
    };
    d.stop_anchor = Some(StopAnchor::of(&d, &fact, None));
    let ticks = peak_and_fall();
    let w = walk(&d, &ticks, fact_fill(), 150.0, &trailing());
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 6_450), "{w:?}");
    assert!((w.exit.price - 100.9).abs() < 1e-4, "{w:?}");
    // The fact's own trailing is the one the close proves quiet.
    d.stop_anchor = Some(StopAnchor::of(&d, &trailing(), None));
    let w = walk(&d, &ticks, fact_fill(), 150.0, &trailing());
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd, "{w:?}");
}

/// `StopLoss Market Sell` is the core's name for both groups' sale: the fact's firing goes to a
/// variant only when its stop AND its trailing are the fact's.
#[test]
fn a_market_sell_hands_its_firing_only_when_both_groups_hold() {
    let fact = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        ..trailing()
    };
    let mut d = stopped();
    d.sell_reason = "StopLoss Market Sell".into();
    d.stop_anchor = Some(StopAnchor::of(&d, &fact, None));
    let ticks = quotes(&[(100, 99.9, 100.1), (9_000, 99.9, 100.1)]);
    let own = walk(&d, &ticks, fact_fill(), 150.0, &fact);
    assert_eq!(
        (own.exit.kind, own.exit.t_ms, own.exit.price),
        (ExitKind::Stop, 4_050, 97.0),
        "{own:?}"
    );
    for other in [
        ExitParams {
            trailing_pct: -2.0,
            ..fact
        },
        ExitParams {
            stop_loss_pct: -2.0,
            ..fact
        },
    ] {
        let w = walk(&d, &ticks, fact_fill(), 150.0, &other);
        assert_ne!(w.exit.kind, ExitKind::Stop, "{other:?}: {w:?}");
    }
}

/// A fact whose take the core booked 3 s after the buy: its sell timers run from the booked take,
/// so the first Price Down step lands at 4 000, not at 1 000 — the print at 2 500 is not a sale,
/// the one at 4 500 is.
#[test]
fn a_late_booked_facts_sell_timers_run_from_the_booked_take() {
    let p = ExitParams {
        sell_price_pct: 1.0,
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_allowed_drop_pct: 0.1,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    };
    let mut d = stopped();
    d.sell_reason = "Auto Price Down".into();
    d.close_ms = 60_000;
    let line = [(3_000, 101.0)];
    d.stop_anchor = Some(StopAnchor::of(&d, &p, Some(&line)));
    assert_eq!(d.stop_anchor.map(|a| a.entry_ms), Some(3_000));
    let ticks = vec![tick(2_500, 100.6), tick(4_500, 100.6), tick(70_000, 100.0)];
    let outcome = simulate(&d, &ticks, &EntryParams::Fact, &p, None);
    let exit = outcome.exit.expect("an exit");
    assert_eq!((exit.kind, exit.t_ms), (ExitKind::Line, 4_500), "{exit:?}");
}
