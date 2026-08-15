//! Regression tests for Report strategy-catalog refresh scope.

use moon_core::db::{self, ReportFilter, ReportStrategyKey, SideFilter};
use rusqlite::Connection;

use super::{
    GENERATION_QUERY_INTERVAL, GenerationRefreshGate, GenerationRefreshPlan, MAX_REPORT_ROWS,
    report_query_result_is_current, strategy_catalog_scope, strategy_metadata_request,
};

/// `query.rs:GenerationRefreshGate::observe` must retain one pending generation until the timer
/// makes it due and an active render consumes it; setting `due` immediately bypasses the Report
/// throttle, while clearing it in the timer loses the final hidden-panel catch-up.
#[test]
fn generation_refresh_waits_and_stays_due_until_render() {
    let mut gate = GenerationRefreshGate::default();

    let timer_token = match gate.observe(std::time::Duration::from_secs(2)) {
        GenerationRefreshPlan::NotifyAfter { wait, timer_token } => {
            assert_eq!(wait, std::time::Duration::from_secs(3));
            timer_token
        }
        plan => panic!("expected a delayed notification, got {plan:?}"),
    };
    assert_eq!(
        gate.observe(std::time::Duration::from_secs(3)),
        GenerationRefreshPlan::Idle
    );
    assert!(!gate.take_due());
    assert!(gate.timer_fired(timer_token));
    assert!(!gate.timer_fired(timer_token));
    assert!(gate.take_due());
    assert!(!gate.take_due());
}

/// `query.rs:GenerationRefreshGate::query_started` must cancel covered pending work without
/// reviving it when an already-armed timer wakes; omitting the pending reset runs a duplicate
/// five-second Report query after a manual refresh already covered that generation.
#[test]
fn query_start_cancels_pending_work_and_its_stale_timer() {
    let mut gate = GenerationRefreshGate::default();

    let timer_token = match gate.observe(std::time::Duration::from_secs(1)) {
        GenerationRefreshPlan::NotifyAfter { timer_token, .. } => timer_token,
        plan => panic!("expected a delayed notification, got {plan:?}"),
    };
    gate.query_started();
    assert!(!gate.timer_fired(timer_token));
    assert!(!gate.take_due());
}

/// `query.rs:GenerationRefreshGate::query_started` must invalidate an armed timer token; removing
/// that invalidation lets the old wake release a generation observed after a manual query, causing
/// a Report refresh less than five seconds after the latest query start.
#[test]
fn stale_timer_cannot_release_a_new_generation_early() {
    let mut gate = GenerationRefreshGate::default();

    let old_timer_token = match gate.observe(std::time::Duration::from_secs(1)) {
        GenerationRefreshPlan::NotifyAfter { timer_token, .. } => timer_token,
        plan => panic!("expected the first delayed notification, got {plan:?}"),
    };
    gate.query_started();
    let new_timer_token = match gate.observe(std::time::Duration::from_secs(1)) {
        GenerationRefreshPlan::NotifyAfter { timer_token, .. } => timer_token,
        plan => panic!("expected the replacement delayed notification, got {plan:?}"),
    };

    assert!(!gate.timer_fired(old_timer_token));
    assert!(!gate.take_due());
    assert!(gate.timer_fired(new_timer_token));
    assert!(gate.take_due());
}

/// `query.rs:GenerationRefreshGate::observe` must release an already-throttled generation without
/// arming another timer; changing `>=` to `>` delays the exact five-second boundary and leaves a
/// visible Report stale until another commit arrives.
#[test]
fn elapsed_generation_is_due_without_another_timer() {
    let mut gate = GenerationRefreshGate::default();

    assert_eq!(
        gate.observe(GENERATION_QUERY_INTERVAL),
        GenerationRefreshPlan::NotifyNow
    );
    assert!(gate.take_due());
}

/// Build one fully populated filter for scope-comparison tests.
///
/// Returns:
///     A filter with every non-strategy predicate represented.
fn populated_filter() -> ReportFilter {
    ReportFilter {
        core_uids: vec![7, 3],
        date_from: Some(100),
        date_to: Some(200),
        coin: " BTC ".to_string(),
        exact_coins: None,
        side: SideFilter::Long,
        emulator: Some(false),
        deleted_only: false,
        closed_only: true,
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 7,
            strategy_id: -11,
        }]),
        strategy_name_mask: "EMA_".to_string(),
        valuation: Default::default(),
    }
}

