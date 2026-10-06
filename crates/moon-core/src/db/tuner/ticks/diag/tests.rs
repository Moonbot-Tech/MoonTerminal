use std::collections::HashMap;

use super::super::settings::ModelSettings;
use super::super::tests::deal;
use super::super::verify::{EntryFinding, ExitFinding, ExitMiss, MissParts, RuleFlags, Verdict};
use super::*;

/// A verdict with the given findings, its `Option<bool>`s read off them as `verify` does.
fn verdict(entry: EntryFinding, exit: ExitFinding) -> Verdict {
    Verdict {
        entry: entry.verdict(),
        entry_dev_pct: None,
        exit: exit.verdict(),
        exit_dev_pct: Some(-0.4),
        fill: None,
        exit_kind: None,
        line_points: None,
        entry_finding: entry,
        exit_finding: exit,
        fill_clock_ms: Some(120),
        rules: RuleFlags {
            stop: true,
            fast_stop: true,
            ..RuleFlags::default()
        },
    }
}

/// A trade whose every identifying field holds a value the report must never print.
fn private_deal(core_uid: u64, sell_reason: &str) -> Deal {
    Deal {
        report_uid: 987_654_321,
        core_uid,
        core_name: "MyPrivateCore".into(),
        strategy_id: 424_242,
        coin: "SECRETCOIN".into(),
        buy_price: 123.456_789,
        sell_price: 124.987_654,
        spent: 5_555.0,
        fact_pnl: 77.77,
        profit: Some(66.66),
        sell_reason: sell_reason.into(),
        ..deal()
    }
}

fn input<'a>(rows: Vec<ReportRow<'a>>, model: ModelSettings) -> ReportInput<'a> {
    ReportInput {
        build: "v0.0.0 (test)".into(),
        period_days: Some(7.0),
        without_ms: 3,
        service: 2,
        untunable: 1,
        rows,
        cores: HashMap::new(),
        model,
    }
}

#[test]
fn the_report_prints_nothing_that_identifies_a_trade() {
    let deals = [
        private_deal(11, "Sell Price"),
        private_deal(11, "StopLoss fixed: 120.1"),
        private_deal(12, "Auto Price Down"),
    ];
    let hit = verdict(EntryFinding::Hit, ExitFinding::Hit);
    let late = verdict(
        EntryFinding::Off,
        ExitFinding::Miss(ExitMiss::Off(MissParts {
            level: true,
            late_ms: Some(1_800),
            first_unmatched: Some(0),
        })),
    );
    let rows = vec![
        ReportRow {
            deal: &deals[0],
            venue: Some("Binance-Futures".into()),
            tape: TapeClass::Covered,
            verdict: Some(&hit),
            outside_model: &[],
        },
        ReportRow {
            deal: &deals[1],
            venue: Some("Binance-Futures".into()),
            tape: TapeClass::Covered,
            verdict: Some(&late),
            outside_model: &[],
        },
        ReportRow {
            deal: &deals[2],
            venue: Some("Bybit-Futures".into()),
            tape: TapeClass::Refused("NoRoute"),
            verdict: None,
            outside_model: &[],
        },
    ];
    let text = render(&input(rows, ModelSettings::default()));
    for secret in [
        "SECRETCOIN",
        "MyPrivateCore",
        "424242",
        "987654321",
        "123.45",
        "124.98",
        "5555",
        "77.77",
        "66.66",
        "120.1",
    ] {
        assert!(!text.contains(secret), "{secret} leaked into:\n{text}");
    }
    // What it is for is there: the miss's parts, the core's clock, the refused venue.
    assert!(text.contains("moment 1"), "{text}");
    assert!(text.contains("line 1 (at the take 1"), "{text}");
    assert!(text.contains("NoRoute 1 [Bybit-Futures]"), "{text}");
    assert!(text.contains("med +120"), "{text}");
}

