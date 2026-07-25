//! Unit tests for persisted filter-tuner controls.

use super::{
    DEFAULT_EDGES, DEFAULT_ITERS, EDGE_OPTIONS, TunerState, iters_of, restore_edges, restore_iters,
    staged_dirty,
};
use moon_core::db::tuner::StratFilters;
use moon_core::db::tuner_smart::{EDGES_MAX, EDGES_MIN, RESTARTS_MAX, RESTARTS_MIN};

/// `filter/state.rs:TunerState::mark_report_stale` must not add the draft resets or request
/// generation bumps from `invalidate`; the former clears edits and the latter starves a long
/// filter scan while new trades keep arriving.
#[test]
fn report_staleness_preserves_filter_drafts() {
    let mut state = TunerState::load(None, None);
    state.bounds[0][0] = ("1".into(), "2".into());
    state.staged_ignore.insert("IgnoreFilters", true);
    let (seq, hist_seq, sugg_seq, dialog_seq) =
        (state.seq, state.hist_seq, state.sugg_seq, state.dialog_seq);

    state.mark_report_stale();

    assert!(
        !state.variants()[1].is_empty(),
        "the KPI consumer must still receive the staged bound"
    );
    assert!(
        staged_dirty(&state.strat, &state.staged_ignore),
        "the Save consumer must still see the staged ignore edit"
    );
    assert_eq!(state.seq, seq);
    assert_eq!(state.hist_seq, hist_seq);
    assert_eq!(state.sugg_seq, sugg_seq);
    assert_eq!(
        state.dialog_seq, dialog_seq,
        "report data cannot invalidate a pending Save dialog"
    );
    assert!(state.needs_reload());
}

/// `filter/state.rs:TunerState::invalidate` must advance every read generation; replacing it with
/// report-only staleness lets old KPI, histogram, or suggestion completions publish in a new scope.
#[test]
fn scope_change_retires_all_filter_requests() {
    let mut state = TunerState::load(None, None);
    let (seq, hist_seq, sugg_seq) = (state.seq, state.hist_seq, state.sugg_seq);

    state.invalidate();

    assert_ne!(state.seq, seq);
    assert_ne!(state.hist_seq, hist_seq);
    assert_ne!(state.sugg_seq, sugg_seq);
    assert!(state.needs_reload());
}

/// `filter/state.rs:TunerState::needs_reload` must include `hist_dirty`; checking only KPI state
/// leaves an old histogram pinned when an automatic KPI finishes after the user leaves Filters.
#[test]
fn stale_histogram_keeps_the_filter_axis_reloadable() {
    let mut state = TunerState::load(None, None);
    state.stats.apply(Ok(Vec::new()));
    state.dirty = false;
    state.hist_dirty = true;

    assert!(state.needs_reload());
}

/// `filter/state.rs:TunerState::apply_strategy_read` must preserve a confirmed Save baseline
/// when an automatic refresh cannot read the strategy row; replacing it with an unconditional
/// assignment turns the next Save preview into a diff against invented defaults.
#[test]
fn automatic_missing_strategy_read_preserves_the_save_baseline() {
    let mut state = TunerState::load(None, None);
    state.apply_strategy_read(
        StratFilters {
            found: true,
            ignore_filters: true,
            ..Default::default()
        },
        false,
    );

    state.apply_strategy_read(StratFilters::default(), true);

    assert!(state.strat.found);
    assert!(state.strat.ignore_filters);
}

/// `filter/state.rs:TunerState::mark_dialog_draft_changed` must advance `dialog_seq`; removing
/// that update lets an async Save preview open for the draft that existed before the user's edit.
#[test]
fn draft_change_retires_pending_save_preview() {
    let mut state = TunerState::load(None, None);
    let pending_preview = state.dialog_seq;

    state.mark_dialog_draft_changed();

    assert_ne!(
        state.dialog_seq, pending_preview,
        "the pending Save preview must no longer match the edited draft"
    );
}

