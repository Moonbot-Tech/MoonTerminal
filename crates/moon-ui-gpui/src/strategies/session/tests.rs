//! The Auto idle-collapse rule for a reopened Strategies window.
//!
//! Not `use super::*`: the strategies module imports `gpui::*`, whose `test` macro shadows
//! `#[test]`, and `session.rs` re-exposes that glob.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use moon_core::config::WorkspaceMode;
use moon_core::session::CoreId;

use crate::core_order::ExchangeSection;

use super::{
    STRATEGIES_IDLE_COLLAPSE, StrategiesSessionState, collapse_strategies_expansion,
    rail_overlay_on_open, strategies_closed_for, strategies_reopen_collapses,
};

fn fifteen_minutes() -> Duration {
    Duration::from_secs(15 * 60)
}

/// `STRATEGIES_IDLE_COLLAPSE` is fifteen minutes.
///
/// Mutation: shrinking the constant. A ten-minute Auto close would then drop expansion
/// the user still expects to find.
#[test]
fn idle_collapse_is_fifteen_minutes() {
    assert_eq!(STRATEGIES_IDLE_COLLAPSE, fifteen_minutes());
}

/// Auto closed for at least fifteen minutes collapses; one nanosecond less keeps.
///
/// Mutation: comparing with `>` instead of `>=`, or dropping the duration check.
/// The window would either keep a fifteen-minute-old tree or collapse a fresh one.
#[test]
fn auto_collapses_at_fifteen_minutes_and_keeps_a_shorter_close() {
    let auto = WorkspaceMode::AutoTrading;
    assert!(
        strategies_reopen_collapses(auto, Some(fifteen_minutes())),
        "a close of exactly fifteen minutes in Auto must collapse"
    );
    assert!(
        strategies_reopen_collapses(auto, Some(fifteen_minutes() + Duration::from_nanos(1))),
        "a close longer than fifteen minutes in Auto must collapse"
    );
    assert!(
        !strategies_reopen_collapses(auto, Some(fifteen_minutes() - Duration::from_nanos(1))),
        "a close shorter than fifteen minutes must keep expansion"
    );
    assert!(
        !strategies_reopen_collapses(auto, Some(Duration::ZERO)),
        "reopening immediately must keep expansion"
    );
    assert!(
        !strategies_reopen_collapses(auto, None),
        "a window that has not closed must keep expansion"
    );
}

/// Classic keeps expansion no matter how long the window was closed.
///
/// Mutation: collapsing on duration alone and ignoring the preset. Manual mode would
/// then forget the tree after the same idle gap.
#[test]
fn classic_keeps_expansion_after_a_long_close() {
    let classic = WorkspaceMode::Classic;
    assert!(!strategies_reopen_collapses(
        classic,
        Some(fifteen_minutes())
    ));
    assert!(!strategies_reopen_collapses(
        classic,
        Some(fifteen_minutes() * 4)
    ));
    assert!(!strategies_reopen_collapses(classic, None));
}

/// A backwards wall clock is not a long close.
///
/// Mutation: `duration_since(...).unwrap_or(STRATEGIES_IDLE_COLLAPSE)` or saturating at
/// zero and then treating zero as elapsed idle. A clock step backwards would collapse
/// a tree the user just closed.
#[test]
fn a_backwards_clock_does_not_count_as_time_closed() {
    let closed = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let earlier = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
    assert_eq!(strategies_closed_for(Some(closed), earlier), None);
    assert!(!strategies_reopen_collapses(
        WorkspaceMode::AutoTrading,
        strategies_closed_for(Some(closed), earlier),
    ));
    let later = closed + fifteen_minutes();
    assert_eq!(
        strategies_closed_for(Some(closed), later),
        Some(fifteen_minutes())
    );
    assert_eq!(strategies_closed_for(None, later), None);
}

/// An idle collapse does not open the Auto-selected core through the rail overlay.
///
/// Mutation: `rail_expanded_core = rail_seed` even when `collapse` is true. Reopening
/// Auto after fifteen minutes would then show the selected core's subtree, which is the
/// row the idle rule is supposed to shut. A shorter close still seeds that core.
#[test]
fn an_idle_collapse_leaves_the_rail_overlay_empty() {
    let core: CoreId = 4;
    assert_eq!(rail_overlay_on_open(true, Some(core)), None);
    assert_eq!(rail_overlay_on_open(false, Some(core)), Some(core));
    assert_eq!(rail_overlay_on_open(true, None), None);
    assert_eq!(rail_overlay_on_open(false, None), None);
}

/// Collapsing drops expansion and folder selection, and leaves search and filters.
///
/// Mutation: clearing `search` or `kind` inside `collapse_strategies_expansion`, or
/// leaving `expanded_folders` / `folder_sel` in place. The reopened Auto tree would
/// either forget the filter or still show the folders the idle gap was meant to close.
#[test]
fn collapse_drops_expansion_and_folder_selection_only() {
    let core: CoreId = 4;
    let mut state = StrategiesSessionState {
        expanded_cores: HashSet::from([core]),
        expanded_folders: HashSet::from([(core, "alpha/beta".to_string())]),
        expanded_deleted: HashSet::from([core]),
        selected: Some((core, 9)),
        sel: HashSet::from([(core, 9)]),
        folder_sel: HashSet::from([(core, "alpha".to_string())]),
        folder_anchor: Some((core, "alpha".to_string())),
        selected_section: 2,
        anchor: Some((core, 9)),
        search: "ema".to_string(),
        kind: Some(3),
        dir: Some(true),
        exchange: Some(ExchangeSection::Unidentified),
        ui_folders: HashSet::from([(core, "empty".to_string())]),
    };

    collapse_strategies_expansion(&mut state);

    assert!(state.expanded_cores.is_empty());
    assert!(state.expanded_folders.is_empty());
    assert!(state.expanded_deleted.is_empty());
    assert!(state.folder_sel.is_empty());
    assert_eq!(state.folder_anchor, None);
    assert_eq!(state.search, "ema");
    assert_eq!(state.kind, Some(3));
    assert_eq!(state.dir, Some(true));
    assert_eq!(state.exchange, Some(ExchangeSection::Unidentified));
    assert_eq!(state.selected, Some((core, 9)));
    assert_eq!(state.sel, HashSet::from([(core, 9)]));
    assert_eq!(state.anchor, Some((core, 9)));
    assert_eq!(state.selected_section, 2);
    assert_eq!(
        state.ui_folders,
        HashSet::from([(core, "empty".to_string())])
    );
}
