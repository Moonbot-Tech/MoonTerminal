//! Workspace retargeting, detachment, and construction routing regressions for ChartTabs.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{
    AutoWorkspaceChartState, Tab, coin_search_bucket, preferred_auto_workspace_market,
    prune_coin_selection_to_scope, windows::chart_detach_allowed,
};
use moon_core::config::{ChartBucket, WorkspaceMode};
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;
use std::collections::HashSet;

/// Build one catalog-backed market label for selection-policy tests.
fn label(coin: &str, quote: &str) -> MarketLabel {
    MarketLabel {
        coin: coin.to_string(),
        quote: quote.to_string(),
        contract: None,
    }
}

/// Removing or reordering any branch in `chart_tabs/mod.rs:preferred_auto_workspace_market` must
/// fail: a rail click would choose another quote or contract even though the exact instrument is
/// available on the selected core.
#[test]
fn auto_market_selection_prefers_exact_then_pair_then_coin() {
    let current = label("AAVE_RP", "USD");
    let candidates = vec![
        ("AAVEUSDT".to_string(), label("AAVE", "USDT")),
        ("AAVEUSD_PERP".to_string(), label("AAVE_RP", "USD")),
        ("AAVEUSD".to_string(), label("AAVE", "USD")),
    ];

    assert_eq!(
        preferred_auto_workspace_market("AAVEUSD", &current, &candidates).as_deref(),
        Some("AAVEUSD")
    );
    assert_eq!(
        preferred_auto_workspace_market("MISSING", &current, &candidates).as_deref(),
        Some("AAVEUSD_PERP")
    );
    assert_eq!(
        preferred_auto_workspace_market("MISSING", &label("AAVE_RP", "BTC"), &candidates)
            .as_deref(),
        Some("AAVEUSDT")
    );
    assert_eq!(
        preferred_auto_workspace_market("MISSING", &label("SOL", "USDT"), &candidates),
        None
    );
}

/// Recording `applied_core` inside `chart_tabs/mod.rs:AutoWorkspaceChartState::candidate` must
/// fail: an initially unavailable target catalog would consume the rail click and never retry.
#[test]
fn auto_chart_target_commits_only_after_success_and_retries_new_catalog_revisions() {
    let current = Some((7, "BTCUSDT".to_string()));
    let mut restored = AutoWorkspaceChartState::seeded(Some(41), current.clone());
    assert_eq!(
        restored.candidate(Some(41), Some((41, 1)), current.clone()),
        Some(41),
        "a persisted selection must not suppress retargeting when Main belongs to another core"
    );

    let restored_target = Some((41, "BTCUSDT".to_string()));
    let mut matching = AutoWorkspaceChartState::seeded(Some(41), restored_target.clone());
    assert_eq!(
        matching.candidate(Some(41), Some((41, 1)), restored_target.clone()),
        None,
        "an actually matching restored Main target must not be replayed"
    );
    assert_eq!(
        matching.candidate(Some(41), Some((41, 2)), current.clone()),
        Some(41),
        "moving Main away from the selected Auto core must re-arm retargeting"
    );

    let mut pending = AutoWorkspaceChartState::default();
    assert_eq!(
        pending.candidate(Some(41), Some((41, 1)), current.clone()),
        Some(41)
    );
    assert_eq!(
        pending.candidate(Some(41), Some((41, 1)), current.clone()),
        None,
        "one unresolved snapshot must not be searched on every backend notification"
    );
    assert_eq!(
        pending.candidate(Some(41), Some((41, 2)), current.clone()),
        Some(41),
        "catalog readiness must retry the same selected core until Main accepts it"
    );
    pending.commit(41);
    assert_eq!(
        pending.candidate(Some(41), Some((41, 3)), Some((41, "BTCUSDT".to_string())),),
        None,
        "focus-only revisions must not replace a successfully applied target"
    );
    assert_eq!(pending.candidate(None, None, None), None);
    assert_eq!(pending.applied_core, None);
}

/// Removing the market-data notification in the coordination loop or the `ChartTabs` observer must
/// fail: a selected core whose catalog becomes ready later would remain stuck behind the previous
/// chart.
///
/// The loop moved to `startup/boot.rs` when the login window split startup in two, so the anchor
/// is matched without its leading indentation — the code is one nesting level shallower there and
/// nothing about this contract depends on how deeply it sits.
#[test]
fn market_data_revisions_wake_pending_auto_chart_retargets() {
    let main = include_str!("../main.rs");
    let startup = include_str!("../startup/boot.rs");
    let backend = include_str!("../backend/mod.rs");
    let chart_tabs = include_str!("mod.rs");
    assert!(main.contains("market_data_revision: Entity<MarketDataRevision>"));
    assert!(startup.contains(
        "if drain.market_data {\n                        b.market_data_revision.update(cx, |_, cx| cx.notify());"
    ));
    assert!(backend.contains("pub(crate) fn market_data_revision("));
    assert!(chart_tabs.contains(
        "cx.observe(&market_data_revision, |this, _revision, cx| {\n            // A failed rail retarget must wake"
    ));
}

