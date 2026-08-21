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

/// Regression target: `market/source/mod.rs:max_order_notional` must keep a stated cap ahead of
/// the quantity fallback, or the Screener and trading toolbar show an order size the exchange did
/// not state and an operator can size an order against the wrong limit.
#[test]
fn stated_max_order_notional_precedes_the_quantity_fallback() {
    assert_eq!(
        max_order_notional(
            "BTCUSDT",
            crate::symbol::Exchange::Binance,
            250.0,
            4.0,
            100.0,
            10.0
        ),
        MaxOrder {
            value: 250.0,
            source: MaxOrderSource::Stated,
        }
    );
}

/// Regression target: `market/source/mod.rs:max_order_notional` must multiply a linear quantity
/// cap by its ask, not by a QUANTO contract size, or the toolbar reports a maximum far above the
/// exchange limit and its order is rejected.
#[test]
fn linear_quantity_cap_uses_the_current_ask_even_with_a_contract_size() {
    assert_eq!(
        max_order_notional(
            "ASTEROID_USDT",
            crate::symbol::Exchange::Gate,
            0.0,
            6.0,
            2.0,
            10_000.0,
        ),
        MaxOrder {
            value: 12.0,
            source: MaxOrderSource::Derived,
        }
    );
}

/// Regression target: `market/source/mod.rs:max_order_notional` must convert a quote-less
/// coin-margined contract through contract size, or an operator sees a cap wrong by the price
/// factor and submits an invalid order.
#[test]
fn inverse_quantity_cap_uses_contract_size_without_an_ask() {
    assert_eq!(
        max_order_notional(
            "AAVEPERP",
            crate::symbol::Exchange::Bybit,
            0.0,
            7.0,
            50.0,
            10.0,
        ),
        MaxOrder {
            value: 70.0,
            source: MaxOrderSource::Derived,
        }
    );
}

/// Regression target: `market/source/mod.rs:max_order_notional` must distinguish no exchange cap
/// from a linear cap waiting for its first ask, or the UI tells an operator that a known limit is
/// absent instead of still loading.
#[test]
fn max_order_notional_distinguishes_absent_from_pending_conversion() {
    assert_eq!(
        max_order_notional(
            "BTCUSDT",
            crate::symbol::Exchange::Binance,
            0.0,
            0.0,
            100.0,
            1.0
        ),
        MaxOrder {
            value: 0.0,
            source: MaxOrderSource::Absent,
        }
    );
    assert_eq!(
        max_order_notional(
            "BTCUSDT",
            crate::symbol::Exchange::Binance,
            0.0,
            4.0,
            0.0,
            1.0
        ),
        MaxOrder {
            value: 0.0,
            source: MaxOrderSource::Pending,
        }
    );
}

/// A hedged core keeps its legs SEPARATELY: the net size is zero while a short is open, and reading
/// the net alone would caption that market as flat. This is the same rule the Assets panel needed.
#[test]
fn a_leg_only_short_is_a_position() {
    let pos = moonproto::state::MarketBalancePosition {
        short_pos_size: 2.0,
        short_pos_price: 100.0,
        short_liq_price: 130.0,
        ..Default::default()
    };

    let (size, price, liq) = super::read::position_of(&pos);

    assert_eq!(size, Some(-2.0), "a short reads as a negative size");
    assert_eq!(price, 100.0);
    assert_eq!(liq, 130.0);
}

/// Direction is not always carried by the sign: a core can report a POSITIVE size with
/// `pos_dir == Sell`, and a caption that trusted the sign would paint that short as a long.
#[test]
fn a_positive_size_marked_sell_reads_as_a_short() {
    let pos = moonproto::state::MarketBalancePosition {
        pos_size: 3.0,
        pos_price: 50.0,
        liq_price: 70.0,
        pos_dir: moonproto::OrderType::Sell,
        ..Default::default()
    };

    let (size, ..) = super::read::position_of(&pos);

    assert_eq!(size, Some(-3.0));
}

/// A flat market has no position, and withholding the size withholds the entry and liquidation
/// prices with it — printing them beside no position would state prices nothing is held at.
#[test]
fn a_flat_market_reports_no_position() {
    let (size, price, liq) = super::read::position_of(&Default::default());
    assert_eq!((size, price, liq), (None, 0.0, 0.0));
}

/// `OrderType::Sell` is byte 0 — which is also `OrderType::default()`, what a core writes when it
/// states no direction at all. A hedge leg already knows its direction from the leg it was read
/// from, so the flag must not be consulted there: doing so printed every long-only hedge position
/// as a short.
#[test]
fn a_long_leg_stays_long_when_the_direction_flag_is_absent() {
    let pos = moonproto::state::MarketBalancePosition {
        long_pos_size: 2.0,
        long_pos_price: 100.0,
        long_liq_price: 70.0,
        // The default, i.e. "the core said nothing".
        pos_dir: moonproto::OrderType::default(),
        ..Default::default()
    };

    let (size, price, liq) = super::read::position_of(&pos);

    assert_eq!(size, Some(2.0), "a long leg is not a short");
    assert_eq!(price, 100.0);
    assert_eq!(liq, 70.0);
}
