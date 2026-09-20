use super::super::columns::{COL_MODEL, COL_RESULT, COL_TAPE, COL_TIME};
use super::super::state::{DealRow, TapeStatus, TicksData, TicksState};
use super::{order_for, result_pct};
use moon_core::db::tuner::ticks::{Deal, Deltas, Verdict};
use moon_core::market::trade_replay::TickStatus;

fn deal(uid: i64, buy_ms: i64, buy: f64, sell: f64, short: bool) -> Deal {
    Deal {
        report_uid: uid,
        core_uid: 1,
        strategy_id: 1,
        kind: "MoonShot".into(),
        coin: "ACE".into(),
        buy_ms,
        close_ms: buy_ms + 1_000,
        buy_price: buy,
        sell_price: sell,
        spent: 100.0,
        is_short: short,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        deltas: Deltas::default(),
        tick: None,
    }
}

fn verdict(entry: Option<bool>, exit: Option<bool>) -> Verdict {
    Verdict {
        entry,
        entry_dev_pct: None,
        exit,
        exit_dev_pct: None,
        fill: None,
        exit_kind: None,
        line_points: None,
    }
}

fn state() -> TicksState {
    let rows = vec![
        DealRow {
            deal: deal(1, 3_000, 100.0, 101.0, false),
            tape: TapeStatus::Missing,
            verdict: None,
            address: None,
            ticks: None,
            entry_start: None,
        },
        DealRow {
            deal: deal(2, 1_000, 100.0, 99.0, false),
            tape: TapeStatus::Covered,
            verdict: Some(verdict(Some(true), Some(true))),
            address: None,
            ticks: None,
            entry_start: None,
        },
        DealRow {
            deal: deal(3, 2_000, 100.0, 99.0, true),
            tape: TapeStatus::Refused(TickStatus::NoRoute),
            verdict: Some(verdict(Some(false), None)),
            address: None,
            ticks: None,
            entry_start: None,
        },
    ];
    let mut state = TicksState::default();
    state.data.apply(Ok(TicksData {
        rows,
        ..TicksData::default()
    }));
    state.rows_rev = 1;
    state
}

fn uids(state: &mut TicksState) -> Vec<i64> {
    let rows: Vec<i64> = state
        .data
        .data()
        .map(|d| d.rows.iter().map(|r| r.deal.report_uid).collect())
        .unwrap_or_default();
    order_for(state).iter().map(|&i| rows[i]).collect()
}

#[test]
fn the_default_order_is_newest_entry_first() {
    let mut state = state();
    assert_eq!(state.sort, Some((COL_TIME.to_string(), true)));
    assert_eq!(uids(&mut state), [1, 3, 2]);
}

#[test]
fn a_short_result_is_signed_from_its_own_side() {
    let state = state();
    let rows = &state.data.data().unwrap().rows;
    assert!((result_pct(&rows[0]) - 1.0).abs() < 1e-9);
    assert!((result_pct(&rows[1]) + 1.0).abs() < 1e-9);
    assert!(
        (result_pct(&rows[2]) - 1.0).abs() < 1e-9,
        "a short sold lower won"
    );
    let mut state = state;
    state.sort = Some((COL_RESULT.to_string(), true));
    let top = uids(&mut state)[0];
    assert!(top == 1 || top == 3);
}

#[test]
fn tape_and_model_sort_by_rank_and_the_cache_follows_the_sort() {
    let mut state = state();
    state.sort = Some((COL_TAPE.to_string(), false));
    assert_eq!(uids(&mut state)[0], 2, "covered first");
    state.sort = Some((COL_MODEL.to_string(), false));
    assert_eq!(uids(&mut state)[0], 2, "both hits first");
    assert_eq!(uids(&mut state)[2], 1, "unanswered last");
    // The cache is keyed by rows_rev and sort: an unchanged pair reuses it.
    let before = state.order.as_ref().map(|c| c.order.clone());
    let again: Vec<usize> = order_for(&mut state).to_vec();
    assert_eq!(before.as_deref(), Some(again.as_slice()));
}

#[test]
fn covered_and_fetchable_count_what_the_captions_say() {
    let state = state();
    let data = state.data.data().unwrap();
    assert_eq!(data.covered(), 1);
    assert_eq!(data.fetchable().count(), 0, "no address, nothing to ask");
    assert!(data.kinds.is_empty() && !data.entry_modelled());
}

// ---- the variant edits and the search gate ----------------------------------------------------

#[test]
fn variant_edits_fold_to_sorted_changes_and_empty_cells_clear() {
    let mut state = TicksState::default();
    assert!(!state.has_changes());
    state.set_variant(0, "SellPrice", " 0.5 ".into());
    state.set_variant(0, "MShotPrice", "2".into());
    state.set_variant(1, "StopLoss", "-1".into());
    assert_eq!(
        state.variant_changes(0),
        vec![
            ("MShotPrice".to_string(), "2".to_string()),
            ("SellPrice".to_string(), "0.5".to_string()),
        ]
    );
    assert!(state.has_changes());
    state.set_variant(0, "MShotPrice", "   ".into());
    assert_eq!(state.variant_changes(0).len(), 1, "a blank clears the cell");
    assert_eq!(state.variant_changes(1).len(), 1);
}

#[test]
fn the_share_gate_answers_per_group_and_only_once_something_answered() {
    use moon_core::db::tuner::ticks::params::ParamGroup;
    let mut data = TicksData::default();
    assert_eq!(data.group_passes(ParamGroup::Entry), None);
    data.entry_share = (8, 10);
    data.exit_share = (7, 10);
    assert_eq!(data.group_passes(ParamGroup::Entry), Some(true));
    assert_eq!(data.group_passes(ParamGroup::Exit), Some(false));
    data.kinds = vec!["MoonShot".into()];
    assert_eq!(data.single_kind(), Some("MoonShot"));
    data.kinds.push("Spread".into());
    assert_eq!(data.single_kind(), None);
}

#[test]
fn invalidate_stops_the_search_and_drops_the_variant_scores_but_keeps_the_edits() {
    let mut state = state();
    state.set_variant(0, "SellPrice", "1".into());
    state.var_stats[0] = Some(moon_core::db::tuner::VarStats::default());
    let handle = moon_core::db::tuner::threshold_search::SearchHandle::new();
    state.sugg = super::super::state::SuggState::Running {
        handle: handle.clone(),
        total: 3,
    };
    state.invalidate();
    assert!(handle.is_cancelled());
    assert!(matches!(state.sugg, super::super::state::SuggState::Idle));
    assert!(state.var_stats[0].is_none());
    assert!(
        state.has_changes(),
        "the user's edits survive a scope change"
    );
}
