//! Regression coverage for strategy-stat cache writes under a damaged report time axis.

use std::path::PathBuf;

use moon_core::config::paths;
use moon_core::db::report_recovery;
use moon_core::strat_db::stats::versions_with_stats;
use rusqlite::Connection;

/// Allocate one process-unique data root outside the working tree.
fn fixture_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "moonterminal-axis-cache-{}",
        std::process::id()
    ))
}

/// `strat_db/stats.rs::core_axis` -- flattening malformed `core_time_offset` rows into an empty
/// identity axis marks a failed offset read as trusted and writes an apparently fresh
/// `version_stats` entry, freezing a wrong strategy-version attribution until another unrelated
/// replica update arrives.
#[test]
fn malformed_offset_rows_compute_but_never_write_the_strategy_stats_cache() {
    let root = fixture_root();
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("remove stale axis-cache fixture");
    }
    std::fs::create_dir_all(&root).expect("create axis-cache fixture root");
    assert!(
        paths::set_data_dir_override(root),
        "this integration-test process must install its isolated data root before resolving paths"
    );

    let reports_path = paths::reports_db_path();
    let reports = Connection::open(&reports_path).expect("open reports fixture");
    reports
        .execute_batch(
            "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO app_meta VALUES ('axis_gen', '9');
             CREATE TABLE core_time_offset (
                 core_uid INTEGER NOT NULL, from_utc INTEGER NOT NULL, offset_secs TEXT NOT NULL
             );
             INSERT INTO core_time_offset VALUES (7, 0, 'malformed-offset');
             CREATE TABLE orders_rep (
                 core_uid INTEGER NOT NULL, strategyid INTEGER NOT NULL, buydate INTEGER NOT NULL,
                 closedate INTEGER, profitbtc REAL NOT NULL, last_update_at INTEGER NOT NULL
             );
             INSERT INTO orders_rep VALUES (7, 44, 100, 120, 7.5, 500);",
        )
        .expect("seed malformed offset replica");
    drop(reports);

    let _permit = report_recovery::prepare().expect("authorize the isolated reports fixture");
    let strategies_path = paths::strategies_db_path();
    let strategies = Connection::open(&strategies_path).expect("open strategies fixture");
    strategies
        .execute_batch(
            "CREATE TABLE strategy_versions (
                 core_uid INTEGER NOT NULL, strategy_id INTEGER NOT NULL, valid_from INTEGER NOT NULL,
                 valid_to INTEGER, change_kind TEXT NOT NULL, origin TEXT, n_changed INTEGER NOT NULL
             );
             INSERT INTO strategy_versions VALUES (7, 44, 0, 200000, 'edit', NULL, 1);",
        )
        .expect("seed stale strategy version");
    drop(strategies);

    let versions = versions_with_stats(7, 44);
    assert_eq!(versions.len(), 1, "the attached replica must still compute visible statistics");
    assert_eq!(versions[0].trades, 1);
    assert_eq!(versions[0].profit, 7.5);

    let strategies = Connection::open(&strategies_path).expect("reopen strategies fixture");
    let cache_rows: i64 = strategies
        .query_row(
            "SELECT COUNT(*) FROM version_stats WHERE core_uid=7 AND strategy_id=44",
            [],
            |row| row.get(0),
        )
        .expect("count strategy-stat cache rows");
    assert_eq!(
        cache_rows, 0,
        "a malformed present offset table may show the calculation but must never cache it as fresh"
    );
}
