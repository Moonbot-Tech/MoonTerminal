//! Shared coin-menu value and delayed workspace-authority regressions.

use super::{blacklist_add, blacklist_contains};

#[test]
fn add_to_empty() {
    assert_eq!(blacklist_add("", "ADA"), "ADA");
    assert_eq!(blacklist_add("   ", "ADA"), "ADA");
}

#[test]
fn add_appends() {
    assert_eq!(blacklist_add("BTC,ETH", "ADA"), "BTC,ETH,ADA");
    assert_eq!(blacklist_add("BTC,ETH,", "ADA"), "BTC,ETH,ADA");
}

#[test]
fn dedup_case_insensitive() {
    assert_eq!(blacklist_add("BTC,ada", "ADA"), "BTC,ada");
    assert!(blacklist_contains("BTC, ada , ETH", "ADA"));
    assert!(!blacklist_contains("BTC,ETH", "ADA"));
}

/// Every mutating callback in `coin_menu.rs:build_items` must validate current workspace authority
/// inside its Backend update and before its first command helper.
///
/// Mutation: remove or move the guard after any listed command. A menu opened on core 7 could then
/// blacklist, join, split, cancel, or open an editor after Auto switched the group to core 9.
///
/// Returns:
///     Nothing; every listed callback must retain guard-before-effect ordering.
#[test]
fn shared_menu_mutations_revalidate_before_their_first_side_effect() {
    let source = include_str!("../coin_menu.rs");
    let cases = [
        ("\"coin-bl-core\"", "add_to_core_blacklist(b, core"),
        ("\"coin-bl-cores\"", "for &c in &cores"),
        ("\"coin-bl-strat\"", "add_to_strategy_blacklist(b, core"),
        ("\"coin-order-edit\"", "crate::panels::open_order_edit("),
        ("\"coin-order-join\"", "b.session.join_sells("),
        ("\"coin-order-split\"", "b.session.split_order("),
        ("\"coin-order-cancel\"", "b.session.cancel_order("),
    ];

    for (key, side_effect) in cases {
        let callback = source
            .split_once(key)
            .unwrap_or_else(|| panic!("missing shared-menu action {key}"))
            .1;
        let update = callback
            .find(".update(app, |b, _|")
            .unwrap_or_else(|| panic!("{key} must re-read Backend authority"));
        let guard = callback
            .find("workspace_action_allows_cores(")
            .unwrap_or_else(|| panic!("{key} must validate its captured core targets"));
        let effect = callback
            .find(side_effect)
            .unwrap_or_else(|| panic!("{key} lost its expected side effect"));

        assert!(
            update < guard && guard < effect,
            "stale-action guard moved in {key}"
        );
    }
}

/// `coin_menu.rs:coin-bl-strat` must re-read the strategy and schema inside its callback; removing
/// the dispatch-time check sends a stale empty blacklist edit after the strategy disappears.
#[test]
fn strategy_blacklist_callback_revalidates_live_identity_and_schema() {
    let source = include_str!("../coin_menu.rs");
    let callback = source
        .split_once("\"coin-bl-strat\"")
        .expect("strategy blacklist action must exist")
        .1;
    let update = callback
        .find(".update(app, |b, _|")
        .expect("callback must re-enter Backend");
    let schema_guard = callback
        .find("strategy_has_blacklist_field(b, core, sid)")
        .expect("callback must revalidate the exact strategy schema");
    let effect = callback
        .find("add_to_strategy_blacklist(b, core, sid")
        .expect("callback must retain its intended edit");

    assert!(update < schema_guard && schema_guard < effect);
}

/// `CoinMenuCtx::workspace_group` must distinguish group-owned panels and charts from intentionally
/// unscoped global Assets and standalone Report surfaces.
///
/// Mutation: pass `None` from a group panel or a group from an unscoped host. The former restores
/// stale Auto writes; the latter silently removes Classic/global action authority.
///
/// Returns:
///     Nothing; source wiring must preserve each host's intended authority boundary.
#[test]
fn menu_callers_preserve_scoped_and_unscoped_authority() {
    let compact =
        |source: &str| -> String { source.chars().filter(|ch| !ch.is_whitespace()).collect() };
    let orders = compact(include_str!("../../panels/orders/table.rs"));
    let assets = compact(include_str!("../../panels/assets/table.rs"));
    let report_actions = compact(include_str!("../../panels/report/actions.rs"));
    let report_columns = compact(include_str!("../../panels/report/columns.rs"));
    let chart = compact(include_str!("../../panels/chart/trade.rs"));

    assert!(orders.contains("workspace_group:Some(workspace_group)"));
    assert!(assets.contains("AssetsScope::Group(group)=>Some(group.clone())"));
    assert!(assets.contains("AssetsScope::All=>None"));
    assert!(report_actions.contains("(!self.standalone).then(||self.group.clone())"));
    assert!(report_columns.contains("workspace_group,"));
    assert!(chart.contains("workspace_group:self.workspace_group.clone()"));
}

/// Shared-menu Open, Compare, and Strategy entries must carry the same group authority as writes.
///
/// Mutation: call an unconditional request or omit `workspace_group` from `open_goto`. A menu
/// retained across a rail switch would navigate to its old core or switch singleton scope.
#[test]
fn shared_menu_navigation_revalidates_captured_workspace_scope() {
    let source = include_str!("../coin_menu.rs");

    assert!(source.contains("b.open_on_main_if_authorized("));
    assert!(source.contains("b.open_compare_if_authorized("));
    let goto = source
        .split("crate::strategies::open_goto(")
        .nth(1)
        .expect("strategy navigation must exist");
    assert!(goto.contains("workspace_group.clone(),"));
}
