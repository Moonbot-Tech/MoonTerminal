//! Regression tests for grouped Report strategy choices.

use moon_core::db::{ReportStrategy, ReportStrategyKey};
use moon_ui::MoonSearchableListItem as _;

use super::*;

/// Removing the core name from `strategy_item` search text must fail this assertion; otherwise a
/// core-name query leaves only zero matching child rows and hides the complete section.
///
/// Returns:
///     Nothing; every row's core-aware search behavior is asserted.
#[test]
fn every_item_matches_its_core_name() {
    let groups = strategy_groups(
        &[ReportStrategy {
            key: ReportStrategyKey {
                core_uid: 42,
                strategy_id: -7,
            },
            name: "BREAKOUT".to_string(),
        }],
        &[(42, "VLTR$18 ~ F-BN".to_string())],
        "All strategies",
        "Manual orders",
    );

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].items.len(), 2);
    assert!(
        groups[0].items.iter().all(|item| item.matches("VLTR$18")),
        "a core-name search must retain its core-wide and exact-strategy rows"
    );
}

/// Mapping a core-wide choice to an exact strategy, or clearing only the exact key on `None`, must
/// fail here; either edit makes the Report query disagree with the row selected in the combobox.
///
/// Returns:
///     Nothing; core-wide, exact, and global filter mappings are asserted.
#[test]
fn choices_map_to_independent_core_wide_exact_and_global_filters() {
    let exact = ReportStrategyKey {
        core_uid: 9,
        strategy_id: -3,
    };

    let (cores, strategy) =
        filters_for_strategy_choice(Some(ReportStrategyChoice::Core(exact.core_uid)));
    assert_eq!(cores, HashSet::from([9]));
    assert_eq!(strategy, None);

    let (cores, strategy) = filters_for_strategy_choice(Some(ReportStrategyChoice::Exact(exact)));
    assert_eq!(cores, HashSet::from([9]));
    assert_eq!(strategy, Some(exact));

    let (cores, strategy) = filters_for_strategy_choice(None);
    assert!(cores.is_empty());
    assert_eq!(strategy, None);
}

/// Matching only the strategy id must fail this assertion and select the same id from another
/// core, while treating the core-wide row as a placeholder must lose its selectable index.
///
/// Returns:
///     Nothing; exact identity and selectable core-wide row indices are asserted.
#[test]
fn grouped_indices_preserve_exact_identity_and_core_wide_rows() {
    let strategies = vec![
        ReportStrategy {
            key: ReportStrategyKey {
                core_uid: 1,
                strategy_id: -7,
            },
            name: "SAME-ID-A".to_string(),
        },
        ReportStrategy {
            key: ReportStrategyKey {
                core_uid: 2,
                strategy_id: -7,
            },
            name: "SAME-ID-B".to_string(),
        },
    ];
    let groups = strategy_groups(
        &strategies,
        &[(1, "CORE-A".to_string()), (2, "CORE-B".to_string())],
        "All strategies",
        "Manual orders",
    );

    let exact = strategy_choice_index(
        &groups,
        Some(ReportStrategyChoice::Exact(strategies[1].key)),
    )
    .expect("the exact strategy must be selectable");
    assert_eq!((exact.section, exact.row), (1, 1));

    let core = strategy_choice_index(&groups, Some(ReportStrategyChoice::Core(2)))
        .expect("the core-wide row must be selectable");
    assert_eq!((core.section, core.row), (1, 0));
}

/// Constructing a replacement delegate from an unfiltered snapshot must fail this assertion and
/// show every core under retained search text until the user types again.
///
/// Returns:
///     Nothing; the first replacement view is asserted against the retained query.
#[test]
fn replacement_delegate_applies_retained_group_search() {
    let search = ReportStrategyDelegate::search_state();
    search.replace("CORE-B".to_string());
    let groups = strategy_groups(
        &[
            ReportStrategy {
                key: ReportStrategyKey {
                    core_uid: 1,
                    strategy_id: -1,
                },
                name: "A".to_string(),
            },
            ReportStrategy {
                key: ReportStrategyKey {
                    core_uid: 2,
                    strategy_id: -2,
                },
                name: "B".to_string(),
            },
        ],
        &[(1, "CORE-A".to_string()), (2, "CORE-B".to_string())],
        "All strategies",
        "Manual orders",
    );

    let delegate = ReportStrategyDelegate::new(groups, search);

    assert_eq!(delegate.matched_groups.len(), 1);
    assert_eq!(delegate.matched_groups[0].title.as_ref(), "CORE-B");
    assert_eq!(delegate.matched_groups[0].items.len(), 2);
}
