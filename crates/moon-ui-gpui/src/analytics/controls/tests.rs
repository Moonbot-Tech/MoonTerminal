//! Filter reset and independent Analytics period ownership contracts.
//!
//! Explicit imports, never `use super::*`: the parent glob-imports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute.

use super::{SummaryResetPlan, summary_reset_plan};
use moon_core::db::SideFilter;

/// Every filter off its default is reset once; defaults stay untouched and an all-default
/// state changes nothing, so no read starts; a pinned core filter is kept.
#[test]
fn reset_plan_clears_set_filters_and_keeps_the_pin() {
    let all = summary_reset_plan(SideFilter::Long, Some(true), "XALPHA", false, false);
    assert_eq!(
        all,
        SummaryResetPlan {
            side: true,
            emu: true,
            mask: true,
            cores: true
        }
    );
    assert!(all.any());
    let none = summary_reset_plan(SideFilter::All, None, "", true, false);
    assert_eq!(none, SummaryResetPlan::default());
    assert!(!none.any(), "nothing to reset must not start a read");
    let pinned = summary_reset_plan(SideFilter::All, None, "", false, true);
    assert!(
        !pinned.cores && !pinned.any(),
        "a pinned core filter is kept"
    );
    let mask_only = summary_reset_plan(SideFilter::All, None, "XALPHA", true, false);
    assert!(mask_only.mask && !mask_only.side && !mask_only.emu && !mask_only.cores);
}

/// A preset click must use page-owned invalidation, including clicking the active preset again.
#[test]
fn period_control_keeps_the_active_preset_as_a_page_refresh() {
    let controls = include_str!("../controls.rs");
    let setter = controls
        .split_once("fn set_period(")
        .expect("period setter")
        .1
        .split_once("fn sync_period_pickers(")
        .expect("period setter boundary")
        .0;
    assert!(setter.contains("self.reload_period(cx);"));
    assert!(!setter.contains("self.reload(cx);"));
    assert!(
        !setter.contains("return;"),
        "the active preset is a manual refresh"
    );
}

/// Broadening a page period change must not cancel another page or erase its tuner draft.
#[test]
fn independent_period_reload_preserves_other_pages() {
    let load = include_str!("../load.rs");
    let period = load
        .split_once("fn reload_period(")
        .expect("page period reload")
        .1
        .split_once("fn invalidate_strategy_scope(")
        .expect("period reload boundary")
        .0;
    let summary = period
        .split_once("Tab::Summary => {")
        .expect("Summary period branch")
        .1
        .split_once("Tab::Strategies => {")
        .expect("Strategies period branch")
        .0;
    assert!(summary.contains("bg::ReadLane::Summary"));
    assert!(summary.contains("self.reload_summary(false, true, cx);"));
    for invalidation in [
        "strategy_",
        "tuner",
        "coins",
        "ticks",
        "cal_",
        "cancel_axis_reads",
    ] {
        assert!(
            !summary.contains(invalidation),
            "Summary must preserve {invalidation}"
        );
    }
    let strategies = period
        .split_once("Tab::Strategies => {")
        .expect("Strategies period branch")
        .1
        .split_once("Tab::Calendar =>")
        .expect("Calendar period branch")
        .0;
    assert!(strategies.contains("bg::ReadLane::StrategyBase"));
    assert!(strategies.contains("self.cancel_axis_reads();"));
    assert!(strategies.contains("self.invalidate_strategy_scope();"));
    assert!(strategies.contains("self.reload_strategy_base(false, true, true, cx);"));
    for invalidation in [
        "ReadLane::Summary",
        "self.data_dirty",
        "self.cal_",
        "cancel_latest_reads",
    ] {
        assert!(
            !strategies.contains(invalidation),
            "Strategies must preserve {invalidation}"
        );
    }
}

/// A period click before mask debounce fires must still apply that shared filter to every page.
#[test]
fn pending_shared_mask_uses_full_scope_invalidation() {
    let load = include_str!("../load.rs");
    let period = load
        .split_once("fn reload_period(")
        .expect("page period reload")
        .1;
    let before_dispatch = period
        .split_once("match self.tab")
        .expect("period page dispatch")
        .0;
    assert!(before_dispatch.contains("StrategyQuery::parse(&self.strategy_mask_applied)"));
    assert!(before_dispatch.contains("StrategyQuery::parse(&self.strategy_mask)"));
    assert!(before_dispatch.contains("self.reload(cx);"));
    assert!(before_dispatch.contains("return;"));
}
