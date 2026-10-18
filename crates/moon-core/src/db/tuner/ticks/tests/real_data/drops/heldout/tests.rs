//! Synthetic count and partition oracles for the frozen held-out protocol.

use super::*;

/// Invalid frozen points must stay misses inside the independently frozen prodwin cohort.
#[test]
fn prodwin_membership_independent_of_frozen_rule() {
    let (mut r, deal) = started_deal();
    r.covered = Coverage::one((0, 2_000_000));
    r.production_spans = production_spans(&r, &[deal], 420_000, 60_000);
    let mut model = fit_rules(&[scored(true, false)], &points(), None);
    model.r0.w_s = -1;
    assert!(in_production_window(&r, 420));
    let totals = evaluate(&[r], &[0], &model, 420, "test", false);
    assert_eq!(
        (
            totals[2].scorable,
            totals[2].prodwin,
            totals[2].prodwin_joint
        ),
        (1, 1, 0)
    );
}

/// A complete detect envelope alone cannot bypass production's required deal spans.
#[test]
fn deal_without_held_required_spans_is_not_prodwin() {
    let (mut r, deal) = started_deal();
    assert_eq!(refusal(&r, 420), None);
    r.production_spans = production_spans(&r, std::slice::from_ref(&deal), 420_000, 60_000);
    assert!(!in_production_window(&r, 420));
    r.covered = Coverage::one((0, 2_000_000));
    r.production_spans = production_spans(&r, &[deal], 420_000, 60_000);
    assert!(in_production_window(&r, 420));
}

/// One factual order cannot qualify two detects, and tied nearest orders are refused.
#[test]
fn deal_join_is_one_to_one() {
    let (r, mut deal) = started_deal();
    deal.buy_set_ms = Some(600_100);
    let mut later = deal.clone();
    later.buy_set_ms = Some(601_000);
    let deals = [later, deal.clone()];
    let mut consumed = std::collections::HashSet::new();
    assert_eq!(join_deal(&r, &deals, &mut consumed), Ok(Some(1)));
    assert_eq!(join_deal(&r, &deals, &mut consumed), Ok(Some(0)));
    assert_eq!(join_deal(&r, &deals, &mut consumed), Ok(None));
    assert_eq!(
        join_deal(&r, &[deal.clone(), deal.clone()], &mut Default::default()),
        Err(())
    );
    deal.buy_set_ms = None;
    assert_eq!(join_deal(&r, &[deal], &mut Default::default()), Ok(Some(0)));
}

/// Removing one-second wall points must preserve dense indices used by frozen rule scoring.
#[test]
fn grid_has_no_wall_slot_step_1000_points() {
    for (index, point) in FitPoint::grid().enumerate() {
        assert!(!(point.wall_slots && point.step_ms == 1000));
        assert_eq!(point_index(point), Some(index));
    }
}

/// Neutral rows expose deliberate day partitions without any real log identifiers.
fn row(day: i64) -> Row {
    let mut detect=logs::parse_core_log("CORE_A","1970-01-01","00:10:00.000 INFO DropsDetection: <STRAT_A> XALPHA DailyVol: 1m PriceIsLow: false xPriceDelta: 1.0\n").records.remove(0);
    detect.detect_ms = day * 86_400_000 + 600_000;
    Row {
        detect,
        params: Some(DropsParams {
            max_time_s: 20,
            price_ma_s: 2,
            last_price_ma: 1,
            price_delta_pct: 1.0,
            ..Default::default()
        }),
        ticks: Vec::new(),
        covered: Coverage::one((day * 86_400_000, day * 86_400_000 + 602_000)),
        refusal: None,
        dump_only: false,
        strategy_id: Some(7),
        production_spans: Vec::new(),
        production_covered: None,
        deal_joined: false,
        deal_ambiguous: false,
        buy_ms_fallback: false,
        from_dump: None,
    }
}

/// A pair of grid points permits independent forced-hit patterns without simulating prices.
fn points() -> [FitPoint; 2] {
    let first = FitPoint {
        w_s: 2,
        k: 1,
        j: 0,
        step_ms: 1000,
        extend: false,
        current: Current::Print,
        wall_slots: false,
    };
    [first, FitPoint { w_s: 3, ..first }]
}

/// Encoded synthetic measurements represent known outcomes, not the selector's answers.
fn scored(first: bool, second: bool) -> Scored {
    let p = points();
    let mut bits = vec![0; (point_index(p[1]).unwrap() + 1).div_ceil(64)];
    for (point, hit) in p.into_iter().zip([first, second]) {
        let i = point_index(point).unwrap();
        if hit {
            bits[i / 64] |= 1 << (i % 64);
        }
    }
    Scored {
        params: row(0).params.unwrap(),
        passing: bits,
    }
}

