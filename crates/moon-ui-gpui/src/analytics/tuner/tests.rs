//! Regression coverage for tuner mode persistence and workspace-constrained retained selection.

use super::{STRAT_MODES, StratMode, strategy_selection_visible};
use moon_core::config::layout::StratColsByMode;

/// Each axis must address its OWN slot. Two axes sharing one is the copy-paste that makes
/// the whole per-axis layout pointless — and it would look like "my columns keep changing
/// when I switch tabs", which is exactly what this feature exists to stop.
#[test]
fn each_axis_owns_its_column_slot() {
    let mut cols = StratColsByMode::default();
    for (i, mode) in STRAT_MODES.into_iter().enumerate() {
        *mode.cols_slot(&mut cols) = i as u16 + 1;
    }
    assert_eq!((cols.filter, cols.coins, cols.time), (1, 2, 3));
    // And reading back returns what that axis wrote, not a neighbour's.
    for (i, mode) in STRAT_MODES.into_iter().enumerate() {
        assert_eq!(*mode.cols_slot(&mut cols), i as u16 + 1);
    }
}

/// Only the coin axis spends width on the coin-list columns — that difference is the
/// reason the mask is per axis at all.
#[test]
fn coin_axis_defaults_to_showing_the_lists() {
    assert_ne!(
        StratMode::Coins.default_cols(),
        StratMode::Filters.default_cols()
    );
    assert_eq!(
        StratMode::Filters.default_cols(),
        StratMode::Time.default_cols()
    );
}

/// Removing effective filtering from `tuner/mod.rs:selected_targets` would keep a stale owner
/// strategy writable after the singleton Auto workspace moves to another core.
#[test]
fn stale_owner_strategy_selection_is_hidden_without_erasing_classic_state() {
    let retained = "41@11";
    assert!(!strategy_selection_visible(retained, Some(&[22])));
    assert!(strategy_selection_visible(retained, Some(&[11])));
    assert!(strategy_selection_visible(retained, None));

    let tuner = include_str!("mod.rs");
    let targets = tuner
        .split("fn selected_targets(&self)")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("selected_targets must exist");
    assert!(targets.contains("strategy_selection_visible(key, workspace)"));

    let save = include_str!("save.rs");
    assert!(save.contains("if !self.save_target_in_workspace(target)"));
    assert!(save.contains("if !this.save_authority_is_current(&authority, &targets, cx)"));
    assert!(save.contains("resolve_complete_target_cores(&targets, &live)"));
}

/// Tuner report and strategy navigation must derive the current singleton owner at dispatch.
///
/// Mutation: omit the owner lookup or its argument to `open_goto`. A row retained after a rail
/// switch could reveal a strategy or report from the previously selected core.
#[test]
fn tuner_navigation_revalidates_singleton_workspace_authority() {
    let source = include_str!("mod.rs");

    assert!(source.matches(".singleton_workspace()").count() >= 2);
    assert!(source.contains(".workspace_action_allows_core(workspace_group.as_deref(), core_uid)"));
    assert!(source.contains("strategy_id,\n            workspace_group,"));
}
