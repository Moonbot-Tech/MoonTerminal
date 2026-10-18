//! Synthetic verdict and displayed-change regressions, without user data or a live view.

use std::collections::HashMap;
use std::time::Duration;

use super::super::state::{NowValue, TicksData, TicksState};
use super::{SummaryModel, Unchecked, Verdict, Why, WriteConfirm, verdict_of};
use crate::analytics::tuner::shared::{InlineWrite, SaveAuthority, SaveTarget};
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::ticks::search::{MIN_HOLDOUT, SearchResult};

/// Publishing stage-B rows must not turn an unanswered tape read into a zero-sample refusal.
#[test]
fn selected_strategy_waits_for_tape_before_refusing_the_sample() {
    let _locale = crate::test_locale::force("en");
    let mut state = TicksState::default();
    state.data.apply(Ok(TicksData::default()));
    state.tape_reading = true;
    state.sugg_refusal = Some("previous refusal".into());
    let loading = SummaryModel::from_state(&state, "fixture".into(), false);
    assert_eq!(loading.verdict, Verdict::Loading);
    assert_eq!(loading.tag, "Loading…");
    assert_eq!(loading.sentence, "reading the tape…");
    state.tape_reading = false;
    assert!(matches!(
        SummaryModel::from_state(&state, "fixture".into(), false).verdict,
        Verdict::Cannot(_)
    ));
}

/// An expired startup forecast must not pin hundreds of remaining restarts to one second.
#[test]
fn remaining_time_tracks_observed_progress_after_the_startup_estimate_expires() {
    use super::remaining_time;
    let elapsed = Duration::from_secs(229);
    assert_eq!(
        remaining_time(229, 500, elapsed, Some(Duration::from_secs(120)), 1),
        Some(Duration::from_secs(271))
    );
    assert_eq!(
        remaining_time(0, 500, elapsed, Some(Duration::from_secs(120)), 1),
        None
    );
    assert_eq!(
        remaining_time(
            0,
            500,
            Duration::from_secs(20),
            Some(Duration::from_secs(120)),
            1,
        ),
        Some(Duration::from_secs(100))
    );
    assert_eq!(remaining_time(500, 500, elapsed, None, 1), None);
    assert_eq!(remaining_time(501, 500, elapsed, None, 1), None);
}

/// Set synthetic slice figures without reaching the core tally's private cumulative peak.
fn tally(n: i64, profit: f64) -> Tally {
    let mut tally = Tally::default();
    tally.n = n;
    tally.profit = profit;
    tally
}

/// A synthetic result whose gains and sufficient holdout independently support Apply.
fn result() -> SearchResult {
    SearchResult {
        values: Vec::new(),
        searched: Vec::new(),
        train: tally(23, 17.0),
        fact_train: tally(23, 11.0),
        purged: 0,
        holdout: Some(tally(9, 8.0)),
        fact_holdout: Some(tally(9, 3.0)),
        holdout_open: 0,
        holdout_open_profit: 0.0,
        holdout_loses: false,
        seed: 43,
        stats: Default::default(),
    }
}

/// Omitting the embargo caption would hide why the chronological training sample shrank.
#[test]
fn summary_states_purged_training_deals() {
    let _locale = crate::test_locale::force("en");
    let mut state = TicksState::default();
    state.last_result = Some(result());
    state.last_result.as_mut().unwrap().purged = 2;
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert_eq!(
        model.train.unwrap().sub,
        "Trades: 23 (2 left out: their tape reaches the held-back period)"
    );
    state.last_result.as_mut().unwrap().purged = 0;
    assert_eq!(
        SummaryModel::from_state(&state, String::new(), false)
            .train
            .unwrap()
            .sub,
        "Trades: 23"
    );
}

/// Sample losses caused by held edits must refuse advice even when both scored slices improve.
#[test]
fn held_open_sample_mismatch_refuses_apply() {
    let mut r = result();
    r.stats.left_open_by_held = 3;
    assert_eq!(
        verdict_of(None, None, None, Some(&r), true),
        Verdict::DontApply(Why::HeldOpen { n: 3 })
    );
}

