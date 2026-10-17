//! Computes dirty markets from a batch of domain events: which markets the chart must reread
//! (history/orderbook/meta). Global events without a market identity narrow to provider-wanted
//! markets, while targeted events mark their own market even when it is outside `wanted`.

use std::collections::HashMap;

use moonproto::Event;
use moonproto::state::{
    CoinCardCandlesEvent, MarketHistoryEvent, MarketsEvent, OrderBookEvent, TradesEvent,
};

use crate::feed::{MarketDirty, MarketDirtyFlags};

fn push_dirty(
    dirty: &mut HashMap<String, MarketDirtyFlags>,
    market: &str,
    flags: MarketDirtyFlags,
) {
    if let Some(existing) = dirty.get_mut(market) {
        // `BitOr` exists; `BitOrAssign` does not, so `|=` does not compile.
        *existing = existing.union(flags);
    } else {
        dirty.insert(market.to_owned(), flags);
    }
}

fn push_wanted_dirty(
    dirty: &mut HashMap<String, MarketDirtyFlags>,
    wanted: &[String],
    flags: MarketDirtyFlags,
) {
    for market in wanted {
        push_dirty(dirty, market, flags);
    }
}

pub(super) fn market_dirty_from_events(
    events: &[Event],
    wanted: &[String],
    force_sample: bool,
) -> Vec<MarketDirty> {
    let mut dirty = HashMap::<String, MarketDirtyFlags>::new();
    // Wanted-market flags are OR'd across the batch and applied once at the end.
    let mut wanted_flags = if force_sample {
        Some(MarketDirtyFlags::ALL)
    } else {
        None
    };

    for event in events {
        match event {
            Event::OrderBook(OrderBookEvent::Apply {
                market_name: Some(market),
                ..
            }) => {
                push_dirty(&mut dirty, market, MarketDirtyFlags::ORDERBOOK);
            }
            Event::Trade(TradesEvent::Applied { .. }) => {
                // MoonProto keeps TradesEvent intentionally small and does not
                // expose market names here. The terminal still narrows the wake
                // to provider-wanted markets instead of waking all charts on
                // every domain event.
                wanted_flags = Some(wanted_flags.map_or(MarketDirtyFlags::HISTORY, |flags| {
                    flags | MarketDirtyFlags::HISTORY
                }));
            }
            Event::Markets(MarketsEvent::PricesUpdated { .. }) => {
                wanted_flags = Some(wanted_flags.map_or(MarketDirtyFlags::HISTORY, |flags| {
                    flags | MarketDirtyFlags::HISTORY
                }));
            }
            // The core's chart archive was merged into this market's retained rings. `Ready` is an
            // apply barrier in MoonProto, so the older rows are already readable here. They sit
            // BEHIND every chart cursor, hence the archive flag on top of the history wake: only a
            // full reset re-reads the window and picks them up.
            Event::MarketHistory(MarketHistoryEvent::Ready { ticket, .. }) => {
                push_dirty(
                    &mut dirty,
                    &ticket.market,
                    MarketDirtyFlags::HISTORY | MarketDirtyFlags::HISTORY_ARCHIVE,
                );
            }
            // Deep CoinCard history arrived (authoritative OHLC for chart candles), so wake this
            // market's chart to rebuild the series.
            Event::CoinCardCandles(CoinCardCandlesEvent::Updated { market, .. }) => {
                push_dirty(&mut dirty, market, MarketDirtyFlags::HISTORY);
            }
            // A live-timeframe bar from a `subscribe_candles` subscription was applied to the
            // retained tf_candles. Wake the chart: a new bucket changes the deep-series signature
            // and requires a series rebuild, while quiet coins without trade events would
            // otherwise never wake at all.
            Event::LiveCandle(ev) if ev.applied_to_history => {
                push_dirty(&mut dirty, &ev.market_name, MarketDirtyFlags::HISTORY);
            }
            Event::Markets(
                MarketsEvent::MarketsListReplaced { .. }
                | MarketsEvent::NewMarketsAdded { .. }
                | MarketsEvent::IndexesUpdated { .. },
            ) => {
                wanted_flags = Some(wanted_flags.map_or(MarketDirtyFlags::MARKET_META, |flags| {
                    flags | MarketDirtyFlags::MARKET_META
                }));
            }
            _ => {}
        }
    }

    if let Some(flags) = wanted_flags {
        push_wanted_dirty(&mut dirty, wanted, flags);
    }

    dirty
        .into_iter()
        .map(|(market, flags)| MarketDirty::new(market, flags))
        .collect()
}

#[cfg(test)]
mod tests;
