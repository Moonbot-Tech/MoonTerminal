//! Rules of the Summary view-model: comparison tones, KPI visibility, insight omission,
//! concentration, ranking fold, chart line choice and the chart's vertical range.
//!
//! Explicit imports, never `use super::*` through a `gpui::*` glob (CONTRIBUTING.md).

use std::collections::BTreeSet;
use std::rc::Rc;

use super::{
    Concentration, CoreRowItem, DeltaFmt, DeltaRule, DeltaTone, DivergingScale, InsightTag,
    InsightTone, KpiBand, chart_range, core_rows, delta_line, insights, shown_cores,
};
use crate::analytics::summary::state::{CoreFilter, CoreLines};
use crate::analytics::{pnl_unit_label, set_pnl_unit};
use moon_core::db::analytics::{CoreSeries, DayPoint, GroupStat, PeriodStats, Summary};
use moon_core::db::{ProfitUnit, QuoteCurrency};
use moon_ui::MoonPalette;

/// Synthetic core with a period total.
fn core(uid: u64, total: f64) -> CoreSeries {
    CoreSeries {
        uid,
        name: format!("XALPHA-{uid:02}"),
        total,
        ..Default::default()
    }
}

/// Money mode in USDT, so value strings carry no `%`.
fn usdt() {
    set_pnl_unit(Some(ProfitUnit::Quote(
        QuoteCurrency::from_report_ordinal(1).expect("USDT report ordinal"),
    )));
}

/// Period with a previous window, enough for every KPI comparison.
fn period(prev: bool) -> Summary {
    Summary {
        cur: PeriodStats {
            n: 40,
            wins: 20,
            losses: 20,
            profit: 120.0,
            pf: 1.29,
            avg: 3.0,
            max_dd: 60.0,
            avg_dur_min: 14.0,
            ..Default::default()
        },
        prev: prev.then(|| PeriodStats {
            n: 20,
            wins: 12,
            losses: 8,
            profit: 80.0,
            pf: 1.18,
            avg: 4.0,
            max_dd: 50.0,
            avg_dur_min: 10.0,
            ..Default::default()
        }),
        bucket_secs: 86_400,
        ..Default::default()
    }
}

/// Every row of the plan's delta table, in both directions, and the rounds-to-zero rule.
///
/// Swapping the `Down` arm makes a deeper drawdown green; dropping the rounding makes a
/// microscopic change carry an arrow its digits do not show.
#[test]
fn every_kpi_delta_follows_its_better_direction() {
    let _locale = crate::test_locale::force("en");
    usdt();
    let p = MoonPalette::LIGHT;
    let cases = [
        (
            120.0,
            80.0,
            DeltaRule::Up,
            DeltaFmt::Money,
            DeltaTone::Better,
        ),
        (
            50.0,
            60.8,
            DeltaRule::Up,
            DeltaFmt::Points,
            DeltaTone::Worse,
        ),
        (
            1.29,
            1.18,
            DeltaRule::Up,
            DeltaFmt::Ratio,
            DeltaTone::Better,
        ),
        (
            60.0,
            50.0,
            DeltaRule::Down,
            DeltaFmt::Drawdown,
            DeltaTone::Worse,
        ),
        (
            40.0,
            50.0,
            DeltaRule::Down,
            DeltaFmt::Drawdown,
            DeltaTone::Better,
        ),
        (10.0, 20.0, DeltaRule::Up, DeltaFmt::Count, DeltaTone::Worse),
        (3.0, 4.0, DeltaRule::Up, DeltaFmt::Pct, DeltaTone::Worse),
    ];
    for (cur, prev, rule, kind, tone) in cases {
        let line = delta_line(cur, Some(prev), rule, kind).expect("comparable");
        assert_eq!(line.tone, tone, "{kind:?} {cur} vs {prev}: {}", line.text);
    }
    let dd = delta_line(60.0, Some(50.0), DeltaRule::Down, DeltaFmt::Drawdown).unwrap();
    assert_eq!(dd.text, "deeper by 20.0%");
    assert_eq!(dd.tone.color(p), p.orange);
    let wr = delta_line(50.0, Some(60.8), DeltaRule::Up, DeltaFmt::Points).unwrap();
    assert_eq!(wr.text, "▼ 10.8 pp");
    assert_eq!(wr.prev_text.as_deref(), Some("was 60.8%"));
    let money = delta_line(120.0, Some(80.0), DeltaRule::Up, DeltaFmt::Money).unwrap();
    assert_eq!(money.text, "+40 (+50.0%)");
    assert_eq!(money.prev_text.as_deref(), Some("was +80"));

    let flat = delta_line(1.2900001, Some(1.29), DeltaRule::Up, DeltaFmt::Ratio).unwrap();
    assert_eq!(
        (flat.text.as_str(), flat.tone),
        ("0.00", DeltaTone::Neutral)
    );
    assert_eq!(flat.tone.color(p), p.text_soft);
    assert!(delta_line(1.0, None, DeltaRule::Up, DeltaFmt::Ratio).is_none());
}

