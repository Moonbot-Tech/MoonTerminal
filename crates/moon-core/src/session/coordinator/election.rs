//! Pure provider election by exchange identity and normalized account currency.

use std::collections::{HashMap, HashSet};

use crate::feed::ExchangeId;
use crate::session::CoreId;

/// A shared tape's identity without changing coarse exchange identities used by caches.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)] // wired in by reconcile_providers (next batch)
pub(crate) struct ProviderKey {
    pub(crate) exchange: ExchangeId,
    pub(crate) currency: Option<Box<str>>,
}

/// Normalize an account currency, treating whitespace-only values as unknown.
#[allow(dead_code)] // wired in by reconcile_providers (next batch)
pub(crate) fn normalize_currency(raw: &str) -> Option<Box<str>> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_ascii_uppercase().into_boxed_str())
}

/// The session facts needed for election, supplied in deterministic session order.
#[allow(dead_code)] // wired in by reconcile_providers (next batch)
pub(crate) struct ElectionCore<'a> {
    pub id: CoreId,
    pub exchange: Option<ExchangeId>,
    pub currency: Option<&'a str>,
    pub ready: bool,
}

/// Elect one provider per currency group and map each identified core to its provider.
/// Unknown currencies join USDT when present, otherwise the first known currency alphabetically.
/// Ready current providers survive key changes; all ties follow the supplied core order.
#[allow(dead_code)] // wired in by reconcile_providers (next batch)
pub(crate) fn elect(
    cores: &[ElectionCore<'_>],
    current: &HashMap<ProviderKey, CoreId>,
) -> (HashMap<ProviderKey, CoreId>, HashMap<CoreId, CoreId>) {
    let mut fallback: HashMap<ExchangeId, Box<str>> = HashMap::new();
    for core in cores {
        if let (Some(exchange), Some(currency)) =
            (core.exchange, core.currency.and_then(normalize_currency))
        {
            fallback
                .entry(exchange)
                .and_modify(|chosen| {
                    if currency.as_ref() == "USDT"
                        || (chosen.as_ref() != "USDT" && currency < *chosen)
                    {
                        *chosen = currency.clone();
                    }
                })
                .or_insert(currency);
        }
    }

    let mut groups: HashMap<ProviderKey, Vec<&ElectionCore<'_>>> = HashMap::new();
    for core in cores {
        if let Some(exchange) = core.exchange {
            let currency = core
                .currency
                .and_then(normalize_currency)
                .or_else(|| fallback.get(&exchange).cloned());
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
            .find(|core| core.ready && current_providers.contains(&core.id))
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