/// Weakening strict gains, ignoring open trades, or checking holdout before train changes advice.
#[test]
fn verdicts_agree_with_both_slices_and_require_strict_gains() {
    let mut cases = vec![(result(), Verdict::Apply)];
    let mut train_loss = result();
    train_loss.train.profit = 7.0;
    cases.push((train_loss, Verdict::DontApply(Why::Train)));
    let mut hold_loss = result();
    hold_loss.holdout.as_mut().unwrap().profit = -2.0;
    hold_loss.holdout_loses = true;
    cases.push((hold_loss, Verdict::DontApply(Why::HoldoutWorse)));
    let mut open = result();
    open.holdout_open = 2;
    open.holdout_loses = true;
    cases.push((open, Verdict::DontApply(Why::HoldoutOpen { k: 2 })));
    let mut whole = result();
    whole.holdout = None;
    whole.fact_holdout = None;
    cases.push((whole, Verdict::Unchecked(Unchecked::Whole)));
    let mut small = result();
    small.holdout.as_mut().unwrap().n = MIN_HOLDOUT - 1;
    small.holdout_open = 1;
    small.holdout_loses = true;
    cases.push((small, Verdict::Unchecked(Unchecked::Small(MIN_HOLDOUT - 1))));
    let mut train_tie = result();
    train_tie.train.profit = train_tie.fact_train.profit;
    cases.push((train_tie, Verdict::DontApply(Why::NoGain)));
    let mut hold_tie = result();
    hold_tie.holdout.as_mut().unwrap().profit = hold_tie.fact_holdout.as_ref().unwrap().profit;
    cases.push((hold_tie, Verdict::DontApply(Why::NoGain)));
    let mut flagged = result();
    flagged.holdout_loses = true;
    cases.push((flagged, Verdict::DontApply(Why::NoGain)));
    for (r, expected) in cases {
        let actual = verdict_of(None, None, None, Some(&r), true);
        assert_eq!(actual, expected);
        if actual == Verdict::DontApply(Why::HoldoutWorse) {
            assert!(r.holdout.as_ref().unwrap().profit < r.fact_holdout.as_ref().unwrap().profit);
        }
        if actual == Verdict::Apply {
            assert!(!r.holdout_loses && r.holdout_open == 0);
            assert!(r.train.profit > r.fact_train.profit);
            assert!(r.holdout.as_ref().unwrap().profit > r.fact_holdout.as_ref().unwrap().profit);
        }
    }
}

/// Fatal refusal must not override a running search or a current successful result.
#[test]
fn running_refusal_and_hand_edit_precedence_is_explicit() {
    let running = (4, 13, Some(Duration::from_secs(6)), 107, false);
    assert_eq!(
        verdict_of(Some(running), Some(2), Some("refused"), None, true),
        Verdict::Running {
            done: 4,
            total: 13,
            left: running.2,
            points: 107,
            stopping: false,
        }
    );
    assert_eq!(
        verdict_of(None, Some(2), None, Some(&result()), true),
        Verdict::Apply
    );
    assert!(matches!(
        verdict_of(None, Some(2), None, None, true),
        Verdict::Cannot(_)
    ));
    assert_eq!(
        verdict_of(None, None, Some("refused"), None, true),
        Verdict::Cannot("refused".into())
    );
    assert_eq!(
        verdict_of(None, None, Some("old refusal"), Some(&result()), true),
        Verdict::Apply
    );
    assert_eq!(
        verdict_of(None, None, None, None, true),
        Verdict::Unchecked(Unchecked::HandEdited)
    );
    assert_eq!(verdict_of(None, None, None, None, false), Verdict::Idle);
}

/// Reusing sugg_note as a fatal refusal would turn a successful range fallback into Cannot.
#[test]
fn informational_range_note_does_not_refuse_a_successful_search() {
    let _locale = crate::test_locale::force("en");
    let mut state = TicksState::default();
    state.last_result = Some(result());
    state.sugg_note =
        Some(rust_i18n::t!("analytics.ticks.range_set_aside", fields = "StopLoss").to_string());
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert_eq!(model.verdict, Verdict::Apply);
    assert!(
        model
            .warnings
            .iter()
            .any(|w| w == state.sugg_note.as_ref().unwrap())
    );
}

/// An expired forecast without measured restart throughput must disappear instead of claiming zero.
#[test]
fn running_time_left_is_unknown_when_the_unmeasured_forecast_expires() {
    let mut state = TicksState::default();
    let started = std::time::Instant::now() - Duration::from_secs(8);
    state.sugg = super::super::state::SuggState::Running {
        handle: moon_core::db::tuner::threshold_search::SearchHandle::new(),
        total: 13,
        parallel: 1,
        started,
        estimate: Some(Duration::from_secs(6)),
        sample: (23, 9),
        stopping: false,
    };
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert_eq!(
        model.verdict,
        Verdict::Running {
            done: 0,
            total: 13,
            left: None,
            points: 0,
            stopping: false,
        }
    );
}

