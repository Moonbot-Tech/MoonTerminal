//! Regression tests for grouped Report multi-strategy choices.

use moon_core::db::{ReportStrategy, ReportStrategyKey};
use moon_ui::{
    MoonSearchableListChange, MoonSearchableListDelegate as _, MoonSearchableListItem as _,
};

use super::*;

/// Build one exact strategy fixture.
///
/// Args:
///     core_uid: Runtime core identity.
///     strategy_id: Signed Moonbot strategy id.
///     name: Display strategy name.
///
/// Returns:
///     One Report strategy metadata row.
fn strategy(core_uid: u64, strategy_id: i64, name: &str) -> ReportStrategy {
    ReportStrategy {
        key: ReportStrategyKey {
            core_uid,
            strategy_id,
        },
        name: name.to_string(),
    }
}

/// Removing the core name from exact-item search text must fail this assertion; otherwise a
/// core-name query hides some children and a core-wide toggle no longer represents the section.
///
/// Returns:
///     Nothing; exact and core-wide search behavior is asserted.
#[test]
fn every_exact_item_matches_its_core_name() {
    let groups = strategy_groups(
        &[strategy(42, -7, "BREAKOUT")],
        &[(42, "VLTR$18 ~ F-BN".to_string())],
        "All strategies",
        "Manual orders",
    );

    assert_eq!(groups.len(), 2);
    assert_eq!(groups[1].items.len(), 2);
    assert!(
        groups[1].items.iter().all(|item| item.matches("VLTR$18")),
        "a core-name search must retain its core-wide and exact-strategy rows"
    );
}

/// Applying default single-row changes to a Core action must fail here: it would store the action
/// row instead of all exact children, so selecting strategies from several cores would not reach
/// the Report query.
///
/// Returns:
///     Nothing; multi-core batching and last-deselect behavior are asserted.
#[test]
fn core_rows_batch_toggle_exact_members_across_cores() {
    let strategies = vec![
        strategy(1, -7, "A"),
        strategy(1, -8, "B"),
        strategy(2, -7, "C"),
    ];
    let available = strategies.iter().map(|item| item.key).collect();
    let groups = strategy_groups(
        &strategies,
        &[(1, "CORE-A".to_string()), (2, "CORE-B".to_string())],
        "All strategies",
        "Manual orders",
    );
    let catalog = ReportStrategyDelegate::catalog(groups, available);
    let mut delegate =
        ReportStrategyDelegate::new(catalog, None, ReportStrategyDelegate::search_state());
    let mut selection = Vec::new();

    delegate.on_will_change(
        &mut selection,
        &[MoonSearchableListChange::Select {
            index: MoonComponentIndexPath::new(0).section(1),
        }],
    );
    assert_eq!(
        delegate.selected,
        HashSet::from([strategies[0].key, strategies[1].key])
    );

    delegate.on_will_change(
        &mut selection,
        &[MoonSearchableListChange::Select {
            index: MoonComponentIndexPath::new(0).section(2),
        }],
    );
    assert_eq!(
        delegate.selected,
        strategies.iter().map(|item| item.key).collect()
    );

    delegate.on_will_change(
        &mut selection,
        &[MoonSearchableListChange::Deselect {
            index: MoonComponentIndexPath::new(0).section(1),
        }],
    );
    assert_eq!(delegate.selected, HashSet::from([strategies[2].key]));

    delegate.on_will_change(
        &mut selection,
        &[MoonSearchableListChange::Deselect {
            index: MoonComponentIndexPath::new(1).section(2),
        }],
    );
    let values = selection
        .iter()
        .map(|(_, item)| item.choice)
        .collect::<Vec<_>>();
    assert!(
        exact_strategy_selection(&values).is_none(),
        "removing the last exact checkbox must restore implicit All like the core selector"
    );
}

