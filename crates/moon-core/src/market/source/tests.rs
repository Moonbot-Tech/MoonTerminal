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

#[test]
fn market_dirty_flags_bump_only_their_slice_revisions() {
    let source = MarketDataSource::new(MarketStore::shared(0.0));
    let mut providers = HashMap::new();
    providers.insert(7, 42);
    source.set_provider_map(&providers);

    let initial = source.market_revisions(7, "BTCUSDT").unwrap();
    assert_eq!((initial.history, initial.book, initial.meta), (0, 0, 0));

    source.mark_dirty(
        42,
        &[MarketDirty::new("BTCUSDT", MarketDirtyFlags::ORDERBOOK)],
    );
    let after_book = source.market_revisions(7, "BTCUSDT").unwrap();
    assert_eq!(
        (after_book.history, after_book.book, after_book.meta),
        (0, 1, 0)
    );

    source.mark_dirty(
        42,
        &[MarketDirty::new(
            "BTCUSDT",
            MarketDirtyFlags::HISTORY | MarketDirtyFlags::MARKET_META,
        )],
    );
    let after_history_meta = source.market_revisions(7, "BTCUSDT").unwrap();
    assert_eq!(
        (
            after_history_meta.history,
            after_history_meta.book,
            after_history_meta.meta
        ),
        (1, 1, 1)
    );
}
