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

/// The per-context migration must repair exactly the sets that predate the new column: the ones
/// belonging to this table, that a user actually saved, and that do not already carry it.
///
/// Breakage: dropping the `keys.is_empty()` guard, which turns a set nobody chose into a
/// one-column table. Breakage: dropping the prefix test, which appends a Report column to every
/// other table's saved layout — the Assets, Orders and tuner tables among them. Breakage: appending
/// unconditionally, which duplicates the column on a set that already has it and reports a change
/// that did not happen, so the layout is rewritten on every launch.
#[test]
fn the_column_migration_repairs_only_the_sets_that_predate_it() {
    let mut sets = std::collections::HashMap::from([
        (
            "report-table-v2:dock".to_string(),
            vec!["coin".to_string(), "profitbtc".to_string()],
        ),
        (
            "report-table-v2:win".to_string(),
            vec![
                "coin".to_string(),
                moon_core::db::VALUATION_PROFIT_COLUMN.to_string(),
            ],
        ),
        ("report-table-v2:empty".to_string(), Vec::new()),
        ("orders-table:dock".to_string(), vec!["coin".to_string()]),
    ]);

    super::migrate_visible_sets(&mut sets, "report-table-v2:");

    assert_eq!(
        sets["report-table-v2:dock"],
        vec!["coin", "profitbtc", moon_core::db::VALUATION_PROFIT_COLUMN],
        "a set saved before the column existed gains it, at the end"
    );
    assert_eq!(
        sets["report-table-v2:win"].len(),
        2,
        "a set that already carries the column is left exactly as it was"
    );
    assert!(
        sets["report-table-v2:empty"].is_empty(),
        "an empty set is not a user choice and must not become a one-column table"
    );
    assert_eq!(
        sets["orders-table:dock"],
        vec!["coin"],
        "another table's saved layout is none of this migration's business"
    );

    let settled = sets.clone();
    super::migrate_visible_sets(&mut sets, "report-table-v2:");
    assert_eq!(
        sets, settled,
        "a second pass must change nothing, so a relaunch cannot duplicate the column"
    );
}