/// A known three-day population must use ceil(D/2), regardless of row ordering.
#[test]
fn split_by_date_never_mixes_a_day() {
    let rows = [row(2), row(0), row(1), row(2), row(0)];
    let split = split_by_date(&rows);
    assert_eq!((split.fit_dates, split.test_dates), (2, 1));
    assert_eq!(split.fit, [1, 2, 4]);
    assert_eq!(split.test, [0, 3]);
}

/// Opposite held-out preferences cannot change a winner fitted without test rows.
#[test]
fn rule_chosen_on_fit_half_only() {
    let training = [scored(true, false), scored(true, false)];
    let frozen = fit_rules(&training, &points(), None);
    let heldout = [
        scored(false, true),
        scored(false, true),
        scored(false, true),
    ];
    assert_eq!(frozen.r0.w_s, 2);
    assert_eq!(frozen.winner, 0);
    assert_eq!(
        heldout
            .iter()
            .map(|r| score_point(r, frozen.point(frozen.winner, &r.params)))
            .sum::<usize>(),
        0
    );
    assert_eq!(fit_rules(&heldout, &points(), None).r0.w_s, 3);
    // Domain failures must remain misses, including negative and oversized affine windows.
    let a = Affine {
        a2: 1,
        b: -20,
        c2: 1,
        j_mode: 0,
        template: points()[0],
    };
    let p = DropsParams {
        max_time_s: 3,
        price_ma_s: 1,
        last_price_ma: 7,
        ..Default::default()
    };
    assert_eq!(a.point(&p).w_s, -18);
    assert!(!a.point(&p).valid());
    assert!(
        !Affine { a2: 4, b: 20, ..a }
            .point(&DropsParams {
                max_time_s: 300,
                ..p
            })
            .valid()
    );
    assert!(!Affine { b: 10, ..a }.point(&p).valid());
}

/// Refusals remain in parsed counts; an empty but covered tape remains a scored miss.
#[test]
fn unscorable_rows_counted_not_dropped() {
    let mut rows = [row(0), row(0), row(0)];
    rows[1].refusal = Some(2);
    rows[2].covered = Coverage::one((599_000, 602_000));
    let model = fit_rules(&[scored(true, false)], &points(), None);
    let c = evaluate(&rows, &[0, 1, 2], &model, 420, "test", false);
    assert_eq!((c[2].parsed, c[2].scorable, c[2].joint), (3, 1, 0));
    assert_eq!(refusal(&rows[1], 420), Some(2));
    assert_eq!(refusal(&rows[2], 420), Some(10));
}

/// Feasibility can flag a feed discrepancy without overriding actual agreement bits.
#[test]
fn infeasible_is_diagnostic_not_miss() {
    let mut c = Counts::default();
    c.add(11, true, true);
    c.add(0, true, false);
    assert_eq!(
        (c.scorable, c.joint, c.infeasible, c.prodwin_joint),
        (2, 1, 2, 1)
    );
}

/// Independently specified gate counts isolate each threshold at its exact boundary.
fn passing() -> Counts {
    Counts {
        parsed: 200,
        scorable: 200,
        joint: 160,
        prodwin: 20,
        prodwin_joint: 16,
        ..Default::default()
    }
}

/// An R2 winner cannot use lookup overfitting to bypass weak held-out R1 performance.
#[test]
fn r2_cannot_pass_gate_without_r1() {
    let c = passing();
    let weak = Counts {
        joint: 159,
        ..c.clone()
    };
    assert!(gate(2, &c, &c, &weak).contains("R1 support"));
    assert!(gate(2, &c, &c, &c).starts_with("PROVISIONAL-PASS"));
    assert!(gate(1, &c, &c, &weak).starts_with("PROVISIONAL-PASS"));
}

/// Cohort and reverse limits are fixed even when all other metrics would pass.
#[test]
fn gate_requires_min_scorable_and_reverse_split() {
    let c = passing();
    let small = Counts {
        scorable: 149,
        joint: 149,
        ..c.clone()
    };
    assert!(gate(0, &small, &c, &c).contains("insufficient"));
    let minimum = Counts {
        scorable: 150,
        joint: 120,
        ..c.clone()
    };
    let reverse = Counts {
        scorable: 200,
        joint: 150,
        ..c.clone()
    };
    assert!(gate(0, &minimum, &reverse, &c).starts_with("PROVISIONAL-PASS"));
    assert!(
        gate(
            0,
            &minimum,
            &Counts {
                joint: 149,
                ..reverse
            },
            &c
        )
        .contains("reverse")
    );
    assert!(gate(0, &Counts::default(), &c, &c).starts_with("FAIL"));
}