/// Replacing `main_stack.rs:replace_or_focus`'s indexed insertion with `push` must fail: selecting
/// successive Auto rail cores would accumulate one Main chart per click instead of replacing the
/// current slot.
#[test]
fn auto_retarget_replaces_the_active_main_slot_without_appending() {
    let source = include_str!("main_stack.rs");
    let body = source
        .split("pub(super) fn replace_or_focus(")
        .nth(1)
        .and_then(|tail| {
            tail.split("/// Remove panels whose own panes are empty")
                .next()
        })
        .expect("Auto Main replacement method must precede empty-panel pruning");

    assert!(body.contains("self.remove_chart_at(active, cx);"));
    assert!(body.contains(".insert(active, ChartStackEntry::new(core, market, panel));"));
    assert!(!body.contains("self.charts.push("));
}

/// Allowing `WorkspaceMode::AutoTrading` in `chart_tabs/windows.rs:chart_detach_allowed`, or
/// moving its caller below geometry/window work, must fail: Auto would mutate Classic detached
/// chart persistence before refusing the operation.
#[test]
fn auto_refuses_chart_detach_before_window_or_persistence_work() {
    assert!(chart_detach_allowed(WorkspaceMode::Classic));
    assert!(!chart_detach_allowed(WorkspaceMode::AutoTrading));

    let source = include_str!("windows.rs");
    let body = source
        .split("pub(super) fn detach(")
        .nth(1)
        .and_then(|tail| tail.split("/// Open an OS window").next())
        .expect("ChartTabs detach method must precede window creation");
    let guard = body
        .find("chart_detach_allowed")
        .expect("Auto detachment guard must exist");
    for mutation in ["spec_geom", "open_chart_window", "upsert_spec"] {
        assert!(
            guard
                < body
                    .find(mutation)
                    .expect("detach mutation anchor must exist"),
            "Auto guard must run before {mutation}"
        );
    }
}

/// Removing the construction drain from `chart_tabs/mod.rs:ChartTabs::new` would strand a Main
/// request emitted before its group window and observer existed.
#[test]
fn construction_consumes_an_already_pending_request() {
    let source = include_str!("mod.rs");
    let constructor = source
        .split("pub fn new(")
        .nth(1)
        .and_then(|tail| tail.split("fn handle_open_request(").next())
        .expect("ChartTabs constructor must precede its request handler");

    assert!(constructor.contains("this.handle_open_request(false, cx);"));
}

/// Changing the constructor argument in `chart_tabs/mod.rs:ChartTabs::new` from `false` to `true`
/// would let an old activating request steal OS focus during startup; live observer delivery must
/// still preserve the request's activation semantics.
#[test]
fn startup_drain_suppresses_activation_while_live_delivery_preserves_it() {
    let source = include_str!("mod.rs");

    assert!(source.contains("this.handle_open_request(false, cx);"));
    assert!(source.contains("this.handle_open_request(true, cx);"));
    assert!(source.contains("if activate && allow_window_activation"));
}

/// `chart_tabs/mod.rs::coin_search_bucket` scopes the coin search popup to the Auto rail's
/// selected core while the active tab is Main or a custom tab.
///
/// Mutation: replacing `auto_core.map(ChartBucket::Core)` with `None` on the `Main | Custom` arm.
/// The popup would then fall back to the unscoped bucket and offer markets from every core in the
/// group's Auto workspace instead of only the one selected on the rail.
#[test]
fn coin_search_scopes_to_the_selected_auto_core_on_main_and_custom() {
    assert_eq!(
        coin_search_bucket(&Tab::Main, Some(41)),
        Some(ChartBucket::Core(41))
    );
    assert_eq!(coin_search_bucket(&Tab::Main, None), None);
    assert_eq!(
        coin_search_bucket(&Tab::Custom(100_000, ChartBucket::Shared), Some(7)),
        Some(ChartBucket::Core(7))
    );
}

/// `chart_tabs/mod.rs::prune_coin_selection_to_scope` drops accumulated coin checkboxes that fall
/// outside a newly selected Auto rail core.
///
/// Mutation: deleting the `selected.retain(...)` line while keeping the early `true`. A rail move
/// would then leave invisible markets of the previous core in the accumulated selection, inflating
/// the "Open in new tab" footer count with coins the popup no longer shows.
#[test]
fn prune_coin_selection_drops_markets_outside_the_new_scope() {
    let mut selected: HashSet<(CoreId, String)> = HashSet::from([
        (7, "BTCUSDT".to_string()),
        (7, "ETHUSDT".to_string()),
        (9, "SOLUSDT".to_string()),
    ]);

    let pruned = prune_coin_selection_to_scope(&mut selected, Some(7));

    assert!(pruned, "a selection spanning more than the new scope must report a change");
    assert_eq!(
        selected,
        HashSet::from([(7, "BTCUSDT".to_string()), (7, "ETHUSDT".to_string())])
    );
}
