//! Regression tests for retained Assets state across temporary Auto workspace scopes.

use std::collections::{HashMap, HashSet};

use super::{reconcile_retained_assets_state, resolve_workspace_wallet_core, roster_width};

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

/// `roster_width.rs::DEFAULT_BASE_W` must remain the inverse-scaled shipped default, not 420.0.
///
/// Mutation: change the base width to the old rendered `420.0`. At the default Font-slider scale
/// the roster would render near 496 px instead of 420 px, silently taking width from all wallets.
#[test]
fn roster_default_renders_at_the_shipped_width_scale() {
    let rendered =
        roster_width::resolved(&HashMap::new(), roster_width::DEFAULT_BASE_W) * (13.0 / 11.0);

    assert!(
        (rendered - 420.0).abs() < 0.05,
        "the unconfigured roster must render at the old 420 px default, got {rendered}"
    );
}

/// `roster_width.rs::auto_base` must convert rendered row pixels back to base-width units.
///
/// Mutation: drop `/ scale` before `ceil`. At a raised Font slider the roster would be scaled
/// twice, consuming the wallet columns even though the measured row already fits exactly once.
#[test]
fn roster_auto_width_round_trips_rendered_pixels_through_font_scale() {
    let base = roster_width::auto_base(600.0, 20.0, 1.25);

    assert_eq!(base, 496.0);
    assert_eq!(base * 1.25, 620.0);
}

/// `roster_width.rs::auto_base` must retain the shipped rendered floor for short core names.
///
/// Mutation: clamp the automatic result to `MIN_BASE_W` instead of `DEFAULT_BASE_W`. A fresh
/// install with short names would narrow the roster below the historical 420-pixel width.
#[test]
fn roster_auto_width_keeps_the_shipped_floor_for_short_rows() {
    let rendered = roster_width::auto_base(20.0, 20.0, 13.0 / 11.0) * (13.0 / 11.0);

    assert!(
        (rendered - 420.0).abs() < 0.05,
        "short rows must retain the shipped 420 px roster width, got {rendered}"
    );
}

/// `roster_width.rs::auto_base` must cap one pathological name before it starves wallet columns.
///
/// Mutation: drop the upper clamp. A single very long core name would take nearly all horizontal
/// space and make the Spot, Futures, and Quarterly figures unreadable.
#[test]
fn roster_auto_width_caps_pathological_rows_before_the_wallet_columns_starve() {
    let rendered = roster_width::auto_base(2_000.0, 20.0, 13.0 / 11.0) * (13.0 / 11.0);

    assert!(
        rendered <= 852.0,
        "the roster cap must leave room for the three wallet columns, got {rendered} px"
    );
}

/// `roster_width.rs::auto_base` must reject unusable scale and measurement inputs.
///
/// Mutation: remove the non-finite guards. A hand-edited Font scale or invalid text measurement
/// would reach `f32::clamp`, panic on NaN, or replace the normal roster width with a bogus value.
#[test]
fn roster_auto_width_rejects_non_finite_scale_and_measurements() {
    for (case, widest_row_px, chrome_px, scale) in [
        ("zero scale", 600.0, 20.0, 0.0),
        ("NaN scale", 600.0, 20.0, f32::NAN),
        ("infinite measurement", f32::INFINITY, 20.0, 1.25),
    ] {
        assert_eq!(
            roster_width::auto_base(widest_row_px, chrome_px, scale),
            roster_width::DEFAULT_BASE_W,
            "{case} must fall back to the shipped base width"
        );
    }
}

/// `roster_width.rs::resolved` must prefer a finite user drag and skip a NaN one.
///
/// Mutation: let `auto` win before the stored width, or clamp a NaN value. A user's resized roster
/// would be overridden on every rebuild, or a hand-edited layout would panic instead of using auto.
#[test]
fn roster_resolved_preserves_finite_user_widths_and_skips_nan() {
    let auto = 460.0;
    let mut widths = HashMap::from([(roster_width::WIDTH_KEY.to_string(), 1_000.0)]);

    assert_eq!(
        roster_width::resolved(&widths, auto),
        roster_width::MAX_BASE_W,
        "a finite stored drag must win and still honour the upper drag boundary"
    );

    widths.insert(roster_width::WIDTH_KEY.to_string(), f32::NAN);
    assert_eq!(
        roster_width::resolved(&widths, auto),
        auto,
        "a NaN layout value must be ignored in favour of the measured roster width"
    );
}

/// `roster_width.rs::dragged` must convert pointer pixels back into persisted base-width units.
///
/// Mutation: drop the division by `scale`. At a raised Font slider the divider would trail the
/// cursor and persist the wrong width, so the wallet roster would jump after a restart.
#[test]
fn roster_drag_converts_rendered_pointer_delta_to_base_width() {
    let actual = roster_width::dragged(300.0, 100.0, 218.0, 13.0 / 11.0)
        .expect("finite pointer and positive scale must produce a base width");
    let expected = 300.0 + 118.0 * 11.0 / 13.0;

    assert!(
        (actual - expected).abs() < 0.01,
        "dragged base width must divide the rendered delta by scale: expected {expected}, got {actual}"
    );
    assert!(
        (actual - 418.0).abs() > 0.01,
        "dropping the scale division would persist the raw 118 px delta"
    );
    assert!(
        (actual - (300.0 + 118.0 * 13.0 / 11.0)).abs() > 0.01,
        "multiplying by scale would make the roster outpace the pointer"
    );
}