/// Every depth the dropdown offers must be one the search will actually honour.
///
/// The oracle is the search's own accepted range, read from `moon-core` — source against
/// source, not a literal restated here. Breakage this pins: someone extends `EDGE_OPTIONS`
/// with a finer or coarser step (256 is the tempting next one) without touching
/// `smart_suggest`'s bounds. The dropdown would then offer a depth that `clamp` silently
/// rewrites, so the tuner would persist and display 256 while every run used 128.
#[test]
fn every_offered_depth_is_one_the_search_accepts() {
    for offered in EDGE_OPTIONS {
        assert!(
            (EDGES_MIN..=EDGES_MAX).contains(&offered),
            "depth {offered} is offered by the dropdown but outside what smart_suggest accepts"
        );
    }
    assert!(
        (EDGES_MIN..=EDGES_MAX).contains(&DEFAULT_EDGES),
        "the fallback depth must itself be acceptable to the search"
    );
}

/// The depth restored from `layout.toml` has to be one the dropdown can select again.
///
/// Breakage this pins: replacing `state.rs:restore_edges` membership validation with a range
/// clamp admits values between dropdown entries. A stored `5` would render with no highlighted
/// item, leaving the user in a state the UI cannot produce.
#[test]
fn depth_restores_only_values_the_dropdown_offers() {
    for offered in EDGE_OPTIONS {
        assert_eq!(
            restore_edges(Some(offered as u32)),
            offered,
            "a depth the dropdown offers must survive a restart unchanged"
        );
    }
    // Inside the search range but absent from the dropdown — the case a clamp would pass.
    for between in [5u32, 63, 100] {
        assert_eq!(
            restore_edges(Some(between)),
            DEFAULT_EDGES,
            "{between} is not selectable, so it must fall back to the default"
        );
    }
    for outside in [0u32, 3, 129, u32::MAX] {
        assert_eq!(restore_edges(Some(outside)), DEFAULT_EDGES);
    }
    assert_eq!(restore_edges(None), DEFAULT_EDGES);
}

/// What gets stored must reopen as the exact count the search will run.
///
/// The oracle is the agreement of two decoupled functions — `iters_of` (raw box text → the
/// number `suggest_into_v1` runs) and `restore_iters` (stored number → box text) — not a
/// literal restated from either.
///
/// Breakage this pins: removing the clamp from `state.rs:iters_of` lets the UI display and store
/// 99999 while the search executes `RESTARTS_MAX`. Re-hardcoding a bound there instead of
/// reading the const drifts the same way as soon as the search moves its own. Replacing
/// `restore_iters` with `saved.unwrap_or_default()` also opens an unset knob on 0 instead of the
/// default.
#[test]
fn restarts_reopen_as_the_count_the_search_runs() {
    for typed in [
        "", "   ", "abc", "0", "1", "7", "500", "1000", "2000", "2001", "99999", "-5",
    ] {
        let effective = iters_of(typed);
        assert_eq!(
            restore_iters(Some(effective as u32)),
            effective.to_string(),
            "storing what the search read from {typed:?} must reopen as that same value"
        );
        assert!((RESTARTS_MIN..=RESTARTS_MAX).contains(&effective));
    }
    // Boundary pair: the last accepted count passes through, one past it is pulled back.
    assert_eq!(iters_of(&RESTARTS_MAX.to_string()), RESTARTS_MAX);
    assert_eq!(iters_of(&(RESTARTS_MAX + 1).to_string()), RESTARTS_MAX);
    assert_eq!(iters_of(&RESTARTS_MIN.to_string()), RESTARTS_MIN);
    assert_eq!(iters_of("0"), RESTARTS_MIN);
    // An absent value opens on the default rather than on an empty box.
    assert_eq!(restore_iters(None), DEFAULT_ITERS.to_string());
}
