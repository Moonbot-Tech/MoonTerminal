//! Per-market tape routing using the catalog's quote before exchange naming fallbacks.

use std::collections::HashMap;

use super::MarketDataSourceInner;
use crate::feed::SharedMoonClient;
use crate::session::CoreId;
use crate::symbol::Exchange;

impl MarketDataSourceInner {
    /// Resolve under the caller's guard, keeping catalog ownership and tape routing consistent.
    pub(in crate::market) fn market_provider(&self, core: CoreId, market: &str) -> Option<CoreId> {
        let provider = self.core_provider.get(&core).copied()?;
        let overrides = self.market_provider_overrides.get(&core);
        if overrides.is_none() {
            return Some(provider);
        }
        let catalog_quote = self
            .clients
            .get(&provider)
            .and_then(SharedMoonClient::get)
            .and_then(|client| client.snapshot_versioned())
            .and_then(|snapshot| snapshot.markets().get(market))
            .map(|handle| handle.with(|market| market.base_currency.clone()));
        market_provider(
            Some(provider),
            self.exchange_of_provider(provider),
            overrides,
            market,
            catalog_quote.as_deref(),
        )
    }
}

/// Prefer an explicit catalog quote, since opaque spot and perpetual names omit the currency.
fn market_provider(
    provider: Option<CoreId>,
    exchange: Exchange,
    overrides: Option<&HashMap<Box<str>, CoreId>>,
    market: &str,
    catalog_quote: Option<&str>,
) -> Option<CoreId> {
    let from_name = crate::symbol::parse::split_market(market, exchange);
    let quote = catalog_quote
        .map(str::trim)
        .filter(|quote| !quote.is_empty())
        .unwrap_or(from_name.quote);
    quote_provider(provider, overrides, quote)
}

/// Resolve a known quote directly, without requiring an exchange-specific market spelling.
pub(super) fn quote_provider(
    provider: Option<CoreId>,
    overrides: Option<&HashMap<Box<str>, CoreId>>,
    quote: &str,
) -> Option<CoreId> {
    let provider = provider?;
    Some(
        overrides
            .into_iter()
            .flatten()
            .find(|(currency, _)| currency.eq_ignore_ascii_case(quote.trim()))
            .map_or(provider, |(_, target)| *target),
    )
}

#[cfg(test)]
mod tests;
