//! Regression tests for Report strategy-select synchronization inputs.

use moon_core::db::ReportStrategyKey;

use super::upsert_strategy_choice;

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
}
