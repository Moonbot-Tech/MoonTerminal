use std::collections::HashMap;

use crate::feed::{MarketDirty, MarketDirtyFlags};
use crate::market::MarketStore;

use super::*;

#[test]
fn orderbook_cadence_phase_is_stable_and_bounded() {
    let a = cadence_phase_ms(1, "BTCUSDT", ORDERBOOK_PULL_PERIOD_MS);
    let b = cadence_phase_ms(1, "BTCUSDT", ORDERBOOK_PULL_PERIOD_MS);
    let c = cadence_phase_ms(1, "ETHUSDT", ORDERBOOK_PULL_PERIOD_MS);

    assert_eq!(a, b);
    assert!(a < ORDERBOOK_PULL_PERIOD_MS);
    assert!(c < ORDERBOOK_PULL_PERIOD_MS);
}

#[test]
fn cadence_slot_waits_until_phase_then_advances_by_period() {
    assert_eq!(cadence_slot(99, 100, 200), None);
    assert_eq!(cadence_slot(100, 100, 200), Some(0));
    assert_eq!(cadence_slot(299, 100, 200), Some(0));
    assert_eq!(cadence_slot(300, 100, 200), Some(1));
}

/// The four per-market counters, so an assertion reads as one line and a fifth counter later
/// changes one place instead of every assertion.
fn counters(revs: MarketRevisions) -> (u64, u64, u64, u64) {
    (revs.history, revs.book, revs.meta, revs.archive)
}

#[test]
fn market_dirty_flags_bump_only_their_slice_revisions() {
    let source = MarketDataSource::new(MarketStore::shared(0.0));
    let mut providers = HashMap::new();
    providers.insert(7, 42);
    source.set_provider_map(&providers);

    let initial = source.market_revisions(7, "BTCUSDT").unwrap();
    assert_eq!(counters(initial), (0, 0, 0, 0));

    source.mark_dirty(
        42,
        &[MarketDirty::new("BTCUSDT", MarketDirtyFlags::ORDERBOOK)],
    );
    let after_book = source.market_revisions(7, "BTCUSDT").unwrap();
    assert_eq!(counters(after_book), (0, 1, 0, 0));

    source.mark_dirty(
        42,
        &[MarketDirty::new(
            "BTCUSDT",
            MarketDirtyFlags::HISTORY | MarketDirtyFlags::MARKET_META,
        )],
    );
    let after_history_meta = source.market_revisions(7, "BTCUSDT").unwrap();
    // The archive counter must stay at zero here: live history arriving at the edge is what cursors
    // are for. If it moved with HISTORY, every chart would force a full reset on every trade batch.
    assert_eq!(counters(after_history_meta), (1, 1, 1, 0));

    source.mark_dirty(
        42,
        &[MarketDirty::new(
            "BTCUSDT",
            MarketDirtyFlags::HISTORY | MarketDirtyFlags::HISTORY_ARCHIVE,
        )],
    );
    let after_archive = source.market_revisions(7, "BTCUSDT").unwrap();
    assert_eq!(counters(after_archive), (2, 1, 1, 1));
}

/// The periodic force-sample passes `ALL`, so an archive bit hiding in it would order a full
/// chart reset on every wanted-market change with no archive behind it.
#[test]
fn all_dirty_flags_exclude_the_chart_archive() {
    assert!(!MarketDirtyFlags::ALL.contains(MarketDirtyFlags::HISTORY_ARCHIVE));
    assert!(MarketDirtyFlags::ALL.contains(MarketDirtyFlags::HISTORY));
}
