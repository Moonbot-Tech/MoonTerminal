//! The old per-event wanted-market fold, kept here as the oracle for the one-shot mark.

use std::collections::HashMap;
use std::sync::Arc;

use moonproto::Event;
use moonproto::state::{OrderBookEvent, OrderBookKind, TopOfBook, TradesEvent};

use crate::feed::{MarketDirty, MarketDirtyFlags};

use super::market_dirty_from_events;

/// Old fold: every trade event unions `HISTORY` onto each wanted name, and an order-book apply
/// unions `ORDERBOOK` onto its own market. `force_sample` starts from `ALL`.
fn reference_fold(
    events: &[Event],
    wanted: &[String],
    force_sample: bool,
) -> Vec<(String, MarketDirtyFlags)> {
    let mut dirty = HashMap::<String, MarketDirtyFlags>::new();
    if force_sample {
        for market in wanted {
            union_into(&mut dirty, market, MarketDirtyFlags::ALL);
        }
    }
    for event in events {
        match event {
            Event::OrderBook(OrderBookEvent::Apply {
                market_name: Some(market),
                ..
            }) => {
                union_into(&mut dirty, market, MarketDirtyFlags::ORDERBOOK);
            }
            Event::Trade(TradesEvent::Applied { .. }) => {
                for market in wanted {
                    union_into(&mut dirty, market, MarketDirtyFlags::HISTORY);
                }
            }
            _ => {}
        }
    }
    let mut rows: Vec<_> = dirty.into_iter().collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

fn union_into(
    dirty: &mut HashMap<String, MarketDirtyFlags>,
    market: &str,
    flags: MarketDirtyFlags,
) {
    dirty
        .entry(market.to_owned())
        .and_modify(|existing| *existing = existing.union(flags))
        .or_insert(flags);
}

fn sorted(dirty: Vec<MarketDirty>) -> Vec<(String, MarketDirtyFlags)> {
    let mut rows: Vec<_> = dirty
        .into_iter()
        .map(|row| (row.market, row.flags))
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

/// One thousand trades plus one book must mark the same markets as folding each event itself.
///
/// Breakage: applying the wanted flags inside the event loop clones every wanted name per trade.
/// Replacing flags instead of OR-ing them drops `HISTORY` from the market whose book also moved.
#[test]
fn a_trade_batch_marks_wanted_markets_once_and_keeps_the_book_flag() {
    let wanted: Vec<String> = (0..50).map(|index| format!("M{index:02}")).collect();
    let book = Event::OrderBook(OrderBookEvent::Apply {
        market_name: Some(Arc::<str>::from(wanted[0].as_str())),
        kind: OrderBookKind::Futures,
        is_full: false,
        top: TopOfBook::default(),
    });
    let mut events = Vec::with_capacity(1_001);
    for _ in 0..500 {
        events.push(Event::Trade(TradesEvent::Applied {}));
    }
    events.push(book);
    for _ in 0..500 {
        events.push(Event::Trade(TradesEvent::Applied {}));
    }

    let actual = sorted(market_dirty_from_events(&events, &wanted, false));
    let expected = reference_fold(&events, &wanted, false);

    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 50);
    assert_eq!(
        actual[0].1,
        MarketDirtyFlags::HISTORY | MarketDirtyFlags::ORDERBOOK
    );
    assert!(
        actual[1..]
            .iter()
            .all(|(_, flags)| *flags == MarketDirtyFlags::HISTORY)
    );
}
