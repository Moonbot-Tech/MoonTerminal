//! Regression tests for Report strategy-select synchronization inputs.

use moon_core::db::ReportStrategyKey;

use super::{ReportPreferenceRevisions, metadata_apply_plan, upsert_strategy_choice};

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

/// Moving `db::open_reader` back into `ReportPanel::new_with_scope` must fail this contract: mode
/// switches that construct a temporary Report would again wait for SQLite open/schema metadata on
/// the GPUI application thread.
#[test]
fn report_construction_defers_sqlite_metadata_to_the_background_executor() {
    let source = include_str!("../state.rs");
    let constructor = source
        .split("pub(crate) fn new_with_scope")
        .nth(1)
        .and_then(|tail| tail.split("pub(crate) fn replace_scope").next())
        .expect("Report construction must remain a bounded source block");
    assert!(!constructor.contains("db::open_reader"));
    let loader = source
        .split("fn load_initial_metadata")
        .nth(1)
        .and_then(|tail| tail.split("/// Create a regular").next())
        .expect("Report must retain its deferred metadata loader");
    assert!(loader.contains("background_executor"));
    assert!(loader.contains("ReportInitialMetadata::load"));
}

/// Replacing any `schedule_report_preference` call in the Report preference handlers with a direct
/// `db::save_*` call must fail this contract: opening or writing SQLite from a click handler would
/// restore the multi-second UI stall reported for Report controls.
#[test]
fn report_preference_io_stays_off_the_gpui_thread() {
    let actions = include_str!("../actions.rs");
    let render = include_str!("../render.rs");
    let state = include_str!("../state.rs");

    let toggle = actions
        .split("fn toggle_comment_pane")
        .nth(1)
        .and_then(|tail| tail.split("fn set_deleted_only").next())
        .expect("comment preference handler must remain discoverable");
    let sort = render
        .split("fn set_report_sort")
        .nth(1)
        .and_then(|tail| tail.split("impl EventEmitter").next())
        .expect("sort preference handler must remain discoverable");
    let detached = state
        .split("fn mark_table_detached")
        .nth(1)
        .and_then(|tail| tail.split("fn mark_standalone").next())
        .expect("detached-host transition must remain discoverable");
    let visible = state
        .split("fn persist_visible")
        .nth(1)
        .and_then(|tail| tail.split("\n    }\n}").next())
        .expect("visible-column persistence must remain discoverable");

    for (name, handler) in [("comment", toggle), ("sort", sort), ("visible", visible)] {
        assert!(
            handler.contains("schedule_report_preference"),
            "{name} preference writes must use the background writer"
        );
        assert!(
            !handler.contains("db::open_reader"),
            "{name} handler must not open SQLite on GPUI"
        );
    }
    assert!(detached.contains("reload_comment_preference"));
    assert!(!detached.contains("db::load_comment_pane"));
}

/// Replacing revision comparison in `metadata_apply_plan` with value-to-default comparison must
/// fail this assertion: a user who changes a preference and then returns to its initial value would
/// otherwise be overwritten by the late initial SQLite read.
#[test]
fn late_report_metadata_cannot_overwrite_a_touched_preference() {
    let expected = ReportPreferenceRevisions::default();
    let current = ReportPreferenceRevisions {
        visible: 2,
        sort: 4,
        comment: 6,
    };

    assert_eq!(
        metadata_apply_plan(current, expected, true),
        (false, false, false)
    );
    assert_eq!(
        metadata_apply_plan(expected, expected, false),
        (false, true, true),
        "a context-specific visible-column set blocks only the app_meta visible seed"
    );
}

/// Removing the shared lock or the post-lock latest-sequence check from
/// `schedule_report_preference` must fail this contract: two fast Report changes could then finish
/// out of order and persist the older choice after the newer one.
#[test]
fn report_preference_writer_serializes_and_rejects_stale_requests() {
    let source = include_str!("../state.rs");
    let writer = source
        .split("fn schedule_report_preference")
        .nth(1)
        .and_then(|tail| tail.split("fn metadata_apply_plan").next())
        .expect("Report preference writer must remain discoverable");

    for anchor in [
        "latest.store(request",
        "REPORT_PREFERENCE_WRITE_LOCK",
        "latest.load(Ordering::Acquire) != request",
        "db::open_reader()",
    ] {
        assert!(
            writer.contains(anchor),
            "serialized preference writer is missing required anchor: {anchor}"
        );
    }
    let publish = writer.find("latest.store(request").unwrap();
    let lock = writer.find("REPORT_PREFERENCE_WRITE_LOCK").unwrap();
    let reject = writer
        .find("latest.load(Ordering::Acquire) != request")
        .unwrap();
    let open = writer.find("db::open_reader()").unwrap();
    assert!(publish < lock && lock < reject && reject < open);
}
