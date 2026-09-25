use super::super::columns::{COL_HELD, COL_MODEL, COL_PROFIT, COL_RESULT, COL_TAPE, COL_TIME};
use super::super::state::{DealRow, TapeStatus, TicksData, TicksState};
use super::{order_for, result_pct};
use moon_core::db::tuner::ticks::{Deal, Deltas, Verdict};
use moon_core::market::trade_replay::TickStatus;

fn deal(uid: i64, buy_ms: i64, buy: f64, sell: f64, short: bool) -> Deal {
    Deal {
        report_uid: uid,
        core_uid: 1,
        core_name: String::new(),
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
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        step_lag_ms: 0.0,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
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
    let mut rows = vec![
        DealRow {
            deal: deal(1, 3_000, 100.0, 101.0, false),
            tape: TapeStatus::Missing,
            verdict: None,
            address: None,
            ticks: None,
            entry_line: None,
            held: None,
        },
        DealRow {
            deal: deal(2, 1_000, 100.0, 99.0, false),
            tape: TapeStatus::Covered,
            verdict: Some(verdict(Some(true), Some(true))),
            address: None,
            ticks: None,
            entry_line: None,
            held: None,
        },
        DealRow {
            deal: deal(3, 2_000, 100.0, 99.0, true),
            tape: TapeStatus::Refused(TickStatus::NoRoute),
            verdict: Some(verdict(Some(false), None)),
            address: None,
            ticks: None,
            entry_line: None,
            held: None,
        },
    ];
    // Money as the core wrote it, deliberately NOT the sign of the price move: the profit sort
    // must read `profit`, not derive it from the prices.
    for (row, profit) in rows.iter_mut().zip([-3.0, 12.5, 0.0]) {
        row.deal.profit = Some(profit);
    }
    // What the terminal holds around each trade, `(lead, trail)`: the third row has nothing.
    rows[0].held = Some((60_000, 30_000));
    rows[1].held = Some((60_000, 60_000));
    let mut state = TicksState::default();
    // The sorts are exercised over every row; the "fit only" switch has its own test.
    state.only_fit = false;
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
fn the_sample_switch_keeps_the_covered_rows_the_model_reproduced() {
    // Row 2: covered, both groups ✓. Row 1: no tape. Row 3: refused, and the entry a miss.
    let mut state = state();
    assert!(
        !TicksState::default().only_fit,
        "off by default: the rows without tape are what the fetch button is for"
    );
    state.only_fit = true;
    assert_eq!(uids(&mut state), [2]);
    // A kind without an entry model answers the entry with nothing: the exit's ✓ is enough.
    state.data.data_mut().unwrap().rows[1].verdict = Some(verdict(None, Some(true)));
    state.rows_rev += 1;
    assert_eq!(uids(&mut state), [2]);
    // A covered row the model does not reproduce is out of the sample (the developer's call,
    // 2026-09-23): what the model answers for a variant of it is not an answer. The table
    // still shows it with the switch off, with its verdict in the "model" column.
    for missed in [
        verdict(Some(false), Some(true)),
        verdict(Some(true), Some(false)),
        verdict(Some(true), None),
    ] {
        state.data.data_mut().unwrap().rows[1].verdict = Some(missed);
        state.rows_rev += 1;
        assert!(uids(&mut state).is_empty(), "{missed:?}");
    }
    // A covered row the model has not run on yet is not in the sample either.
    state.data.data_mut().unwrap().rows[1].verdict = None;
    state.rows_rev += 1;
    assert!(uids(&mut state).is_empty());
    // Flipping the switch alone rebuilds the order: the cache keys on it.
    state.only_fit = false;
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

/// The profit column sorts by the row's money, whatever the prices say.
#[test]
fn profit_sorts_by_the_rows_money() {
    let mut state = state();
    state.sort = Some((COL_PROFIT.to_string(), true));
    assert_eq!(uids(&mut state), [2, 3, 1]);
    state.sort = Some((COL_PROFIT.to_string(), false));
    assert_eq!(uids(&mut state), [1, 3, 2]);
}

/// The held column sorts by the trail the terminal holds past the exit — what the exit
/// horizon of the sample is taken from; a row with nothing held sorts as the shortest.
#[test]
fn the_held_tape_sorts_by_its_trail() {
    let mut state = state();
    state.sort = Some((COL_HELD.to_string(), true));
    assert_eq!(uids(&mut state), [2, 1, 3]);
    state.sort = Some((COL_HELD.to_string(), false));
    assert_eq!(uids(&mut state), [3, 1, 2]);
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
    state.set_variant("SellPrice", " 0.5 ".into());
    state.set_variant("MShotPrice", "2".into());
    assert_eq!(
        state.variant_changes(),
        vec![
            ("MShotPrice".to_string(), "2".to_string()),
            ("SellPrice".to_string(), "0.5".to_string()),
        ]
    );
    assert!(state.has_changes());
    state.set_variant("MShotPrice", "   ".into());
    assert_eq!(state.variant_changes().len(), 1, "a blank clears the cell");
}

#[test]
fn the_share_gate_answers_per_group_and_only_once_something_answered() {
    use moon_core::db::tuner::ticks::params::ParamGroup;
    let mut data = TicksData::default();
    assert_eq!(data.group_passes(ParamGroup::Entry, 0.8), None);
    data.entry_share = (8, 10);
    data.exit_share = (7, 10);
    assert_eq!(data.group_passes(ParamGroup::Entry, 0.8), Some(true));
    assert_eq!(data.group_passes(ParamGroup::Exit, 0.8), Some(false));
    // The gate is the search settings' own: lowered, the exit group passes too.
    assert_eq!(data.group_passes(ParamGroup::Exit, 0.7), Some(true));
    data.kinds = vec!["MoonShot".into()];
    assert_eq!(data.single_kind(), Some("MoonShot"));
    data.kinds.push("Spread".into());
    assert_eq!(data.single_kind(), None);
}

#[test]
fn invalidate_stops_the_search_and_drops_the_variant_scores_but_keeps_the_edits() {
    let mut state = state();
    state.set_variant("SellPrice", "1".into());
    state.var_stats = Some(moon_core::db::tuner::VarStats::default());
    let handle = moon_core::db::tuner::threshold_search::SearchHandle::new();
    state.sugg = super::super::state::SuggState::Running {
        handle: handle.clone(),
        total: 3,
    };
    state.last_result = Some(moon_core::db::tuner::ticks::SearchResult {
        values: Vec::new(),
        searched: Vec::new(),
        train: Default::default(),
        holdout: Some(Default::default()),
        holdout_open: 0,
        seed: 1,
        stats: Default::default(),
    });
    state.invalidate();
    assert!(handle.is_cancelled());
    assert!(matches!(state.sugg, super::super::state::SuggState::Idle));
    assert!(state.var_stats.is_none());
    assert!(
        state.last_result.is_none(),
        "the last search's holdout is of the previous scope's deals"
    );
    assert!(
        state.has_changes(),
        "the user's edits survive a scope change"
    );
}

/// The gate reads the typed per cent, falls back to the default on anything else, and never
/// goes past the whole.
#[test]
fn the_gate_reads_the_typed_percent() {
    let mut state = TicksState::default();
    assert_eq!(
        state.gate(),
        f64::from(super::super::state::DEFAULT_GATE_PCT) / 100.0
    );
    state.gate_pct = "65".into();
    assert_eq!(state.gate(), 0.65);
    state.gate_pct = "250".into();
    assert_eq!(state.gate(), 1.0);
    state.gate_pct = "abc".into();
    assert_eq!(state.gate(), 0.8);
}

/// What the layout keeps of the axis comes back as it went out.
#[test]
fn the_axis_settings_restore_what_they_saved() {
    let mut state = TicksState::default();
    state.iters = "40".into();
    state.seed = "123".into();
    state.passes = "8".into();
    state.gate_pct = "70".into();
    state.train_pct = 80;
    state.locked.insert("SellPrice".into());
    let saved = state.saved();
    let mut back = TicksState::default();
    back.restore(&saved);
    assert_eq!(back.iters, "40");
    assert_eq!(back.seed, "123");
    assert_eq!(back.passes, "8");
    assert_eq!(back.gate_pct, "70");
    assert_eq!(back.train_pct, 80);
    assert!(back.locked.contains("SellPrice"));
    // A seed that is not a number is not kept.
    state.seed = "x1".into();
    assert_eq!(state.saved().seed, None);
}

/// A reload carries what the last load judged only for the same trade with the same stamps and
/// prices; a row rewritten under its uid is judged again.
#[test]
fn a_reload_carries_only_the_trade_it_judged() {
    let before = deal(7, 1_000, 100.0, 101.0, false);
    assert!(super::super::load::carryable(&before, &before.clone()));
    let mut moved = before.clone();
    moved.close_ms += 1;
    assert!(!super::super::load::carryable(&before, &moved));
    let mut repriced = before.clone();
    repriced.sell_price = 101.5;
    assert!(!super::super::load::carryable(&before, &repriced));
    let other = deal(8, 1_000, 100.0, 101.0, false);
    assert!(!super::super::load::carryable(&before, &other));
}

/// The fetch job's words on a hop land in order and in one pass: a mark, then that row's
/// answer, leaves the answer; a stop unmarks only what is still fetching; a row not in the
/// table is skipped.
#[test]
fn the_fetch_edits_land_in_order_in_one_pass() {
    use super::super::state::RowEdit;
    let mut state = state();
    let answer = DealRow {
        deal: deal(1, 3_000, 100.0, 101.0, false),
        tape: TapeStatus::Covered,
        verdict: Some(verdict(Some(true), Some(true))),
        address: None,
        ticks: None,
        entry_line: None,
        held: Some((60_000, 60_000)),
    };
    state.edit_rows([
        (1, RowEdit::MarkFetching),
        (1, RowEdit::Replay(Box::new(answer))),
        (2, RowEdit::UnmarkFetching),
        (99, RowEdit::MarkFetching),
    ]);
    let data = state.data.data().expect("data");
    let row = |uid: i64| {
        data.rows
            .iter()
            .find(|r| r.deal.report_uid == uid)
            .expect("row")
    };
    assert_eq!(row(1).tape, TapeStatus::Covered);
    assert!(row(1).verdict.is_some());
    // Covered, not fetching: the unmark leaves it alone.
    assert_eq!(row(2).tape, TapeStatus::Covered);
    // The answer was counted: both judged rows are fit now.
    assert_eq!(data.fit(), 2);
}

/// A packed tape of `n` prints, one a millisecond.
fn tape_of(n: usize) -> super::super::tape::PackedTape {
    super::super::tape::PackedTape::pack(
        (0..n)
            .map(|i| moon_core::feed::types::Tick {
                time_ms: 1_000.0 + i as f64,
                price: 1.0,
                qty: 1.0,
                side: moon_core::feed::types::Side::Buy,
            })
            .collect(),
    )
}

/// Only a fit row is ever replayed: a tape on any other row is let go, and what stays is
/// counted in bytes, packed.
#[test]
fn the_cap_keeps_only_the_fit_rows_tapes_and_counts_them_packed() {
    let mut data = TicksData {
        rows: vec![
            DealRow {
                deal: deal(1, 1_000, 100.0, 101.0, false),
                tape: TapeStatus::Covered,
                verdict: Some(verdict(Some(true), Some(true))),
                address: None,
                ticks: Some(tape_of(10)),
                entry_line: None,
                held: Some((60_000, 60_000)),
            },
            DealRow {
                deal: deal(2, 2_000, 100.0, 99.0, false),
                tape: TapeStatus::Covered,
                verdict: Some(verdict(Some(false), Some(false))),
                address: None,
                ticks: Some(tape_of(10)),
                entry_line: None,
                held: None,
            },
        ],
        ..TicksData::default()
    };
    assert!(data.rows[0].fit() && !data.rows[1].fit());
    data.retain_within_cap();
    assert!(data.rows[0].ticks.is_some());
    assert!(
        data.rows[1].ticks.is_none(),
        "an unfit row's tape is never replayed"
    );
    let budget = data.tape_budget();
    assert_eq!(
        (budget.rows, budget.fit, budget.replayable, budget.dropped),
        (2, 1, 1, 0)
    );
    assert_eq!((budget.prints, budget.bytes), (10, 80));
}

/// A fit row the cap left tapeless is what a scope reload must read again; a row that holds
/// its tape, or one that is not fit, is carried as it is.
#[test]
fn a_fit_row_without_its_tape_is_the_one_that_lost_it() {
    let mut row = DealRow {
        deal: deal(1, 1_000, 100.0, 101.0, false),
        tape: TapeStatus::Covered,
        verdict: Some(verdict(Some(true), Some(true))),
        address: None,
        ticks: Some(tape_of(3)),
        entry_line: None,
        held: Some((60_000, 60_000)),
    };
    assert!(!row.lost_tape());
    row.ticks = None;
    assert!(row.lost_tape());
    row.verdict = Some(verdict(Some(false), Some(true)));
    assert!(!row.lost_tape(), "an unfit row keeps no tape by design");
}

/// A reproduced row whose tape stops short of the shortest tail past the close is out of the
/// sample: nothing held past the close against the setting in force, whatever the store's
/// margin caps it at, while two hours reach any of them.
#[test]
fn a_reproduced_row_with_a_short_tail_is_not_fit() {
    let mut row = DealRow {
        deal: deal(1, 1_000, 100.0, 101.0, false),
        tape: TapeStatus::Covered,
        verdict: Some(verdict(Some(true), Some(true))),
        address: None,
        ticks: Some(tape_of(3)),
        entry_line: None,
        held: Some((60_000, 0)),
    };
    assert!(!row.fit());
    row.held = Some((60_000, 7_200_000));
    assert!(row.fit());
}
