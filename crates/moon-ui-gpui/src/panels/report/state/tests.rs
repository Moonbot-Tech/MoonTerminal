//! Regression tests for Report strategy-select synchronization inputs.

use moon_core::db::{ReportStrategy, ReportStrategyKey};

use super::{strategy_selected_index, upsert_strategy_choice};

/// Matching by strategy id alone must select row zero instead of row one and expose another core's
/// trades; treating All as row zero must also fail the `None` assertion.
///
/// Returns:
///     Nothing; exact core/signed-id selection and the All route are asserted.
#[test]
fn strategy_selection_index_preserves_exact_identity_and_all() {
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

    assert_eq!(
        strategy_selected_index(
            &strategies,
            Some(ReportStrategyKey {
                core_uid: 2,
                strategy_id: -7,
            }),
        )
        .map(|index| index.row),
        Some(1)
    );
    assert_eq!(strategy_selected_index(&strategies, None), None);
}

/// Removing scoped insertion must keep the replacement key absent until the minute metadata
/// refresh, while ignoring the update arm must leave repeated Analytics opens mislabeled.
///
/// Returns:
///     Nothing; a scoped key is inserted once, refreshed, and remains selectable.
#[test]
fn scoped_retarget_upserts_the_exact_choice() {
    let key = ReportStrategyKey {
        core_uid: 55,
        strategy_id: -999,
    };
    let mut strategies = Vec::new();

    upsert_strategy_choice(&mut strategies, key, "TARGET".to_string());
    upsert_strategy_choice(&mut strategies, key, "RENAMED".to_string());

    assert_eq!(strategies.len(), 1);
    assert_eq!(strategies[0].key, key);
    assert_eq!(strategies[0].name, "RENAMED");
    assert_eq!(
        strategy_selected_index(&strategies, Some(key)).map(|index| index.row),
        Some(0)
    );
}