/// Editing the next run's training share must not relabel a running holdout as whole-period.
#[test]
fn running_sentence_uses_the_launched_sample_after_dropdown_changes() {
    let _locale = crate::test_locale::force("en");
    let mut state = TicksState::default();
    state.start_search(
        moon_core::db::tuner::threshold_search::SearchHandle::new(),
        11,
        1,
        std::time::Instant::now(),
        None,
        (23, 9),
    );
    state.train_pct = 100;
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert_eq!(
        model.sentence,
        rust_i18n::t!("analytics.ticks.sum.running", n = 23, hold = 9)
    );
    assert_ne!(
        model.sentence,
        rust_i18n::t!("analytics.ticks.sum.running_whole", n = 32)
    );
}

/// Entry-point work must remain visible between restarts, while plain searches omit zero work.
#[test]
fn running_progress_keeps_entry_points_only_when_nonzero() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(
        super::progress_text(3, 11, 107),
        "Starts: 3 of 11 · entry points: 107"
    );
    assert_eq!(super::progress_text(3, 11, 0), "Starts: 3 of 11");
}

/// Leaving timing or refusal behind on scope invalidation would describe the previous scope.
#[test]
fn invalidation_retires_search_timing_and_refusal_with_the_result() {
    let mut state = TicksState::default();
    state.last_result = Some(result());
    state.last_search_elapsed = Some(Duration::from_secs(9));
    state.sugg_refusal = Some("old scope".into());
    state.invalidate();
    assert!(state.last_result.is_none());
    assert!(state.last_search_elapsed.is_none());
    assert!(state.sugg_refusal.is_none());
}

/// Counting open profit into the row would contradict the separately displayed closed profit.
#[test]
fn holdout_row_preserves_closed_profits_and_explains_its_open_trades() {
    let _locale = crate::test_locale::force("en");
    let mut state = TicksState::default();
    let mut found = result();
    found.holdout_open = 2;
    found.holdout_open_profit = 4.75;
    found.holdout_loses = true;
    state.last_result = Some(found);
    let model = SummaryModel::from_state(&state, String::new(), false);
    let row = model.holdout.unwrap();
    assert_eq!((row.n, row.fact, row.variant), (9, 3.0, 8.0));
    let line = row.open_line.unwrap();
    assert!(line.contains("Not closed in the tape: 2") && line.contains("4.75"));
    assert_eq!(model.verdict, Verdict::DontApply(Why::HoldoutOpen { k: 2 }));
}

/// A stale result must lose its slice rows; a hand variant still explains its own open trades.
#[test]
fn summary_reads_the_displayed_point_and_its_open_profit() {
    let _locale = crate::test_locale::force("en");
    let mut state = TicksState::default();
    state.last_result = Some(result());
    state.set_variant("StopLoss", "1.7".into());
    state.var_open = 3;
    state.var_open_profit = 7.25;
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert_eq!(model.verdict, Verdict::Unchecked(Unchecked::HandEdited));
    assert!(model.train.is_none() && model.holdout.is_none());
    assert!(model.details.is_empty());
    assert!(
        model
            .warnings
            .iter()
            .any(|w| w.contains("Not closed in the tape: 3") && w.contains("7.25"))
    );
}

/// Dropping the no-op filter or using the raw key for Differs would misstate changed fields.
#[test]
fn changed_header_counts_only_differing_fields_and_localizes_varies() {
    let _locale = crate::test_locale::force("ru");
    let mut state = TicksState::default();
    state.set_variant("StopLoss", "1.7".into());
    state.set_variant("SellPrice", "2.3".into());
    state.set_variant("StopLossDelay", "6".into());
    state.data.apply(Ok(TicksData {
        now: HashMap::from([
            ("StopLoss".into(), NowValue::Same("1.7".into())),
            ("SellPrice".into(), NowValue::Differs),
            ("StopLossDelay".into(), NowValue::Same("0".into())),
        ]),
        ..Default::default()
    }));
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert_eq!(model.changes.len(), 2);
    assert!(!model.changes.iter().any(|r| r.key == "StopLoss"));
    assert_eq!(
        model
            .changes
            .iter()
            .find(|r| r.key == "SellPrice")
            .unwrap()
            .old,
        "разные"
    );
    assert_eq!(model.changed_header, "Что поменялось · полей: 2");
}

