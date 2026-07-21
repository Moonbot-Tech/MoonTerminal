//! Unit tests for persisted filter-tuner controls.

use super::{DEFAULT_EDGES, DEFAULT_ITERS, EDGE_OPTIONS, iters_of, restore_edges, restore_iters};
use moon_core::db::tuner_smart::{EDGES_MAX, EDGES_MIN, RESTARTS_MAX, RESTARTS_MIN};

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