/// Removing the `selection.clear()` branch from `ReportStrategyDelegate:on_will_change` must leave
/// the exact checkbox selected and fail this assertion, making the global All row ineffective.
///
/// Returns:
///     Nothing; the explicit All action is asserted to restore implicit All.
#[test]
fn global_all_clears_a_nonempty_multi_selection() {
    let strategies = vec![strategy(1, -7, "A"), strategy(1, -8, "B")];
    let groups = strategy_groups(
        &strategies,
        &[(1, "CORE-A".to_string())],
        "All strategies",
        "Manual orders",
    );
    let catalog =
        ReportStrategyDelegate::catalog(groups, strategies.iter().map(|item| item.key).collect());
    let mut delegate =
        ReportStrategyDelegate::new(catalog, None, ReportStrategyDelegate::search_state());
    let mut selection = Vec::new();
    delegate.on_will_change(
        &mut selection,
        &[MoonSearchableListChange::Select {
            index: MoonComponentIndexPath::new(1).section(1),
        }],
    );
    assert_eq!(delegate.selected, HashSet::from([strategies[0].key]));

    delegate.on_will_change(
        &mut selection,
        &[MoonSearchableListChange::Select {
            index: MoonComponentIndexPath::new(0).section(0),
        }],
    );

    assert!(selection.is_empty());
    assert!(delegate.selected.is_empty());
}

/// Treating every exact row as enabled must fail after the stale selection is removed; otherwise
/// a deleted strategy can be immediately reselected before metadata confirms it again.
///
/// Returns:
///     Nothing; a selected stale row stays removable and then becomes disabled.
#[test]
fn stale_exact_row_disables_immediately_after_deselection() {
    let stale = strategy(1, -7, "DELETED");
    let groups = strategy_groups(
        std::slice::from_ref(&stale),
        &[(1, "CORE-A".to_string())],
        "All strategies",
        "Manual orders",
    );
    let selected = HashSet::from([stale.key]);
    let catalog = ReportStrategyDelegate::catalog(groups, HashSet::new());
    let mut delegate = ReportStrategyDelegate::new(
        catalog,
        Some(&selected),
        ReportStrategyDelegate::search_state(),
    );
    let stale_index = MoonComponentIndexPath::new(1).section(1);
    let mut selection = vec![(
        stale_index,
        delegate
            .catalog_item(stale_index)
            .expect("stale catalog row")
            .clone(),
    )];
    assert!(delegate.choice_is_enabled(ReportStrategyChoice::Exact(stale.key)));

    delegate.on_will_change(
        &mut selection,
        &[MoonSearchableListChange::Deselect { index: stale_index }],
    );

    assert!(selection.is_empty());
    assert!(!delegate.choice_is_enabled(ReportStrategyChoice::Exact(stale.key)));
}

/// Matching only the strategy id must fail this assertion and select the same id from another
/// core; storing Core/All action rows as selections must also add unexpected indices.
///
/// Returns:
///     Nothing; full core-plus-strategy identity is asserted.
#[test]
fn grouped_indices_preserve_exact_identity_only() {
    let strategies = vec![strategy(1, -7, "SAME-ID-A"), strategy(2, -7, "SAME-ID-B")];
    let groups = strategy_groups(
        &strategies,
        &[(1, "CORE-A".to_string()), (2, "CORE-B".to_string())],
        "All strategies",
        "Manual orders",
    );

    let indices = strategy_choice_indices(&groups, Some(&HashSet::from([strategies[1].key])));
    assert_eq!(indices.len(), 1);
    assert_eq!((indices[0].section, indices[0].row), (2, 1));
}

