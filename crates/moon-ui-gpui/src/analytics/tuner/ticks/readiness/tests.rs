//! Synthetic regressions: the card's verdict and the strategy list's "Tuning" column read one
//! count. No user data, no live view.
// Not `super::*`: the parent chain brings gpui's own `test` attribute.

use super::super::state::{DealRow, TapeStatus, TicksData, TicksState};
use super::super::summary::{SummaryModel, Verdict};
use super::super::tape::PackedTape;
use super::Readiness;
use crate::analytics::tuner::ready::ReadyClass;
use moon_core::db::tuner::ticks::{Deal, Deltas};

fn deal(uid: i64) -> Deal {
    Deal {
        report_uid: uid,
        core_uid: 1,
        core_name: String::new(),
        strategy_id: 1,
        kind: "Spread".into(),
        coin: "XALPHA".into(),
        buy_ms: 1_000,
        close_ms: 2_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
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
        strike_detect: None,
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

fn verdict(entry: Option<bool>, exit: Option<bool>) -> moon_core::db::tuner::ticks::Verdict {
    moon_core::db::tuner::ticks::Verdict {
        entry,
        entry_dev_pct: None,
        exit,
        exit_dev_pct: None,
        fill: None,
        exit_kind: None,
        line_points: None,
        entry_finding: match entry {
            None => moon_core::db::tuner::ticks::verify::EntryFinding::Fact,
            Some(true) => moon_core::db::tuner::ticks::verify::EntryFinding::Hit,
            Some(false) => moon_core::db::tuner::ticks::verify::EntryFinding::Off,
        },
        exit_finding: match exit {
            None => moon_core::db::tuner::ticks::verify::ExitFinding::Unjudged(
                moon_core::db::tuner::ticks::verify::Unjudged::OtherRule,
            ),
            Some(true) => moon_core::db::tuner::ticks::verify::ExitFinding::Hit,
            Some(false) => moon_core::db::tuner::ticks::verify::ExitFinding::Miss(
                moon_core::db::tuner::ticks::verify::ExitMiss::NoLevel,
            ),
        },
        fill_clock_ms: None,
        rules: moon_core::db::tuner::ticks::verify::RuleFlags::default(),
        stop: moon_core::db::tuner::ticks::verify::StopFacts::default(),
    }
}

fn tape() -> PackedTape {
    PackedTape::pack(
        (0..4)
            .map(|i| moon_core::feed::types::Tick {
                time_ms: 1_000.0 + f64::from(i),
                price: 1.0,
                qty: 1.0,
                side: moon_core::feed::types::Side::Buy,
            })
            .collect(),
    )
}

/// A row the search learns on: covered, reproduced, a long tail, its tape in memory.
fn fit_row(uid: i64) -> DealRow {
    DealRow {
        deal: deal(uid),
        tape: TapeStatus::Covered,
        verdict: Some(verdict(Some(true), Some(true))),
        address: None,
        ticks: Some(tape()),
        entry_line: None,
        held: Some((0, 3_600_000)),
    }
}

/// A covered row the model does not reproduce: covered, never fit.
fn missed_row(uid: i64) -> DealRow {
    DealRow {
        verdict: Some(verdict(Some(false), Some(false))),
        ..fit_row(uid)
    }
}

/// A row without tape and without an address: nothing can fetch it.
fn bare_row(uid: i64) -> DealRow {
    DealRow {
        tape: TapeStatus::NoAddress,
        verdict: None,
        ticks: None,
        held: None,
        ..fit_row(uid)
    }
}

/// The card's verdict over `rows`, as the summary builds it from the table.
fn card_cannot(rows: &[DealRow]) -> bool {
    let mut state = TicksState::default();
    state.data.apply(Ok(TicksData {
        rows: rows.to_vec(),
        ..Default::default()
    }));
    matches!(
        SummaryModel::from_state(&state, String::new(), false).verdict,
        Verdict::Cannot(_)
    )
}

/// One strategy's readiness as the column computes it — the same function the card's table goes
/// through ([`TicksData::readiness`]).
fn column(rows: &[DealRow]) -> Readiness {
    Readiness::of_rows(rows, 0, super::super::tail::effective_s())
}

/// Can tune, too few, no tape: the column's class and the card's verdict agree on each, and the
/// card's table counts exactly what the column counts.
#[test]
fn the_column_and_the_card_agree_on_can_few_and_no_tape() {
    let _locale = crate::test_locale::force("en");
    let min = Readiness::min();
    let can: Vec<DealRow> = (0..min as i64 + 4).map(fit_row).collect();
    let mut few: Vec<DealRow> = (0..10).map(fit_row).collect();
    few.extend((10..14).map(missed_row));
    let none: Vec<DealRow> = (0..30).map(bare_row).collect();
    for (rows, want) in [
        (&can, ReadyClass::Can { fit: min + 4 }),
        (&few, ReadyClass::Few { fit: 10 }),
        (&none, ReadyClass::NoTape),
    ] {
        let ready = column(rows);
        let data = TicksData {
            rows: rows.clone(),
            ..Default::default()
        };
        assert_eq!(data.readiness().fit, ready.fit, "one count for both");
        assert_eq!(data.readiness().covered, ready.covered);
        assert_eq!(ReadyClass::of(&ready), want);
        assert_eq!(
            card_cannot(rows),
            !matches!(want, ReadyClass::Can { .. }),
            "the card refuses exactly what the column does not call tunable: {want:?}"
        );
    }
}

/// The sample is the fit rows WITH their tape in memory; a fit row whose tape the cap let go
/// does not count, the same as on the card.
#[test]
fn a_fit_row_without_its_tape_is_not_in_the_sample() {
    let rows = [
        fit_row(1),
        DealRow {
            ticks: None,
            ..fit_row(2)
        },
    ];
    let ready = column(&rows);
    assert_eq!(ready.fit, 1);
    assert_eq!(ready.covered, 2);
    assert_eq!(ready.too_few(), Some(1));
}

/// Every deal is subtracted exactly once; dropped retained tape is counted as unjudged.
#[test]
fn the_breakdown_partitions_overlapping_exclusions() {
    let mut short = fit_row(3);
    short.held = Some((0, 0));
    let mut hole = fit_row(4);
    hole.deal.gap = Some(moon_core::db::tuner::ticks::gap::TapeGap {
        from_ms: 1_100,
        to_ms: 1_800,
        fact_line: None,
        fact_stop: None,
    });
    short.deal.gap = hole.deal.gap.clone();
    let mut unjudged = fit_row(6);
    unjudged.verdict = Some(verdict(Some(true), None));
    unjudged.deal.gap = hole.deal.gap.clone();
    let lost = DealRow {
        ticks: None,
        ..fit_row(7)
    };
    let rows = [
        fit_row(1),
        bare_row(2),
        short,
        hole,
        missed_row(5),
        unjudged,
        lost,
    ];
    let ready = Readiness::of_rows(&rows, 0, 60);
    assert_eq!(
        (
            ready.deals,
            ready.no_tape,
            ready.short_tail,
            ready.holed,
            ready.missed,
            ready.unjudged
        ),
        (7, 1, 1, 1, 1, 2)
    );
    assert_eq!(
        ready.deals
            - ready.no_tape
            - ready.short_tail
            - ready.holed
            - ready.missed
            - ready.unjudged,
        ready.fit
    );
    assert_eq!(
        ready.fit,
        rows.iter()
            .filter(|row| row.fit() && row.ticks.is_some())
            .count()
    );
}
