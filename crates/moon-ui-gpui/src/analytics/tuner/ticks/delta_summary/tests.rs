use super::super::state::{DealRow, TapeStatus};
use super::delta_summary;
use moon_core::db::tuner::ticks::deltas::DeltaField;
use moon_core::db::tuner::ticks::{Deal, Deltas};

fn row(tape: TapeStatus) -> DealRow {
    DealRow {
        deal: Deal {
            report_uid: 1,
            core_uid: 1,
            core_name: String::new(),
            strategy_id: 1,
            kind: "MoonShot".into(),
            coin: "ACE".into(),
            buy_ms: 1_000,
            close_ms: 2_000,
            buy_price: 1.0,
            sell_price: 1.0,
            spent: 1.0,
            is_short: false,
            sell_reason: String::new(),
            fact_pnl: 0.0,
            sizing: None,
            pnl_pct: false,
            profit: None,
            deltas: Deltas::default(),
            delta_track: None,
            quotes: None,
            bars: None,
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
            own_entry: None,
            buy_set_ms: None,
            corridor: None,
            entry_placed: None,
            gap: None,
        },
        tape,
        verdict: None,
        address: None,
        ticks: None,
        entry_line: None,
        held: None,
    }
}

#[test]
fn no_row_with_its_tape_says_nothing() {
    assert!(delta_summary(&[row(TapeStatus::Missing)]).is_none());
}

#[test]
fn the_tooltip_names_every_delta_computed_or_not() {
    let _locale = crate::test_locale::force("en");
    let (caption, tip) = delta_summary(&[row(TapeStatus::Covered)]).unwrap();
    assert!(caption.contains("0 of 1"), "{caption}");
    for field in DeltaField::ALL {
        assert!(
            tip.contains(field.column()),
            "{} missing from {tip}",
            field.column()
        );
    }
    for column in ["dmark", "pricebug", "exchange1hdelta", "exchange24hdelta"] {
        assert!(tip.contains(column), "{column} missing from {tip}");
    }
}

/// Reusing a summary across row publication or language changes would show stale coverage or copy.
#[test]
fn cached_summary_follows_rows_and_locale() {
    use super::super::state::{TicksData, TicksState};
    let mut state = TicksState::default();
    state.publish(
        Ok(TicksData {
            rows: vec![row(TapeStatus::Covered)].into(),
            ..Default::default()
        }),
        false,
    );
    let english = {
        let _locale = crate::test_locale::force("en");
        let expected = delta_summary(&state.data.data().unwrap().rows);
        assert_eq!(super::cached(&mut state), expected);
        assert_eq!(super::cached(&mut state), expected);
        expected
    };
    {
        let _locale = crate::test_locale::force("ru");
        let expected = delta_summary(&state.data.data().unwrap().rows);
        assert_ne!(expected, english);
        assert_eq!(super::cached(&mut state), expected);
    }
    let _locale = crate::test_locale::force("en");
    state.publish(
        Ok(TicksData {
            rows: vec![row(TapeStatus::Missing)].into(),
            ..Default::default()
        }),
        false,
    );
    assert_eq!(super::cached(&mut state), None);
    state.publish(Err(moon_core::db::ReadFail::NotReady), false);
    assert_eq!(super::cached(&mut state), None);
}
