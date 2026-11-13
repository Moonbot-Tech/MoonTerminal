//! Pure provider election by exchange identity and normalized account currency.

use std::collections::{HashMap, HashSet};

use crate::feed::ExchangeId;
use crate::session::CoreId;

/// A shared tape's identity without changing coarse exchange identities used by caches.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProviderKey {
    pub(crate) exchange: ExchangeId,
    pub(crate) currency: Option<Box<str>>,
}

/// Normalize an account currency, treating whitespace-only values as unknown.
pub(crate) fn normalize_currency(raw: &str) -> Option<Box<str>> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_ascii_uppercase().into_boxed_str())
}

/// Resolve a market against same-exchange currency overrides, retaining the catalog provider
/// when the quote has no elected group. Parsing and case folding borrow the market name.
pub(crate) fn market_provider(
    provider: Option<CoreId>,
    exchange: crate::symbol::Exchange,
    overrides: Option<&HashMap<Box<str>, CoreId>>,
    market: &str,
) -> Option<CoreId> {
    let provider = provider?;
    let Some(overrides) = overrides else {
        return Some(provider);
    };
    let quote = crate::symbol::parse::split_market(market, exchange)
        .quote
        .trim();
    Some(
        overrides
            .iter()
            .find(|(currency, _)| currency.eq_ignore_ascii_case(quote))
            .map_or(provider, |(_, target)| *target),
    )
}

/// The session facts needed for election, supplied in deterministic session order.
pub(crate) struct ElectionCore<'a> {
    pub(crate) id: CoreId,
    pub(crate) exchange: Option<ExchangeId>,
    pub(crate) currency: Option<&'a str>,
    pub(crate) ready: bool,
}

/// Elect one provider per currency group and map each identified core to its provider.
/// Unknown currencies join USDT when present, otherwise the first known currency alphabetically.
/// The exact key's Ready provider wins before providers moving from another key; remaining ties
/// follow the supplied core order, preserving a healthy tape when a currency first arrives.
pub(crate) fn elect(
    cores: &[ElectionCore<'_>],
    current: &HashMap<ProviderKey, CoreId>,
) -> (HashMap<ProviderKey, CoreId>, HashMap<CoreId, CoreId>) {
    let normalized: Vec<_> = cores
        .iter()
        .map(|core| (core, core.currency.and_then(normalize_currency)))
        .collect();
    let mut fallback: HashMap<ExchangeId, Box<str>> = HashMap::new();
    for (core, currency) in &normalized {
        if let (Some(exchange), Some(currency)) = (core.exchange, currency) {
            fallback
                .entry(exchange)
                .and_modify(|chosen| {
                    if currency.as_ref() == "USDT"
                        || (chosen.as_ref() != "USDT" && currency < chosen)
                    {
                        *chosen = currency.clone();
                    }
                })
                .or_insert_with(|| currency.clone());
        }
    }

    let mut groups: HashMap<ProviderKey, Vec<&ElectionCore<'_>>> = HashMap::new();
    for (core, currency) in normalized {
        if let Some(exchange) = core.exchange {
            let currency = currency.or_else(|| fallback.get(&exchange).cloned());
            groups
                .entry(ProviderKey { exchange, currency })
                .or_default()
                .push(core);
        }
    }

    let current_providers: HashSet<CoreId> = current.values().copied().collect();
    let mut elected = HashMap::new();
    let mut core_provider = HashMap::new();
    for (key, members) in groups {
        let provider = members
            .iter()
            .find(|core| core.ready && current.get(&key) == Some(&core.id))
            .or_else(|| {
                members
                    .iter()
                    .find(|core| core.ready && current_providers.contains(&core.id))
            })
            .or_else(|| members.iter().find(|core| core.ready))
            .or_else(|| members.first());
        if let Some(provider) = provider {
            let provider = provider.id;
            elected.insert(key, provider);
            for core in members {
                core_provider.insert(core.id, provider);
            }
        }
    }
    (elected, core_provider)
}

#[cfg(test)]
mod tests;