/// A blank stored field must remain an explicit muted absence before the change arrow.
#[test]
fn changed_fields_show_absence_for_missing_empty_and_whitespace_old_values() {
    for old in [None, Some(""), Some("  \t")] {
        let mut state = TicksState::default();
        state.set_variant("PriceToSwitch2Stop", "-2".into());
        let mut data = TicksData::default();
        if let Some(old) = old {
            data.now
                .insert("PriceToSwitch2Stop".into(), NowValue::Same(old.into()));
        }
        state.data.apply(Ok(data));
        let model = SummaryModel::from_state(&state, "fixture".into(), false);
        assert_eq!(model.changes.len(), 1);
        assert_eq!(model.changes[0].old, "—");
        assert_eq!(model.changes[0].new, "-2");
    }
    let changes = vec![("PriceToSwitch2Stop".into(), "-2".into())];
    for old in ["0", "-1"] {
        let now = HashMap::from([("PriceToSwitch2Stop".into(), NowValue::Same(old.into()))]);
        assert_eq!(super::change_rows(&changes, &now, &[], false)[0].old, old);
    }
    let now = HashMap::from([("PriceToSwitch2Stop".into(), NowValue::Same("-2".into()))]);
    assert!(super::change_rows(&changes, &now, &[], false).is_empty());
}

/// Capture a synthetic consent for the state tests without connecting to a real core.
fn consent(state: &TicksState) -> WriteConfirm {
    WriteConfirm {
        write: InlineWrite {
            authority: SaveAuthority {
                producer: crate::analytics::tuner::save::SaveProducer::Selection,
                dialog_seq: 7,
                workspace_generation: Some(9),
                workspace_cores: Some(vec![33]),
                targets: vec![(303, Some(33))],
            },
            targets: vec![SaveTarget {
                sid: 303,
                core: Some(33),
                name: "fixture".into(),
            }],
            changes: state.variant_changes(),
        },
        name: "fixture".into(),
        core: "test core".into(),
        n: state.variant_changes().len(),
        holdout_worse: false,
        reason: None,
        warns: Vec::new(),
    }
}

/// Omitting consent retirement would leave a previous V1 confirm visible after a field edit.
#[test]
fn editing_or_clearing_a_variant_retires_inline_consent() {
    let mut state = TicksState::default();
    state.set_variant("StopLoss", "1.7".into());
    state.write_confirm = Some(consent(&state));
    state.sugg_refusal = Some("old point".into());
    state.set_variant("StopLoss", "2.6".into());
    assert!(state.write_confirm.is_none());
    assert!(state.sugg_refusal.is_none());
    assert_eq!(
        state.variant_changes(),
        vec![("StopLoss".to_string(), "2.6".to_string())]
    );
    state.write_confirm = Some(consent(&state));
    state.set_variant("StopLoss", String::new());
    assert!(state.write_confirm.is_none());
    assert!(!state.has_changes());
}

/// A scope change must retire inline consent while preserving the user's variant draft.
#[test]
fn invalidation_retires_inline_consent_and_keeps_the_draft() {
    let mut state = TicksState::default();
    state.set_variant("StopLoss", "1.7".into());
    state.write_confirm = Some(consent(&state));
    state.invalidate();
    assert!(state.write_confirm.is_none());
    assert_eq!(
        state.variant_changes(),
        vec![("StopLoss".to_string(), "1.7".to_string())]
    );
}

/// Launching another search must not keep the previous point's consent or refusal alive.
#[test]
fn a_search_start_retires_inline_consent_before_publishing_running() {
    let mut state = TicksState::default();
    state.set_variant("StopLoss", "1.7".into());
    state.write_confirm = Some(consent(&state));
    state.sugg_refusal = Some("previous run".into());
    state.start_search(
        moon_core::db::tuner::threshold_search::SearchHandle::new(),
        13,
        1,
        std::time::Instant::now(),
        Some(Duration::from_secs(6)),
        (23, 9),
    );
    assert!(state.write_confirm.is_none());
    assert!(state.sugg_refusal.is_none());
    assert!(matches!(
        state.sugg,
        super::super::state::SuggState::Running { total: 13, .. }
    ));
    assert_eq!(
        state.variant_changes(),
        vec![("StopLoss".to_string(), "1.7".to_string())]
    );
}

