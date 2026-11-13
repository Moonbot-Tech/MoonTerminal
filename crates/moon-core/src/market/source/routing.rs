//! Per-market tape routing using the catalog's quote before exchange naming fallbacks.

use std::collections::HashMap;

use super::MarketDataSourceInner;
use crate::feed::SharedMoonClient;
use crate::session::CoreId;
use crate::symbol::Exchange;

impl MarketDataSourceInner {
    /// Copy a core's small routing table so catalog scans release the source lock before iterating.
    pub(in crate::market) fn market_routing(
        &self,
        core: CoreId,
    ) -> (Option<CoreId>, Exchange, Option<HashMap<Box<str>, CoreId>>) {
        let provider = self.core_provider.get(&core).copied();
        (
            provider,
            provider.map_or(Exchange::Unknown, |provider| {
                self.exchange_of_provider(provider)
            }),
            self.market_provider_overrides.get(&core).cloned(),
        )
    }

    /// Resolve under the caller's guard, keeping catalog ownership and tape routing consistent.
    pub(in crate::market) fn market_provider(&self, core: CoreId, market: &str) -> Option<CoreId> {
        let provider = self.core_provider.get(&core).copied()?;
        let overrides = self.market_provider_overrides.get(&core);
        if overrides.is_none() {
            return Some(provider);
        }
        let exchange = self.exchange_of_provider(provider);
        let handle = self
            .clients
            .get(&provider)
            .and_then(SharedMoonClient::get)
            .and_then(|client| client.snapshot_versioned())
            .and_then(|snapshot| snapshot.markets().get(market));
        match handle {
            Some(handle) => handle.with(|row| {
                market_provider(
                    Some(provider),
                    exchange,
                    overrides,
                    market,
                    Some(&row.base_currency),
                )
            }),
            None => market_provider(Some(provider), exchange, overrides, market, None),
        }
    }
}

/// Prefer an explicit catalog quote, since opaque spot and perpetual names omit the currency.
pub(in crate::market) fn market_provider(
    provider: Option<CoreId>,
    exchange: Exchange,
    overrides: Option<&HashMap<Box<str>, CoreId>>,
    market: &str,
    catalog_quote: Option<&str>,
) -> Option<CoreId> {
    quote_provider(
        provider,
        overrides,
        market_quote(exchange, market, catalog_quote),
    )
}

/// Share the catalog-first quote rule with recording, including opaque market names.
pub(in crate::market) fn market_quote<'a>(
    exchange: Exchange,
    market: &'a str,
    catalog_quote: Option<&'a str>,
) -> &'a str {
    catalog_quote
        .map(str::trim)
        .filter(|quote| !quote.is_empty())
        .unwrap_or_else(|| crate::symbol::parse::split_market(market, exchange).quote)
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
