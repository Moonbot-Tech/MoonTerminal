//! Вычисление «грязных» рынков из пачки доменных событий — какие рынки чарт должен
//! перечитать (history/orderbook/meta). Сужаем побудку до provider-wanted рынков.

use std::collections::HashMap;

use moonproto::state::{MarketsEvent, OrderBookEvent, TradesEvent};
use moonproto::Event;

use crate::feed::{MarketDirty, MarketDirtyFlags};

fn push_dirty(
    dirty: &mut HashMap<String, MarketDirtyFlags>,
    market: impl Into<String>,
    flags: MarketDirtyFlags,
) {
    dirty
        .entry(market.into())
        .and_modify(|existing| *existing = existing.union(flags))
        .or_insert(flags);
}

fn push_wanted_dirty(
    dirty: &mut HashMap<String, MarketDirtyFlags>,
    wanted: &[String],
    flags: MarketDirtyFlags,
) {
    for market in wanted {
        push_dirty(dirty, market.clone(), flags);
    }
}

pub(super) fn market_dirty_from_events(
    events: &[Event],
    wanted: &[String],
    force_sample: bool,
) -> Vec<MarketDirty> {
    let mut dirty = HashMap::<String, MarketDirtyFlags>::new();
    if force_sample {
        push_wanted_dirty(&mut dirty, wanted, MarketDirtyFlags::ALL);
    }

    for event in events {
        match event {
            Event::OrderBook(OrderBookEvent::Apply {
                market_name: Some(market),
                ..
            }) => {
                push_dirty(&mut dirty, market.to_string(), MarketDirtyFlags::ORDERBOOK);
            }
            Event::Trade(TradesEvent::Applied { .. }) => {
                // MoonProto keeps TradesEvent intentionally small and does not
                // expose market names here. The terminal still narrows the wake
                // to provider-wanted markets instead of waking all charts on
                // every domain event.
                push_wanted_dirty(&mut dirty, wanted, MarketDirtyFlags::HISTORY);
            }
            Event::Markets(MarketsEvent::PricesUpdated { .. }) => {
                push_wanted_dirty(&mut dirty, wanted, MarketDirtyFlags::HISTORY);
            }
            Event::Markets(
                MarketsEvent::MarketsListReplaced { .. }
                | MarketsEvent::NewMarketsAdded { .. }
                | MarketsEvent::IndexesUpdated { .. },
            ) => {
                push_wanted_dirty(&mut dirty, wanted, MarketDirtyFlags::MARKET_META);
            }
            _ => {}
        }
    }

    dirty
        .into_iter()
        .map(|(market, flags)| MarketDirty::new(market, flags))
        .collect()
}
