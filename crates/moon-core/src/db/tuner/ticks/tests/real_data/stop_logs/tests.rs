//! Deterministic diagnostic helpers use synthetic readings and dates only.

use super::*;

/// The override grammar must reach the actual production model choice and sanitization.
#[test]
fn parses_fast_price_override() {
    let parse = super::super::parse_fast_price_override;
    assert_eq!(parse("last"), FastPrice::LastOfTick);
    assert_eq!(parse("vwap5"), FastPrice::VwapLastTrades(5));
    assert_eq!(parse("vwap1"), FastPrice::VwapLastTrades(1));
    assert_eq!(parse("vwap16"), FastPrice::VwapLastTrades(16));
    assert_eq!(
        parse("ema0.25"),
        FastPrice::TradeEma {
            alpha: 0.25,
            qty_weighted: false
        }
    );
    assert_eq!(
        parse("emaq0.25"),
        FastPrice::TradeEma {
            alpha: 0.25,
            qty_weighted: true
        }
    );
    assert!(std::panic::catch_unwind(|| parse("unknown")).is_err());
}

/// Invalid estimator ranges must fail loudly rather than silently testing a different rule.
#[test]
fn fast_price_override_rejects_sanitized_values() {
    for value in [
        "vwap0", "vwap17", "ema0", "ema-0.1", "ema1.1", "emaNaN", "emaqinf",
    ] {
        assert!(
            std::panic::catch_unwind(|| super::super::parse_fast_price_override(value)).is_err(),
            "{value}"
        );
    }
    assert!(matches!(
        super::super::parse_fast_price_override("ema1"),
        FastPrice::TradeEma { alpha: 1.0, .. }
    ));
}

/// Fill overrides must select the production rule and reject misspellings.
#[test]
fn parses_stop_fill_override() {
    use crate::db::tuner::ticks::settings::StopFill;
    let parse = super::super::parse_stop_fill_override;
    assert_eq!(parse("check"), StopFill::AtCheck);
    assert_eq!(parse("market"), StopFill::MarketAfterLatency);
    assert!(std::panic::catch_unwind(|| parse("unknown")).is_err());
}

/// Every A/B arm must use the same indices, with holes before the fill still admitted.
#[test]
fn ab_drops_holed_deals_from_every_arm() {
    use crate::db::tuner::ticks::gap::TapeGap;
    let mut kept = Vec::new();
    for end in [None, Some(10), Some(-1)] {
        let (mut k, _, _) = cause_fixture();
        k.deal.gap = end.map(|to_ms| TapeGap {
            from_ms: -10,
            to_ms,
            fact_line: None,
            fact_stop: None,
        });
        kept.push(k);
    }
    assert_eq!(ab_population(&kept), vec![0, 2]);
}