/// A covered winning-deal subset cannot be hidden behind other successful detects.
#[test]
fn prodwin_subset_must_also_pass() {
    let c = passing();
    assert!(gate(0, &c, &c, &c).starts_with("PROVISIONAL-PASS"));
    assert!(
        gate(
            0,
            &Counts {
                prodwin_joint: 15,
                ..c.clone()
            },
            &c,
            &c
        )
        .contains("prodwin")
    );
    assert!(
        gate(
            0,
            &Counts {
                prodwin: 0,
                prodwin_joint: 0,
                ..c.clone()
            },
            &c,
            &c
        )
        .contains("prodwin")
    );
}

/// The widest candidate grid fixes admission independently of either date half.
#[test]
fn envelope_is_grid_constant() {
    let points: Vec<_> = FitPoint::grid().collect();
    assert_eq!(choose_envelope(&points), 420);
    assert!(points.iter().all(|p| p.history_s() <= 420));
}

/// A fully passing calibration must never be mistaken for production-parity authorization.
#[test]
fn passing_verdict_is_provisional_without_parity() {
    let c = passing();
    for family in 0..3 {
        assert_eq!(
            gate(family, &c, &c, &c),
            "PROVISIONAL-PASS (production parity not run)"
        );
    }
}

/// Build a factual joined order with no dependency on persisted UI settings or real data.
fn started_deal() -> (Row, Deal) {
    let mut row = row(0);
    row.detect.core_uid = Some(42);
    row.detect.outcome = Outcome::TaskStarted;
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.kind = KIND_DROPS.to_string();
    deal.core_uid = 42;
    deal.strategy_id = 7;
    deal.coin = "USDT-XALPHA".to_string();
    deal.buy_set_ms = Some(600_000);
    deal.buy_ms = 601_000;
    deal.close_ms = 610_000;
    (row, deal)
}

/// Task creation alone cannot count when the UI's lead omits even the shortest rule warm-up.
#[test]
fn task_started_outside_prod_window_is_not_prodwin() {
    let (mut row, deal) = started_deal();
    row.covered = Coverage::one((0, 2_000_000));
    row.production_spans = production_spans(&row, &[deal], 60_000, 60_000);
    assert_eq!(row.production_spans.len(), 1);
    assert!(!in_production_window(&row, 420));
    assert_eq!(refusal(&row, 420), None);
}

/// A joined factual order qualifies only with the fixed cohort's entire UI-held history.
#[test]
fn task_started_inside_prod_window_is_prodwin() {
    let (mut row, mut deal) = started_deal();
    row.covered = Coverage::one((0, 2_000_000));
    row.production_spans = production_spans(&row, &[deal.clone()], 420_000, 60_000);
    assert!(in_production_window(&row, 420));
    deal.buy_set_ms = None;
    row.production_spans = production_spans(&row, &[deal], 421_000, 60_000);
    assert!(in_production_window(&row, 420));
    row.detect.outcome = Outcome::NoFollowUp;
    assert!(!in_production_window(&row, 420));
}

/// Missing, mistimed or differently identified deals cannot borrow another order's tape.
#[test]
fn task_started_without_deal_is_not_prodwin() {
    let (mut row, deal) = started_deal();
    let mut wrong_core = deal.clone();
    wrong_core.core_uid += 1;
    let mut wrong_strategy = deal.clone();
    wrong_strategy.strategy_id += 1;
    let mut wrong_coin = deal.clone();
    wrong_coin.coin = "XBETA".to_string();
    let mut too_late = deal.clone();
    too_late.buy_set_ms = Some(605_001);
    let mut too_early = deal.clone();
    too_early.buy_set_ms = Some(599_999);
    let mut wrong_kind = deal;
    wrong_kind.kind = "MoonShot".to_string();
    for deals in [
        vec![],
        vec![
            wrong_core,
            wrong_strategy,
            wrong_coin,
            too_late,
            too_early,
            wrong_kind,
        ],
    ] {
        row.production_spans = production_spans(&row, &deals, 120_000, 60_000);
        assert!(row.production_spans.is_empty());
        assert!(!in_production_window(&row, 420));
    }
}

/// A one-millisecond missing endpoint must refuse identically in the cohort and fit engine.
#[test]
fn cohort_and_fit_agree_at_span_edge() {
    let mut row = row(0);
    for (span, admitted) in [
        ((180_000, 602_000), true),
        ((180_001, 602_000), false),
        ((180_000, 601_999), false),
    ] {
        row.covered = Coverage::one(span);
        let fit = fit_detect_in_cohort(
            &row.detect,
            row.params.as_ref().unwrap(),
            &[],
            &row.covered,
            None,
            &points(),
            420,
        );
        assert_eq!(refusal(&row, 420).is_none(), admitted);
        assert_eq!(fit.refusal.is_none(), admitted);
    }
}
