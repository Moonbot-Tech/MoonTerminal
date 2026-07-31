//! Regression tests for Report strategy-catalog refresh scope.

use moon_core::db::{ReportFilter, ReportStrategyKey, SideFilter};

use super::{strategy_catalog_scope, strategy_metadata_request};

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
        side: SideFilter::Long,
        emulator: Some(false),
        deleted_only: false,
        closed_only: true,
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 7,
            strategy_id: -11,
        }]),
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

    let published = strategy_catalog_scope(&first);
    assert_eq!(published.core_uids, vec![3, 7]);
    assert_eq!(published.coin, "BTC");
    assert_eq!(published.strategies, None);
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