/// A band without a previous period, or with comparison switched off, must show no `was` line
/// anywhere; one leaked line claims a comparison the user turned off.
#[test]
fn comparison_lines_vanish_without_prev_or_when_switched_off() {
    let _locale = crate::test_locale::force("en");
    usdt();
    let spark: Rc<[f32]> = Rc::from([0.0f32, 1.0]);
    let on = KpiBand::new(&period(true), spark.clone(), true);
    assert!(on.result.was.is_some() && on.quality.wr_prev.is_some());
    assert!(on.risk.dd_of_profit.is_some());
    for band in [
        KpiBand::new(&period(false), spark.clone(), true),
        KpiBand::new(&period(true), spark, false),
    ] {
        assert!(band.result.was.is_none());
        assert!(band.quality.wr_delta.is_none() && band.quality.wr_prev.is_none());
        assert!(band.quality.pf_delta.is_none());
        assert!(band.risk.dd_line.is_none());
        // A current-period fact, not a comparison (F1).
        assert!(band.risk.dd_of_profit.is_some());
        assert!(band.activity.trades_delta.is_none() && band.activity.avg_delta.is_none());
        assert_eq!(band.risk.recovery, Some(2.0));
    }
}

/// Summary with every insight fact present on a daily grid.
fn rich() -> Summary {
    let mut d = period(true);
    d.strategies = vec![GroupStat {
        key: "17@1".into(),
        name: "XSTRAT".into(),
        n: 10,
        wins: 9,
        profit: 30.0,
        ..Default::default()
    }];
    d.coins = vec![
        GroupStat {
            name: "XCOIN".into(),
            n: 5,
            profit: 50.0,
            ..Default::default()
        },
        GroupStat {
            name: "XLOSS".into(),
            n: 4,
            wins: 1,
            profit: -20.0,
            worst: -9.0,
            ..Default::default()
        },
    ];
    d.days = [10.0, -30.0, 140.0]
        .iter()
        .enumerate()
        .map(|(i, &profit)| DayPoint {
            start: 1_800_000_000 + i as i64 * 86_400,
            profit,
            trades: 3,
        })
        .collect();
    d.best_hour = Some((1, 36.0, 12));
    d
}

/// Order is fixed and a missing fact is omitted rather than left as an empty slot.
#[test]
fn insights_keep_their_order_and_omit_missing_facts() {
    let _locale = crate::test_locale::force("en");
    usdt();
    let tags = |d: &Summary| {
        insights(d, chrono_tz::UTC)
            .into_iter()
            .map(|i| i.tag)
            .collect::<Vec<_>>()
    };
    let full = rich();
    assert_eq!(
        tags(&full),
        [
            InsightTag::Leader,
            InsightTag::Minus,
            InsightTag::Day,
            InsightTag::Time,
            InsightTag::Quality
        ]
    );
    let all = insights(&full, chrono_tz::UTC);
    assert_eq!(
        all[2].why.as_deref(),
        Some("50% of the period drawdown fell on this day")
    );
    assert_eq!(
        all[3].why.as_deref(),
        Some("12 trades · 30% of profit in 4% of the time")
    );
    assert_eq!(all[4].value_tone, InsightTone::Edge);

    let mut sparse = rich();
    sparse.strategies.clear();
    sparse.coins.truncate(1);
    sparse.best_hour = None;
    assert_eq!(tags(&sparse), [InsightTag::Day, InsightTag::Quality]);
    assert!(tags(&Summary::default()).is_empty());
}

