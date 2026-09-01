//! Static regressions for strategy-window navigation authority.

/// Group-owned strategy reveals must reject stale targets without selecting another Auto core.
///
/// Mutation: restore `activate_auto_workspace_core` or queue `strategies_goto` before the current
/// authority check. An old Orders/menu callback would then move the workspace or reveal hidden data.
#[test]
fn stale_strategy_reveal_never_retargets_the_workspace() {
    let source = include_str!("../window.rs");
    let selection = include_str!("../selection.rs");
    let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();

    assert!(!source.contains("activate_auto_workspace_core"));
    assert!(
        compact.contains(
            "if!workspace_allows_reveal(b,workspace_group.as_deref(),core){returnfalse;}"
        )
    );
    assert!(compact.contains(
        "StrategyRevealRequest::new(core,RevealTarget::Id(strat_id),workspace_group.clone()"
    ));
    assert!(compact.contains(
        "letworkspace_group=b.singleton_workspace().map(|workspace|workspace.group);if!workspace_allows_reveal(b,workspace_group.as_deref(),core){returnfalse;}"
    ));
    assert!(
        selection
            .matches(".is_authorized(self.backend.read(cx))")
            .count()
            >= 2
    );
}

/// Every direct `open_goto` caller must classify its captured authority explicitly.
///
/// Mutation: remove the group argument from Orders, CoinMenu, Analytics, or the manual-strategy
/// popup. That caller would become unscoped and could reveal its retained core after Auto
/// navigation.
#[test]
fn every_strategy_navigation_producer_passes_workspace_authority() {
    let orders = include_str!("../../panels/orders/table.rs");
    let menu = include_str!("../../controls/coin_menu.rs");
    let tuner = include_str!("../../analytics/tuner/mod.rs");
    let ms_slots = include_str!("../../controls/manual_strat/settings.rs");

    assert!(orders.contains("Some(workspace_group),"));
    assert!(menu.contains("workspace_group.clone(),"));
    assert!(tuner.contains("strategy_id,\n            workspace_group,"));
    // Its authority is the group whose header owns the popup, taken as an argument rather than
    // from `singleton_workspace()`, which names whichever group last held Auto focus.
    assert!(ms_slots.contains("Some(workspace_group.clone()),"));
    assert!(!ms_slots.contains("singleton_workspace"));
}