/// Replacing exact non-empty summary equality with subset containment must make stale-plus-complete
/// read as All; converting an explicit complete set to `None` must also fail because the current
/// query could then admit newly discovered or legacy rows absent from its checkbox snapshot.
///
/// Returns:
///     Nothing; presentation equality and exact query normalization are asserted independently.
#[test]
fn all_summary_requires_equality_while_every_explicit_filter_stays_exact() {
    let first = ReportStrategyKey {
        core_uid: 1,
        strategy_id: -1,
    };
    let second = ReportStrategyKey {
        core_uid: 2,
        strategy_id: -2,
    };
    let stale = ReportStrategyKey {
        core_uid: 9,
        strategy_id: -9,
    };
    let available = HashSet::from([first, second]);

    let (label, all_on) = strategy_selection_summary(&available, None, "All strategies", |n| {
        format!("Strategies: {n}")
    });
    assert_eq!((label.as_str(), all_on), ("All strategies", true));
    assert_eq!(normalized_strategy_filter_keys(None), None);

    let complete = HashSet::from([first, second]);
    assert_eq!(
        strategy_selection_summary(&available, Some(&complete), "All strategies", |n| {
            format!("Strategies: {n}")
        }),
        ("All strategies".to_string(), true)
    );
    assert_eq!(
        normalized_strategy_filter_keys(Some(&complete)),
        Some(vec![first, second])
    );

    let with_stale = HashSet::from([first, second, stale]);
    assert_eq!(
        strategy_selection_summary(&available, Some(&with_stale), "All strategies", |n| {
            format!("Strategies: {n}")
        }),
        ("Strategies: 3".to_string(), false)
    );
    assert_eq!(
        normalized_strategy_filter_keys(Some(&with_stale))
            .expect("stale selection remains explicit")
            .len(),
        3
    );

    let empty = HashSet::new();
    assert_eq!(
        normalized_strategy_filter_keys(Some(&empty)),
        Some(Vec::new())
    );
}

/// Dropping the selected-key merge loop must lose both stale rows, while adding merged rows to the
/// available set must incorrectly normalize a stale-only selection to All.
///
/// Returns:
///     Nothing; stale display retention and current availability remain separate.
#[test]
fn metadata_refresh_preserves_all_stale_labels_without_marking_them_available() {
    let previous = vec![strategy(1, -1, "OLD-A"), strategy(2, -2, "OLD-B")];
    let selected = previous.iter().map(|item| item.key).collect::<HashSet<_>>();
    let refreshed = vec![strategy(3, -3, "NEW-C")];

    let (display, available) =
        merge_strategy_metadata(&previous, refreshed.clone(), Some(&selected));
    assert_eq!(available, HashSet::from([refreshed[0].key]));
    assert_eq!(
        display
            .iter()
            .map(|item| (item.key, item.name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (refreshed[0].key, "NEW-C"),
            (previous[0].key, "OLD-A"),
            (previous[1].key, "OLD-B"),
        ]
    );
    assert!(!strategy_selection_is_all(&available, Some(&selected)));
}

/// Replacing a delegate with an unfiltered initial snapshot must fail this assertion and show every
/// core under retained search text until the user types again.
///
/// Returns:
///     Nothing; replacement visibility is asserted against the retained query.
#[test]
fn replacement_delegate_applies_retained_group_search() {
    let search = ReportStrategyDelegate::search_state();
    search.replace("CORE-B".to_string());
    let strategies = vec![strategy(1, -1, "A"), strategy(2, -2, "B")];
    let groups = strategy_groups(
        &strategies,
        &[(1, "CORE-A".to_string()), (2, "CORE-B".to_string())],
        "All strategies",
        "Manual orders",
    );
    let catalog =
        ReportStrategyDelegate::catalog(groups, strategies.iter().map(|item| item.key).collect());
    let delegate = ReportStrategyDelegate::new(catalog, None, search);

    assert_eq!(delegate.matched_groups.len(), 1);
    let matched = &delegate.matched_groups[0];
    assert_eq!(
        delegate.catalog.groups[matched.source_section]
            .title
            .as_ref()
            .map(SharedString::as_ref),
        Some("CORE-B")
    );
    assert_eq!(delegate.items_count(0), 2);
}