/// Stop keeps the run's generation, so its answer still lands; a later hard stop drops it.
#[test]
fn finish_search_keeps_the_generation_until_a_hard_stop() {
    let mut state = TicksState::default();
    let handle = moon_core::db::tuner::threshold_search::SearchHandle::new();
    state.start_search(
        handle.clone(),
        13,
        1,
        std::time::Instant::now(),
        None,
        (23, 9),
    );
    let seq = state.sugg_seq;
    state.finish_search();
    // A second press while the run winds down changes nothing.
    state.finish_search();
    assert_eq!(state.sugg_seq, seq);
    assert!(matches!(
        state.sugg,
        super::super::state::SuggState::Running { stopping: true, .. }
    ));
    assert!(handle.is_cancelled() && handle.keeps());
    // The card disables Stop from the verdict's `stopping`.
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert!(matches!(
        model.verdict,
        Verdict::Running { stopping: true, .. }
    ));
    state.stop_search("scope change");
    assert_ne!(state.sugg_seq, seq);
    assert!(matches!(state.sugg, super::super::state::SuggState::Idle));
    assert!(handle.is_cancelled() && !handle.keeps());
}

/// A stopped run shows its answer as a finished one, with the Stop said among the warnings.
#[test]
fn a_stopped_result_reads_as_finished_with_its_note() {
    let mut state = TicksState::default();
    let mut found = result();
    found.stats.stopped = true;
    state.last_result = Some(found);
    state.sugg_note = Some("stopped note".into());
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert!(!matches!(
        model.verdict,
        Verdict::Running { .. } | Verdict::Cannot(_) | Verdict::Idle
    ));
    assert!(model.warnings.iter().any(|w| w == "stopped note"));
}

/// Stopped before any variant: the empty state carries the Stop's own reason.
#[test]
fn a_stop_before_any_variant_is_the_cannot_state() {
    let _locale = crate::test_locale::force("en");
    let mut state = TicksState::default();
    let empty = rust_i18n::t!("analytics.ticks.sum.stopped_empty").to_string();
    state.sugg_note = Some(empty.clone());
    state.sugg_refusal = Some(empty.clone());
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert_eq!(model.verdict, Verdict::Cannot(empty));
}

/// While the run winds down there is no time left to promise.
#[test]
fn a_stopping_run_shows_no_time_left() {
    let mut state = TicksState::default();
    state.sugg = super::super::state::SuggState::Running {
        handle: moon_core::db::tuner::threshold_search::SearchHandle::new(),
        total: 13,
        parallel: 1,
        started: std::time::Instant::now(),
        estimate: Some(Duration::from_secs(600)),
        sample: (23, 9),
        stopping: true,
    };
    let model = SummaryModel::from_state(&state, String::new(), false);
    assert!(matches!(
        model.verdict,
        Verdict::Running {
            left: None,
            stopping: true,
            ..
        }
    ));
}

/// The note a landing run leaves, for every way it can end.
#[test]
fn a_landing_run_picks_its_note() {
    use super::super::state::{StopOutcome, stopped_note};
    let _locale = crate::test_locale::force("en");
    let after = |n: usize, m: usize| {
        rust_i18n::t!("analytics.ticks.sum.stopped_after", n = n, m = m).to_string()
    };
    let empty = rust_i18n::t!("analytics.ticks.sum.stopped_empty").to_string();
    let found = || StopOutcome::Found {
        restarts: 4,
        requested: 13,
    };
    let missed = |too_few| StopOutcome::Missed {
        too_few,
        note: "miss".into(),
        completed: 2,
        launched: 13,
    };
    let launch = || Some("set aside".to_string());
    // Not stopped: an answer keeps the launch's note, a miss replaces it.
    assert_eq!(
        stopped_note(false, found(), false, launch()),
        (launch(), None)
    );
    let miss = (Some("miss".to_string()), Some("miss".to_string()));
    assert_eq!(stopped_note(false, missed(false), true, launch()), miss);
    // Stopped answer: the launch's note first, then how much of the run stands behind it.
    assert_eq!(
        stopped_note(true, found(), false, launch()),
        (Some(format!("set aside {}", after(4, 13))), None)
    );
    assert_eq!(
        stopped_note(true, found(), false, None),
        (Some(after(4, 13)), None)
    );
    // A sample too small to search keeps its own reason.
    assert_eq!(stopped_note(true, missed(true), true, launch()), miss);
    // Stopped miss with a candidate in В1: never "no variants".
    assert_eq!(
        stopped_note(true, missed(false), true, launch()),
        (Some(after(2, 13)), Some("miss".to_string()))
    );
    // Stopped before anything: the Stop is the refusal.
    assert_eq!(
        stopped_note(true, missed(false), false, launch()),
        (Some(empty.clone()), Some(empty))
    );
}