/// Hourly and weekly grids have no "day" insight and no day counts in the quality line (J8).
#[test]
fn hourly_and_weekly_grids_skip_day_facts() {
    let _locale = crate::test_locale::force("en");
    usdt();
    for bucket in [3_600, 7 * 86_400] {
        let mut d = rich();
        d.bucket_secs = bucket;
        let all = insights(&d, chrono_tz::UTC);
        assert!(all.iter().all(|i| i.tag != InsightTag::Day), "{bucket}");
        let quality = all.iter().find(|i| i.tag == InsightTag::Quality).unwrap();
        assert_eq!(quality.why, None, "{bucket}");
    }
    let daily = insights(&rich(), chrono_tz::UTC);
    let quality = daily.iter().find(|i| i.tag == InsightTag::Quality).unwrap();
    assert_eq!(
        quality.why.as_deref(),
        Some("2 profitable days versus 1 losing day")
    );
}

/// The insight value's unit tracks the metric switch: `%` in percent mode, none in money mode.
#[test]
fn insight_sentence_unit_follows_the_metric() {
    let _locale = crate::test_locale::force("en");
    set_pnl_unit(Some(ProfitUnit::Percent));
    assert_eq!(pnl_unit_label(), "%");
    let pct = insights(&rich(), chrono_tz::UTC);
    assert!(pct[0].value.ends_with('%'), "{}", pct[0].value);
    usdt();
    let money = insights(&rich(), chrono_tz::UTC);
    assert!(!money[0].value.contains('%'), "{}", money[0].value);
}

/// Leader, top-5 and half-of-profit figures come out exact for a hand-checked set.
#[test]
fn concentration_matches_hand_computed_shares() {
    let cores = [
        core(1, 50.0),
        core(2, 20.0),
        core(3, 10.0),
        core(4, 10.0),
        core(5, 5.0),
        core(6, 5.0),
        core(7, -25.0),
    ];
    let c = Concentration::of(&cores).expect("gains");
    assert_eq!(c.leader_pct, 50.0);
    assert_eq!(c.top5_pct, 95.0);
    assert_eq!(c.half_k, 1);
    assert_eq!(c.n, 7);
    assert_eq!(c.eaten_pct, 25.0);
    assert!(Concentration::of(&[core(1, 0.0), core(2, -5.0)]).is_none());
}

/// A folded list shows every core at most once, and hidden plus shown covers the whole list.
#[test]
fn ranking_folds_without_repeating_and_shares_one_scale() {
    let cores: Vec<_> = (0..20).map(|i| core(i, 100.0 - i as f64 * 9.0)).collect();
    let rows = core_rows(&cores, CoreFilter::All, false);
    let mut seen = BTreeSet::new();
    let mut hidden = 0;
    for row in &rows {
        match row {
            CoreRowItem::Core(i) => assert!(seen.insert(*i), "core {i} repeats"),
            CoreRowItem::Fold { hidden: h, between } => {
                hidden += h;
                assert_eq!(*between, (cores[8].total, cores[14].total));
            }
        }
    }
    assert_eq!(seen.len(), 13);
    assert_eq!(seen.len() + hidden, cores.len());
    assert_eq!(core_rows(&cores, CoreFilter::All, true).len(), 20);
    assert_eq!(core_rows(&cores[..13], CoreFilter::All, false).len(), 13);

    let neg = core_rows(&cores, CoreFilter::Neg, false);
    assert_eq!(neg.first(), Some(&CoreRowItem::Core(19)));
    let pos = core_rows(&cores, CoreFilter::Pos, false);
    assert!(
        pos.iter()
            .all(|r| matches!(r, CoreRowItem::Core(i) if cores[*i].total > 0.0))
    );

    let scale = DivergingScale::of(&cores);
    assert_eq!((scale.max_pos, scale.max_neg), (100.0, 71.0));
    assert_eq!(scale.zero_frac, (71.0 / 171.0) as f32);
    let (start, width) = scale.bar(-71.0);
    assert!(start.abs() < 1e-6 && (width - scale.zero_frac).abs() < 1e-6);
}