#[test]
fn the_funnel_and_the_segments_count_every_row_once() {
    let deals = [
        private_deal(11, "Sell Price"),
        private_deal(11, "StopLoss Market Sell"),
        private_deal(11, "StopLoss Market Sell"),
        private_deal(12, "Auto Price Down"),
    ];
    let hit = verdict(EntryFinding::Fact, ExitFinding::Hit);
    let missed = verdict(
        EntryFinding::Fact,
        ExitFinding::Miss(ExitMiss::StopNotFired),
    );
    let row = |deal, venue: &str, tape, verdict| ReportRow {
        deal,
        venue: Some(venue.into()),
        tape,
        verdict,
        outside_model: &[],
    };
    let rows = vec![
        row(&deals[0], "Binance-Futures", TapeClass::Covered, Some(&hit)),
        row(
            &deals[1],
            "Binance-Futures",
            TapeClass::Covered,
            Some(&missed),
        ),
        row(
            &deals[2],
            "Binance-Futures",
            TapeClass::Covered,
            Some(&missed),
        ),
        row(&deals[3], "Bybit-Futures", TapeClass::Missing, None),
    ];
    let input = input(rows, ModelSettings::default());
    let report = aggregate(&input);
    assert_eq!(report.funnel.rows, 4);
    assert_eq!(report.funnel.tape.get(&TapeClass::Covered), Some(&3));
    assert_eq!(report.funnel.exit, Share { hits: 1, n: 3 });
    // No kind with an entry model: the entry share counts nothing.
    assert_eq!(report.funnel.entry, Share { hits: 0, n: 0 });
    // The core with more rows is C1.
    assert_eq!(report.cores[0].uid, 11);
    assert_eq!(report.cores[0].covered, 3);
    assert_eq!(report.cores[1].covered, 0);
    // The stop segment, with both misses, leads.
    assert_eq!(report.segments.len(), 2);
    assert_eq!(report.segments[0].key.close, CloseClass::Stop);
    assert_eq!(report.segments[0].exit.stop_not_fired, 2);
    assert_eq!(
        report.segments[0].rules[1], 2,
        "fast stop counted per trade"
    );
    assert_eq!(report.segments[1].key.close, CloseClass::Take);
}

#[test]
fn a_changed_model_setting_is_named_and_the_default_is_said() {
    let quiet = render(&input(Vec::new(), ModelSettings::default()));
    assert!(quiet.contains("model settings: default"), "{quiet}");
    let tuned = ModelSettings {
        latency_ms: 321.0,
        ..ModelSettings::default()
    };
    let loud = render(&input(Vec::new(), tuned));
    assert!(loud.contains("latency_ms 321"), "{loud}");
}

#[test]
fn the_close_class_follows_the_verdicts_reading_of_the_reason() {
    assert_eq!(CloseClass::of("Sell Price"), CloseClass::Take);
    assert_eq!(CloseClass::of("Auto Price Down 3"), CloseClass::Line);
    assert_eq!(CloseClass::of("StopLoss Market Sell"), CloseClass::Stop);
    assert_eq!(
        CloseClass::of("TrailingStop PeakPrice = 1.2;"),
        CloseClass::Stop
    );
    assert_eq!(CloseClass::of("Global PanicSell"), CloseClass::Other);
}

/// The report over stop trades on two venues, one moment miss per entry of `misses`: the
/// venue's name and the miss's model − core ms.
fn moment_report(misses: &[(&str, i64)]) -> String {
    let deals: Vec<Deal> = misses
        .iter()
        .map(|_| private_deal(11, "StopLoss AutoActivated"))
        .collect();
    let verdicts: Vec<Verdict> = misses
        .iter()
        .map(|&(_, late_ms)| {
            verdict(
                EntryFinding::Fact,
                ExitFinding::Miss(ExitMiss::Off(MissParts {
                    level: false,
                    late_ms: Some(late_ms),
                    first_unmatched: None,
                })),
            )
        })
        .collect();
    let rows = misses
        .iter()
        .zip(deals.iter().zip(&verdicts))
        .map(|(&(venue, _), (deal, verdict))| ReportRow {
            deal,
            venue: Some(venue.into()),
            tape: TapeClass::Covered,
            verdict: Some(verdict),
            outside_model: &[],
        })
        .collect();
    render(&input(rows, ModelSettings::default()))
}

#[test]
fn moment_misses_are_split_by_side_each_with_its_own_median() {
    let text = moment_report(&[
        ("Gate-Futures", -6_000),
        ("Gate-Futures", -4_000),
        ("Gate-Futures", 3_000),
        ("Gate-Futures", 5_000),
        ("Gate-Futures", 9_000),
        ("Bybit-Futures", -2_500),
        ("Bybit-Futures", -3_500),
    ]);
    // One median over the five would read +3000 and hide the early two; each side keeps its own
    // median and its own far tail — the most negative miss early, the largest late.
    assert!(
        text.contains(
            "moment 5 (early 2: med -4000 ms · tail -6000 ms; late 3: med +5000 ms · tail +9000 ms)"
        ),
        "{text}"
    );
    // A side with no miss is left out, not printed empty.
    assert!(
        text.contains("moment 2 (early 2: med -2500 ms · tail -3500 ms)"),
        "{text}"
    );
}