/// Selection is based on each deal's own stop sign, independently of factual stop reasons.
#[test]
fn positive_stop_population_selects_own_positive_stops() {
    let mut kept = Vec::new();
    for (i, pct) in [-1.0, 0.0, 2.0, 3.0].into_iter().enumerate() {
        let (mut k, _, _) = cause_fixture();
        k.exit.stop_loss_pct = pct;
        k.deal.report_uid = i as i64;
        kept.push(k);
    }
    assert_eq!(
        positive_stop_population(&kept)
            .map(|k| k.deal.report_uid)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
}

/// Same timestamp with a changed side opens a new trade; separated runs stay separate.
#[test]
fn trades_group_same_ms_and_side() {
    let ticks = [
        diag_tick(1, 100.0, Side::Buy),
        diag_tick(1, 98.0, Side::Buy),
        diag_tick(1, 99.0, Side::Sell),
        diag_tick(2, 100.0, Side::Sell),
    ];
    assert_eq!(
        group_trades(&ticks)
            .iter()
            .map(|g| g.len())
            .collect::<Vec<_>>(),
        vec![2, 1, 1]
    );
}

/// Grouped candidates must retain quantities instead of averaging trade prices equally.
#[test]
fn vwap_last_trades_weights_by_qty() {
    let ticks = [
        Tick {
            qty: 3.0,
            ..diag_tick(1, 100.0, Side::Buy)
        },
        diag_tick(2, 96.0, Side::Buy),
    ];
    assert_eq!(
        Candidate::Trade(FastPrice::VwapLastTrades(2), false).price(&ticks, 2, Side::Sell),
        Some(99.0)
    );
}

/// The diagnostic must share production's one-step-per-trade semantics.
#[test]
fn ema_over_trades_ignores_split_prints() {
    let split = [
        diag_tick(1, 100.0, Side::Buy),
        diag_tick(2, 96.0, Side::Buy),
        diag_tick(2, 100.0, Side::Buy),
    ];
    let whole = [
        split[0],
        Tick {
            qty: 2.0,
            ..diag_tick(2, 98.0, Side::Buy)
        },
    ];
    let c = Candidate::Trade(
        FastPrice::TradeEma {
            alpha: 0.25,
            qty_weighted: false,
        },
        false,
    );
    assert_eq!(c.price(&split, 2, Side::Sell), Some(99.5));
    assert_eq!(c.price(&whole, 2, Side::Sell), Some(99.5));
}

/// Prev must omit the entire latest trade, including all its split prints.
#[test]
fn prev_is_one_trade_earlier() {
    let ticks = [
        diag_tick(1, 100.0, Side::Buy),
        diag_tick(2, 98.0, Side::Buy),
        diag_tick(2, 96.0, Side::Buy),
    ];
    assert_eq!(
        Candidate::Trade(FastPrice::VwapLastTrades(2), true).price(&ticks, 2, Side::Sell),
        Some(100.0)
    );
}

/// Keep synthetic tapes independent of logged market identifiers.
fn diag_tick(at: i64, price: f32, side: Side) -> Tick {
    Tick {
        time_ms: at as f64,
        price,
        qty: 1.0,
        side,
    }
}

/// A stop case with a far-future model close exposes cause-window composition.
fn cause_fixture() -> (Kept, StopFire, LineWalk) {
    let exit = ExitParams {
        stop_loss_pct: -1.0,
        stop_loss_delay_s: 1.0,
        model: ModelSettings {
            book_stop_time_ms: 100,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    };
    let mut deal = super::super::super::deal();
    deal.is_short = false;
    deal.buy_ms = 0;
    deal.buy_price = 100.0;
    let k = Kept {
        deal,
        ticks: vec![diag_tick(5000, 98.0, Side::Sell)],
        exit,
        fill: Fill {
            t_ms: 0,
            price: 100.0,
        },
    };
    let fire = core_log::parse_stop_fire(
        10000,
        "CORE_A",
        "StopLoss AutoActivated BID = 98 ASK: 99 LastPrice = 98 BuyPrice = 100 StopLoss fixed: 99",
    )
    .unwrap();
    let w = walk(&k, None);
    (k, fire, w)
}

/// Restricting the cause to the gap probe would lose a genuine crossing earlier after fill.
#[test]
fn cause_window_spans_fill_to_fire() {
    let (mut k, fire, mut w) = cause_fixture();
    k.ticks = vec![
        diag_tick(999, 95.0, Side::Sell),
        diag_tick(1500, 98.0, Side::Sell),
        diag_tick(10101, 90.0, Side::Sell),
        diag_tick(10000, 100.0, Side::Sell),
    ];
    w.exit.kind = ExitKind::OpenAtWindowEnd;
    w.exit.t_ms = 20000;
    w.stop_level = Some(99.0);
    let case = CauseCase::new(&k, &fire, &w, 99.0, &[]);
    assert_eq!(case.extreme, Some(98.0));
    assert_eq!(case.class, "crossed_not_fired");
}

/// A short's adverse book extreme is on buys, and uses max rather than min.
#[test]
fn tape_extreme_short_reads_buys() {
    let ticks = [
        diag_tick(1000, 104.0, Side::Sell),
        diag_tick(1001, 102.0, Side::Buy),
        diag_tick(1002, 103.0, Side::Buy),
    ];
    assert_eq!(tape_extreme(&ticks, 1000, 1002, false, true), Some(103.0));
}

/// The armed boundary belongs to the live window, while the previous millisecond does not.
#[test]
fn armed_boundary_is_inclusive() {
    let (mut k, fire, mut w) = cause_fixture();
    w.exit.kind = ExitKind::OpenAtWindowEnd;
    w.exit.t_ms = 20000;
    w.stop_level = Some(99.0);
    k.ticks = vec![
        diag_tick(999, 98.0, Side::Sell),
        diag_tick(10000, 100.0, Side::Sell),
    ];
    assert_eq!(
        CauseCase::new(&k, &fire, &w, 99.0, &[]).class,
        "crossed_before_armed"
    );
    k.ticks[0].time_ms = 1000.0;
    assert_eq!(
        CauseCase::new(&k, &fire, &w, 99.0, &[]).class,
        "crossed_not_fired"
    );
}

/// Invalid close windows must not slice backwards; budget exhaustion must stop retention.
#[test]
fn collector_skips_reversed_windows_and_latches_budget() {
    let mut diag = StopLogDiag {
        active: true,
        kept: Vec::new(),
        skipped_big: 0,
        skipped_window: 0,
        dropped_budget: 0,
        retained_prints: RETAIN_BUDGET_PRINTS - 1,
    };
    let mut deal = super::super::super::deal();
    deal.buy_ms = 1_000_000;
    deal.close_ms = 0;
    let exit = ExitParams::default();
    let ticks = [super::super::super::tick(500_000, 99.0, Side::Buy)];
    diag.add(&deal, &ticks, &exit, None);
    assert_eq!(diag.skipped_window, 1);
    deal.buy_ms = 10_000;
    deal.close_ms = 20_000;
    let ticks = [
        super::super::super::tick(10_000, 99.0, Side::Buy),
        super::super::super::tick(20_000, 98.0, Side::Sell),
    ];
    diag.add(&deal, &ticks, &exit, None);
    diag.add(&deal, &ticks[..1], &exit, None);
    assert_eq!(diag.dropped_budget, 2);
    assert_eq!(diag.retained_prints, RETAIN_BUDGET_PRINTS - 1);
    assert!(diag.kept.is_empty());
}

/// Equal timestamps of distinct events must survive; a shared event keeps its nearest deal.
#[test]
fn event_ownership_uses_identity_and_distance() {
    let ids = [
        Some((("CORE_A".into(), 0), 500)),
        Some((("CORE_A".into(), 1), 500)),
        Some((("CORE_A".into(), 0), 100)),
        Some((("CORE_B".into(), 0), 20)),
        None,
    ];
    assert_eq!(
        exclusive_owners(&ids),
        (vec![true, false, false, false, false], 1)
    );
}

/// Miss penalties must not inflate real-stop quantiles or the fitted allowance.
#[test]
fn timing_separates_real_errors_from_miss_penalties() {
    let mut timing = Timing::default();
    timing.add(Some(110), 100, 20);
    timing.add(None, 100, 20);
    timing.add(None, 100, 20);
    assert_eq!(quantile(&mut timing.abs.clone(), 0.9), Some(10.0));
    assert_eq!(timing.second, 1);
    assert_eq!(timing.n, 3);
    assert!(timing.render().contains("penalised med|dt| 60000.000"));
}

/// Splitting relative to the first sampled day would swap fit and held when the range changes.
#[test]
fn half_uses_absolute_utc_day_parity() {
    let oct4 = 1_791_072_000_000;
    assert_eq!(day_half(oct4), Half::Fit);
    assert_eq!(day_half(oct4 + 86_400_000), Half::Held);
    assert_eq!(day_half(oct4 + 86_399_999), Half::Fit);
}

/// Crossing sets include a long's ask as well as a short's, with mirrored directions.
#[test]
fn reading_crossings_mirror_sides_and_count_long_ask() {
    assert_eq!(
        fired_by(true, 1.0, [None, None, None, Some(0.99), None, None]),
        "anoW"
    );
    assert_eq!(
        fired_by(
            true,
            1.0,
            [Some(0.99), Some(1.01), Some(1.02), Some(1.03), None, None]
        ),
        "bnoW"
    );
    assert_eq!(
        fired_by(
            false,
            1.0,
            [Some(0.99), Some(0.98), Some(0.97), Some(1.01), None, None]
        ),
        "anoW"
    );
    assert_eq!(
        fired_by(
            true,
            1.0,
            [Some(0.99), Some(0.99), Some(1.02), None, None, None]
        ),
        "benoW"
    );
    assert_eq!(
        fired_by(true, 1.0, [None, None, Some(1.01), None, None, None]),
        "-noW"
    );
    for (long, other, weighted) in [(true, 1.01, 0.99), (false, 0.99, 1.01)] {
        assert_eq!(
            fired_by(
                long,
                1.0,
                [
                    Some(other),
                    Some(other),
                    Some(other),
                    Some(other),
                    Some(weighted),
                    Some(other)
                ]
            ),
            "w"
        );
    }
}

/// Quantity weighting must differ from an arithmetic mean and exclude the open boundary.
#[test]
fn vwap_weights_by_qty() {
    let ticks = candidate_tape();
    assert_eq!(
        Candidate::Vwap(2, false).price(&ticks, 2000, Side::Sell),
        Some(1.75)
    );
    assert_eq!(
        Candidate::Vwap(2, true).price(&ticks, 2000, Side::Sell),
        Some(1.0)
    );
}

/// Using print counts instead of held durations would change this two-second average.
#[test]
fn twap_holds_the_price_between_prints() {
    let ticks = [
        super::super::super::tick(0, 1.0, Side::Sell),
        super::super::super::tick(1000, 2.0, Side::Buy),
    ];
    assert_eq!(
        Candidate::Twap(2).price(&ticks, 2000, Side::Sell),
        Some(1.5)
    );
    assert_eq!(Candidate::Twap(3).price(&ticks, 2000, Side::Sell), None);
}

/// Including earlier or future prints would change the last-two quantity-weighted mean.
#[test]
fn vwap_last_n_takes_the_last_prints() {
    let ticks = candidate_tape();
    assert_eq!(
        Candidate::LastN(2).price(&ticks, 2000, Side::Sell),
        Some(1.75)
    );
    assert_eq!(
        Candidate::Last(false).price(&ticks, 2000, Side::Sell),
        Some(2.0)
    );
}

/// A boundary print and a future print distinguish interval selection from averaging.
fn candidate_tape() -> Vec<Tick> {
    let mut ticks = vec![
        super::super::super::tick(0, 3.0, Side::Buy),
        super::super::super::tick(1000, 1.0, Side::Sell),
        super::super::super::tick(2000, 2.0, Side::Buy),
        super::super::super::tick(2001, 3.0, Side::Sell),
    ];
    ticks[1].qty = 1.0;
    ticks[2].qty = 3.0;
    ticks
}

/// Independent cases pin every rule and overlapping evidence must keep the earlier rule.
#[test]
fn classifies_each_cause_in_rule_order() {
    let base = CauseInput {
        model_stop: false,
        dt: 0,
        allow: 100,
        level: Some(1.0),
        fixed: 1.0,
        near_prints: 1,
        crossed: true,
        crossed_before: false,
    };
    let cases = [
        (
            CauseInput {
                model_stop: true,
                level: None,
                ..base
            },
            "stopped_ok",
        ),
        (
            CauseInput {
                model_stop: true,
                dt: -101,
                ..base
            },
            "stopped_off_time",
        ),
        (
            CauseInput {
                dt: -101,
                level: None,
                ..base
            },
            "other_exit_first",
        ),
        (
            CauseInput {
                level: None,
                near_prints: 0,
                ..base
            },
            "no_stop_model",
        ),
        (
            CauseInput {
                level: Some(1.01),
                crossed: false,
                ..base
            },
            "level_differs",
        ),
        (
            CauseInput {
                near_prints: 0,
                crossed: false,
                ..base
            },
            "tape_gap",
        ),
        (
            CauseInput {
                crossed: false,
                ..base
            },
            "tape_not_crossed",
        ),
        (
            CauseInput {
                crossed: false,
                crossed_before: true,
                ..base
            },
            "crossed_before_armed",
        ),
        (base, "crossed_not_fired"),
    ];
    for (input, expected) in cases {
        assert_eq!(classify(input), expected);
    }
    assert_eq!(
        classify(CauseInput {
            model_stop: true,
            dt: 100,
            ..base
        }),
        "stopped_ok"
    );
    assert_eq!(
        classify(CauseInput {
            model_stop: true,
            dt: 101,
            ..base
        }),
        "stopped_off_time"
    );
}

/// Preserve the parent bench's upper-middle median and p90 convention on even samples.
#[test]
fn quantile_even_sample_matches_existing_bench() {
    let mut sample = [4.0, 1.0, 3.0, 2.0];
    assert_eq!(quantile(&mut sample, 0.5), Some(3.0));
    assert_eq!(quantile(&mut sample, 0.9), Some(4.0));
    assert_eq!(quantile(&mut sample, 0.1), Some(1.0));
    assert_eq!(quantile(&mut [], 0.5), None);
    assert_eq!(
        quantile(&mut [0.123456, 0.234567, 0.345678], 0.5),
        Some(0.234567)
    );
}

/// Dropping hard deals must not beat higher all-deal agreement or default proximity.
#[test]
fn period_selection_counts_misses_and_breaks_ties() {
    assert_eq!(
        pick_period(
            &[
                (1800, vec![Some(1), None, None]),
                (2200, vec![Some(200), Some(-300), Some(400)]),
            ],
            2150
        ),
        2200
    );
    assert_eq!(
        pick_period(
            &[
                (1800, vec![Some(100), Some(200)]),
                (2150, vec![Some(100), Some(200)]),
            ],
            2150
        ),
        2150
    );
    assert_eq!(
        pick_period(
            &[
                (2150, vec![Some(1), None, None]),
                (2200, vec![Some(1), Some(2000), Some(3000)]),
            ],
            2150
        ),
        2200
    );
}

/// Wrapping to an unsigned hour would turn a one-second lead into 3599 seconds.
#[test]
fn clock_offset_wraps_with_sign() {
    assert_eq!(clock_offset_s(10, 11), -1);
    assert_eq!(clock_offset_s(3599, 0), -1);
    assert_eq!(clock_offset_s(0, 3599), 1);
}

/// Independently known constant paired improvements preserve their sign under resampling.
#[test]
fn bootstrap_is_deterministic_and_paired() {
    let days = [
        (1, 10.0, 8.0),
        (1, 20.0, 18.0),
        (3, 30.0, 28.0),
        (5, 40.0, 38.0),
    ];
    let ci = bootstrap_diff(&days, 200, 0x9E3779B97F4A7C15);
    assert_eq!(ci, bootstrap_diff(&days, 200, 0x9E3779B97F4A7C15));
    assert_eq!(ci, (2.0, 2.0));
    let identical = [(1, 10.0, 10.0), (1, 20.0, 20.0), (3, 30.0, 30.0)];
    assert_eq!(
        bootstrap_diff(&identical, 200, 0x9E3779B97F4A7C15),
        (0.0, 0.0)
    );
    let opposite = [(1, 20.0, 10.0), (3, 10.0, 20.0)];
    let ci = bootstrap_diff(&opposite, 200, 0x9E3779B97F4A7C15);
    assert_eq!(ci, (-10.0, 10.0));
    let mut unbalanced = vec![(1, 20.0, 10.0); 100];
    unbalanced.push((3, 10.0, 20.0));
    assert_eq!(
        bootstrap_diff(&unbalanced, 200, 0x9E3779B97F4A7C15),
        (-10.0, 10.0)
    );
    let paired = [
        (1, penalised_error(None), penalised_error(Some(10))),
        (1, penalised_error(Some(20)), penalised_error(None)),
        (3, penalised_error(None), penalised_error(Some(30))),
    ];
    assert_eq!(
        quantile(&mut paired.iter().map(|p| p.1).collect::<Vec<_>>(), 0.5),
        Some(60_000.0)
    );
    assert_eq!(
        quantile(&mut paired.iter().map(|p| p.2).collect::<Vec<_>>(), 0.5),
        Some(30.0)
    );
    assert_eq!(
        bootstrap_diff(&paired, 200, 0x9E3779B97F4A7C15),
        (0.0, 59_970.0)
    );
}