/// Each line mode picks the documented cores.
#[test]
fn shown_cores_follow_each_line_mode() {
    let cores: Vec<_> = [5.0, 40.0, -30.0, 20.0, 10.0, 30.0, 1.0]
        .iter()
        .enumerate()
        .map(|(i, &t)| core(i as u64 + 1, t))
        .collect();
    let picked: BTreeSet<u64> = [7, 3].into();
    assert!(shown_cores(&cores, CoreLines::None, &picked).is_empty());
    assert_eq!(
        shown_cores(&cores, CoreLines::Top5, &picked),
        [1, 5, 3, 4, 0]
    );
    assert_eq!(shown_cores(&cores, CoreLines::Selected, &picked), [2, 6]);
}

/// A picked core outside the old top-12 still widens the range; a stale subset would clip it (J1).
#[test]
fn a_picked_core_outside_the_old_top_12_widens_the_range() {
    let total = [0.0f32, 5.0, 10.0];
    let curves: Vec<Rc<[f32]>> = (0..20)
        .map(|i| {
            Rc::from(if i == 17 {
                [0.0, -40.0, 90.0]
            } else {
                [0.0, 1.0, 2.0]
            })
        })
        .collect();
    assert_eq!(chart_range(&total, &curves, &[]), (0.0, 10.0));
    assert_eq!(chart_range(&total, &curves, &[17]), (-40.0, 90.0));
}

/// A zero baseline still reports the change: a drawdown that appeared from nothing is "deeper"
/// by its size and worse, a count from zero reads "was 0" in its rule's tone (F2).
#[test]
fn zero_baselines_still_emit_a_comparison() {
    let _locale = crate::test_locale::force("en");
    usdt();
    let dd = delta_line(60.0, Some(0.0), DeltaRule::Down, DeltaFmt::Drawdown).expect("line");
    assert_eq!(
        (dd.text.as_str(), dd.tone),
        ("deeper by 60", DeltaTone::Worse)
    );
    assert!(delta_line(0.0, Some(0.0), DeltaRule::Down, DeltaFmt::Drawdown).is_none());
    let count = delta_line(12.0, Some(0.0), DeltaRule::Up, DeltaFmt::Count).expect("line");
    assert_eq!(
        (count.text.as_str(), count.tone),
        ("was 0", DeltaTone::Better)
    );
    let avg = delta_line(-3.0, Some(0.0), DeltaRule::Up, DeltaFmt::Pct).expect("line");
    assert_eq!((avg.text.as_str(), avg.tone), ("was 0", DeltaTone::Worse));
}

/// The pick hint follows what is drawn, not the pick set: picks naming cores the snapshot lacks
/// draw nothing, so the hint must show; keyed on an empty pick set it would leave a bare chart.
#[test]
fn selection_hint_follows_the_drawn_lines() {
    use super::{selection_hint_due, shown_cores};
    let cores = vec![CoreSeries {
        uid: 1,
        name: "XCORE-01".into(),
        ..Default::default()
    }];
    let stale: BTreeSet<u64> = [99].into_iter().collect();
    let shown = shown_cores(&cores, CoreLines::Selected, &stale);
    assert!(selection_hint_due(CoreLines::Selected, &shown));
    let live: BTreeSet<u64> = [1].into_iter().collect();
    let shown = shown_cores(&cores, CoreLines::Selected, &live);
    assert!(!selection_hint_due(CoreLines::Selected, &shown));
    assert!(!selection_hint_due(CoreLines::Top5, &[]));
}