/// Removing core sorting/deduplication or SQL-equivalent coin normalization from
/// `query:strategy_catalog_scope` must make the equivalent filters differ and request an
/// unnecessary distinct-strategy scan. Retaining `strategies` must do the same after a checkbox
/// change.
///
/// Returns:
///     Nothing; canonical scope equality and the no-refresh decision are asserted.
#[test]
fn equivalent_catalog_scopes_do_not_refresh() {
    let first = populated_filter();
    let mut equivalent = first.clone();
    equivalent.core_uids = vec![3, 7, 3];
    equivalent.coin = "btc".to_string();
    equivalent.strategies = Some(vec![ReportStrategyKey {
        core_uid: 3,
        strategy_id: 99,
    }]);
    equivalent.strategy_name_mask = "different strategy predicate".to_string();

    let published = strategy_catalog_scope(&first);
    assert_eq!(published.core_uids, vec![3, 7]);
    assert_eq!(published.coin, "BTC");
    assert_eq!(published.strategies, None);
    assert_eq!(published.strategy_name_mask, "");
    assert_eq!(strategy_catalog_scope(&equivalent), published);
    assert_eq!(
        strategy_metadata_request(&equivalent, Some(&published), false),
        None
    );
}

/// Omitting any non-strategy field from `query:strategy_catalog_scope` must leave the matching
/// mutation equal to the published scope and skip the catalog refresh, exposing stale choices.
/// Returning `None` when the interval elapsed must also delay newly arriving strategies forever.
///
/// Returns:
///     Nothing; every catalog predicate and the periodic fallback force a request.
#[test]
fn every_catalog_predicate_and_periodic_tick_refreshes() {
    let filter = populated_filter();
    let published = strategy_catalog_scope(&filter);
    let mut changed = Vec::new();

    let mut core = filter.clone();
    core.core_uids.push(9);
    changed.push(core);
    let mut date_from = filter.clone();
    date_from.date_from = Some(101);
    changed.push(date_from);
    let mut date_to = filter.clone();
    date_to.date_to = Some(201);
    changed.push(date_to);
    let mut coin = filter.clone();
    coin.coin = "ETH".to_string();
    changed.push(coin);
    let mut side = filter.clone();
    side.side = SideFilter::Short;
    changed.push(side);
    let mut emulator = filter.clone();
    emulator.emulator = Some(true);
    changed.push(emulator);
    let mut deleted = filter.clone();
    deleted.deleted_only = true;
    changed.push(deleted);
    let mut closed = filter.clone();
    closed.closed_only = false;
    changed.push(closed);

    for candidate in changed {
        assert!(
            strategy_metadata_request(&candidate, Some(&published), false).is_some(),
            "every non-strategy Report predicate must invalidate the strategy catalog"
        );
    }
    assert!(strategy_metadata_request(&filter, Some(&published), true).is_some());
}

/// The Report query window includes rows 101 through 500 while retaining a deterministic top-N.
///
/// Breakage this pins: restoring `query::MAX_REPORT_ROWS` to 100. The panel would again report only
/// 100 visible orders even though the shared Analytics/Report limit is 500.
#[test]
fn report_query_window_contains_five_hundred_rows() {
    let conn = Connection::open_in_memory().expect("open Report fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL,
             core_name TEXT NOT NULL,
             newrecid INTEGER NOT NULL,
             closedate INTEGER,
             profitbtc REAL,
             PRIMARY KEY (core_uid, newrecid)
         );
         WITH RECURSIVE rows(n) AS (
             VALUES(1) UNION ALL SELECT n + 1 FROM rows WHERE n < 505
         )
         INSERT INTO orders_rep(core_uid, core_name, newrecid, closedate, profitbtc)
         SELECT 1, 'CORE-A', n, n, 1.0 FROM rows;",
    )
    .expect("seed 505 reports");
    let table = db::query_reports(
        &conn,
        &ReportFilter::default(),
        "closedate",
        true,
        MAX_REPORT_ROWS,
    )
    .expect("query Report window");

    assert_eq!(table.rows.len(), 500);
    assert_eq!(
        db::query_totals(&conn, &ReportFilter::default())
            .unwrap()
            .orders,
        505
    );
}

/// `query.rs:schedule_requery` must reject a completed read whose effective workspace core changed
/// without relying only on sequence timing. Removing filter equality publishes core 7 rows under
/// the pinned core 9 selector when the old task wins the race.
#[test]
fn scope_change_rejects_pending_old_scope_result() {
    let requested = ReportFilter {
        core_uids: vec![7],
        ..ReportFilter::default()
    };
    let current = ReportFilter {
        core_uids: vec![9],
        ..ReportFilter::default()
    };

    assert!(report_query_result_is_current(4, 4, &requested, &requested));
    assert!(!report_query_result_is_current(4, 4, &requested, &current));
    assert!(!report_query_result_is_current(
        4, 5, &requested, &requested
    ));

    let mask_drift = ReportFilter {
        strategy_name_mask: "EMA_".to_string(),
        ..requested.clone()
    };
    assert!(
        !report_query_result_is_current(4, 4, &requested, &mask_drift),
        "a late result must not publish after the Auto strategy mask changes"
    );
}
