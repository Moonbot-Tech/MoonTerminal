//! Computes dirty markets from a batch of domain events: which markets the chart must reread
//! (history/orderbook/meta). Global events without a market identity narrow to provider-wanted
//! markets, while targeted events mark their own market even when it is outside `wanted`.

use std::collections::HashMap;

use moonproto::state::{CoinCardCandlesEvent, MarketsEvent, OrderBookEvent, TradesEvent};
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
            // Deep CoinCard history arrived (authoritative OHLC for chart candles), so wake this
            // market's chart to rebuild the series.
            Event::CoinCardCandles(CoinCardCandlesEvent::Updated { market, .. }) => {
                push_dirty(&mut dirty, market.clone(), MarketDirtyFlags::HISTORY);
            }
            // A live-timeframe bar from a `subscribe_candles` subscription was applied to the
            // retained tf_candles. Wake the chart: a new bucket changes the deep-series signature
            // and requires a series rebuild, while quiet coins without trade events would
            // otherwise never wake at all.
            Event::LiveCandle(ev) if ev.applied_to_history => {
                push_dirty(
                    &mut dirty,
                    ev.market_name.clone(),
                    MarketDirtyFlags::HISTORY,
                );
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
