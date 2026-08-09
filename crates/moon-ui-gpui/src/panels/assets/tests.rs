//! Regression tests for retained Assets state across temporary Auto workspace scopes.

use std::collections::HashSet;

use super::{reconcile_retained_assets_state, resolve_workspace_wallet_core};

/// `cache.rs:AssetsView::rebuild_cache` must validate retained filters and wallet detail against
/// every live group core, not the effective one-core Auto query. Replacing the full validity set
/// with `[2]` drops core 1 and prevents Classic from restoring the prior view.
#[test]
fn auto_scope_does_not_prune_classic_filter_or_wallet_core() {
    let mut filter = HashSet::from([1, 2]);
    let mut wallet = Some(1);

    let changed = reconcile_retained_assets_state(&[1, 2, 3], &[2], &mut filter, &mut wallet);

    assert!(!changed);
    assert_eq!(filter, HashSet::from([1, 2]));
    assert_eq!(wallet, Some(1));
}

/// `cache.rs:AssetsView::rebuild_cache` must pass the full validity universe as the first
/// reconciliation argument.
///
/// Mutation: replace `&valid` with `&effective` at the actual cache call. The wiring assertion
/// reddens even if the pure reconciliation helper and its direct behavior test remain unchanged.
#[test]
fn rebuild_cache_wires_full_scope_into_retained_state_reconciliation() {
    let source = include_str!("cache.rs");
    let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();

    assert!(compact.contains(
        "super::reconcile_retained_assets_state(&valid,&effective,&mutself.sel_cores,&mutself.selected_core,)"
    ));
}

/// Workspace revision handling must reach pending-transfer invalidation through `rebuild_cache`.
///
/// Mutation: remove either the revision observer's rebuild or cache.rs's invalidation call. The
/// corresponding structural edge disappears and a dialog captured for the prior core stays armed.
#[test]
fn workspace_revision_reconciles_pending_transfer_target() {
    let panel_source = include_str!("mod.rs");
    let cache_source = include_str!("cache.rs");
    let panel_compact: String = panel_source
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    let cache_compact: String = cache_source
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();

    assert!(panel_compact.contains(
        "cx.observe(&workspace_revision,|this,_revision,cx|{letbackend=this.backend.clone();this.rebuild_cache(backend.read(cx));"
    ));
    assert!(cache_compact.contains(
        "leteffective_wallet_core=self.effective_wallet_core(b);self.invalidate_pending_transfer_for_wallet_core(effective_wallet_core);"
    ));
}

/// `mod.rs:resolve_workspace_wallet_core` must overlay without falling back under Auto ownership.
///
/// Mutation: append `.or(retained_core)` to Auto's result. Overview exposes core 1 instead of no
/// detail, while another selected Auto core may leak the Classic target when it disappears.
#[test]
fn wallet_target_restores_classic_but_has_no_auto_overview_fallback() {
    let retained = Some(1);

    assert_eq!(
        resolve_workspace_wallet_core(false, None, retained),
        Some(1)
    );
    assert_eq!(resolve_workspace_wallet_core(true, None, retained), None);
    assert_eq!(
        resolve_workspace_wallet_core(true, Some(2), retained),
        Some(2)
    );
    assert_eq!(
        resolve_workspace_wallet_core(false, None, retained),
        Some(1)
    );
}

/// `mod.rs:reconcile_retained_assets_state` must still prune a genuinely removed core and repair
/// the wallet target. Removing either repair leaves Classic with an empty filter or dead detail.
#[test]
fn removed_core_is_pruned_from_retained_assets_state() {
    let mut filter = HashSet::from([1, 2]);
    let mut wallet = Some(1);

    let changed = reconcile_retained_assets_state(&[2, 3], &[2], &mut filter, &mut wallet);

    assert!(changed);
    assert_eq!(filter, HashSet::from([2]));
    assert_eq!(wallet, Some(2));
}

/// Group Assets keeps Core shortcuts passive in Auto and validates delayed chart navigation.
///
/// Mutation: restore either Auto selection writer or replace the authorized Main request with the
/// unconditional one. A stale Assets row would then override or reveal a non-rail core.
#[test]
fn group_assets_shortcuts_cannot_bypass_the_auto_rail() {
    let panel = include_str!("mod.rs");
    let table = include_str!("table.rs");

    assert!(!panel.contains("select_auto_workspace_core"));
    assert!(!table.contains("select_auto_workspace_core"));
    assert!(table.contains("b.open_on_main_if_authorized("));
    assert!(table.contains("AssetsScope::Group(group) => Some(group.as_str())"));
    assert!(table.contains("AssetsScope::All => None"));
}
