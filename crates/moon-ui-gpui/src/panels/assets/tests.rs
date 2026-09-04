//! Regression tests for Assets state and presentation contracts.

use std::collections::{HashMap, HashSet};

use super::{
    reconcile_retained_assets_state, resolve_workspace_wallet_core, roster_width,
    wallets::wallet_count_label,
};

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

/// Extract one Rust function so source assertions cover its complete implementation branch.
///
/// Args:
///     source: Rust source containing the uniquely named function.
///     marker: Prefix identifying the function signature.
///
/// Returns:
///     The signature and balanced-brace body of the named function.
fn function_source<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source
        .find(marker)
        .expect("expected function marker in table source");
    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .expect("expected function body in table source");
    let mut depth = 0usize;

    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=body_start + offset];
                }
            }
            _ => {}
        }
    }

    panic!("expected balanced function body in table source");
}

/// Remove line and nested block comments before source-contract assertions inspect a function.
///
/// Args:
///     source: Rust source text whose double-quoted literals must remain available to the
///         assertions.
///
/// Returns:
///     The source without comments, while retaining line breaks and double-quoted literals.
fn strip_rust_comments(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();

    while let Some(character) = characters.next() {
        if character == '"' {
            stripped.push(character);
            let mut escaped = false;
            for string_character in characters.by_ref() {
                stripped.push(string_character);
                if escaped {
                    escaped = false;
                } else if string_character == '\\' {
                    escaped = true;
                } else if string_character == '"' {
                    break;
                }
            }
            continue;
        }

        if character == '/' && characters.next_if_eq(&'/').is_some() {
            for comment_character in characters.by_ref() {
                if comment_character == '\n' {
                    stripped.push('\n');
                    break;
                }
            }
            continue;
        }

        if character == '/' && characters.next_if_eq(&'*').is_some() {
            let mut depth = 1usize;
            while let Some(comment_character) = characters.next() {
                if comment_character == '/' && characters.next_if_eq(&'*').is_some() {
                    depth += 1;
                } else if comment_character == '*' && characters.next_if_eq(&'/').is_some() {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                } else if comment_character == '\n' {
                    stripped.push('\n');
                }
            }
            continue;
        }

        stripped.push(character);
    }

    stripped
}

/// Source assertions must not let a comment impersonate an unavailable-asset branch contract.
///
/// Mutation: skip comment stripping before compaction. User consequence: a stale comment could
/// keep a source contract green after the explanatory translation call was removed from Assets.
#[test]
fn source_contract_ignores_commented_translation_tokens() {
    let source = r#"
        fn actions_cell() {
            // t!("assets.actions.unavailable_qty")
            /* t!("assets.pnl.spot_unavailable") */
            MoonDataCell::empty()
        }
    "#;
    let compact: String = strip_rust_comments(source)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();

    assert!(!compact.contains("assets.actions.unavailable_qty"));
    assert!(!compact.contains("assets.pnl.spot_unavailable"));
    assert!(compact.contains("MoonDataCell::empty()"));
}

/// `wallets.rs:wallet_count_label` must expose dust-filtered rows as `shown / total`.
///
/// Mutation: return only `total_count.to_string()`. User consequence: an empty filtered Spot
/// wallet reads `Spot (833)` again instead of exposing that all 833 rows were filtered away.
#[test]
fn wallet_count_label_distinguishes_filtered_from_complete_wallets() {
    assert_eq!(wallet_count_label(0, 833), "0 / 833");
    assert_eq!(wallet_count_label(11, 833), "11 / 833");
    assert_eq!(wallet_count_label(11, 11), "11");
    assert_eq!(wallet_count_label(0, 0), "0");
}

/// `table.rs:actions_cell` and `table.rs:pnl_cell` must retain their explanatory dash branches.
///
/// Mutation A: reassign an unavailable-asset `t!` call to a different `actions_cell` branch.
/// Mutation B: remove `&& e.row.listed == 1` from `pnl_cell`'s Spot-only match guard. User
/// consequence: an intentionally unsellable holding can name the wrong cause, or a non-Spot PnL
/// dash can misleadingly claim the Spot-only explanation.
#[test]
fn unsellable_assets_explain_the_action_and_spot_pnl_dashes() {
    let table_source = include_str!("table.rs");
    let table_without_comments = strip_rust_comments(table_source);
    let actions: String = function_source(&table_without_comments, "fn actions_cell(")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let pnl: String = function_source(&table_without_comments, "fn pnl_cell(")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();

    assert!(actions.contains("if!sellable||e.row.market.is_empty()||!e.market_exists{"));
    assert!(actions.contains("MoonDataCell::element("));
    assert!(actions.contains("justify_end()"));
    assert!(actions.contains("crate::panels::common::text_tooltip("));
    assert!(actions.contains(
        "if!sellable{t!(\"assets.actions.unavailable_qty\",coin=e.row.coin.as_str()).to_string()}elseif"
    ));
    assert!(actions.contains(
        "e.row.market.is_empty(){t!(\"assets.actions.unavailable_market_identity\",coin=e.row.coin.as_str()).to_string()}else"
    ));
    assert!(actions.contains(
        "{t!(\"assets.actions.unavailable_market_catalog\",market=e.row.market.trim(),).to_string()}"
    ));

    let spot_guard = "Noneife.row.pos_size==0.0&&e.row.listed==1=>";
    let spot_start = pnl
        .find(spot_guard)
        .expect("expected the Spot-only PnL unavailable guard");
    let fallback = "None=>MoonDataCell::text(\"\u{2013}\").tone(MoonTone::Muted)";
    let spot_end = pnl[spot_start..]
        .find(fallback)
        .map(|offset| spot_start + offset)
        .expect("expected the undecorated PnL fallback after the Spot-only arm");
    let spot_arm = &pnl[spot_start..spot_end];

    assert!(spot_arm.contains("MoonDataCell::element("));
    assert!(spot_arm.contains("crate::panels::common::text_tooltip("));
    assert!(spot_arm.contains("t!(\"assets.pnl.spot_unavailable\").to_string()"));
    assert!(pnl.contains("None=>MoonDataCell::text(\"–\").tone(MoonTone::Muted)"));
}
