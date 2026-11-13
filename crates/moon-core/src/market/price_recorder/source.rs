//! Which core's list feeds the recorder, one per exchange and account currency.
//!
//! Cores with different account currencies carry different market lists. The first core whose
//! list arrives owns its currency group for as long as its lists keep coming; one silent for
//! [`TAKEOVER_MS`] — disconnected, stuck, removed — is replaced by the next core of the group
//! whose list arrives. Nothing is elected ahead of time: the owner is whoever is actually
//! delivering, so no core is relied upon that is not.
//!
//! The other cores' check costs one lock per list, about every two seconds each.
//! The primary currency keeps the legacy exchange key; other groups have independent keys.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use moonproto::state::MarketsState;

use super::ring::Row;
use crate::market::source::routing::market_quote;
use crate::session::CoreId;
use crate::symbol::Exchange;

/// An owner whose lists stopped this long ago is replaced. Under [`super::ring::GAP_MS`], so a
/// takeover does not break the run of lists.
pub(super) const TAKEOVER_MS: i64 = 5_000;

/// One list group; its currency is normalized by the session's election helper.
#[derive(Clone, PartialEq, Eq, Hash)]
struct SourceKey {
    exchange: Arc<str>,
    currency: Option<Box<str>>,
}

/// Known list groups and the cores currently delivering them.
#[derive(Default)]
struct Registry {
    /// Each running core's group, once it named its exchange.
    exchange_of: HashMap<CoreId, SourceKey>,
    /// Per group: the owning core and the stamp of its last list.
    owner: HashMap<SourceKey, (CoreId, i64)>,
    /// Catalog quotes retained for opaque names used by trade events.
    quotes: HashMap<(Arc<str>, String), Box<str>>,
}

impl Registry {
    /// Known currencies in deterministic primary-first order, as in provider election.
    fn currencies(&self, exchange: &str) -> Vec<Box<str>> {
        let mut currencies: Vec<_> = self
            .exchange_of
            .values()
            .filter(|key| &*key.exchange == exchange)
            .filter_map(|key| key.currency.clone())
            .collect();
        currencies.sort_by(|a, b| {
            (a.as_ref() != "USDT", a.as_ref()).cmp(&(b.as_ref() != "USDT", b.as_ref()))
        });
        currencies.dedup();
        currencies
    }

    /// Resolve an unknown currency as election does: USDT, then alphabetical, else unknown.
    fn key(&self, core: CoreId) -> Option<SourceKey> {
        let mut key = self.exchange_of.get(&core)?.clone();
        if key.currency.is_none() {
            key.currency = self.currencies(&key.exchange).into_iter().next();
        }
        Some(key)
    }
}

/// A source's immutable routing snapshot, so extraction and queueing use the same group.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Claim {
    pub storage: Arc<str>,
    exchange: Arc<str>,
    currency: Option<Box<str>>,
    currencies: Vec<Box<str>>,
}

impl std::ops::Deref for Claim {
    type Target = str;

    /// Let ownership checks inspect the key without exposing the routing snapshot.
    fn deref(&self) -> &str {
        &self.storage
    }
}

impl Claim {
    /// A primary change must never transfer one currency's validity to another currency.
    pub(super) fn ring_key(&self) -> Arc<str> {
        Arc::from(storage_key(&self.exchange, self.currency.as_deref(), None))
    }

    /// Assign quotes without a matching group to the primary; a single group keeps every row.
    fn accepts(&self, market: &str, catalog_quote: Option<&str>) -> bool {
        let quote = market_quote(exchange_kind(&self.exchange), market, catalog_quote);
        let group = self
            .currencies
            .iter()
            .find(|currency| currency.eq_ignore_ascii_case(quote))
            .or_else(|| self.currencies.first());
        group.map(|currency| &**currency) == self.currency.as_deref()
    }
}

/// The coarse key contains MoonProto's numeric exchange code before the first colon.
fn exchange_kind(exchange: &str) -> Exchange {
    exchange
        .split(':')
        .next()
        .and_then(|code| code.parse().ok())
        .map_or(Exchange::Unknown, Exchange::from_code)
}

/// Keep the existing key for the primary group, qualifying only secondary currencies.
fn storage_key(exchange: &str, currency: Option<&str>, primary: Option<&str>) -> String {
    match currency.filter(|currency| Some(*currency) != primary) {
        Some(currency) => format!("{exchange}|{currency}"),
        None => exchange.to_owned(),
    }
}

/// Resolve a public storage address using the same quote rule as row extraction.
pub(super) fn market_key(exchange: &str, market: &str) -> String {
    market_keys(exchange, market).0
}

/// Resolve a physical storage key to the stable group identity used for queue-loss and start stamps.
pub(super) fn recording_key(exchange: &str) -> String {
    if exchange.contains('|') {
        return exchange.to_owned();
    }
    with(|r| storage_key(exchange, r.currencies(exchange).first().map(|c| &**c), None))
}

/// Resolve storage and ring identities together: only storage assigns the primary a coarse key.
pub(super) fn market_keys(exchange: &str, market: &str) -> (String, String) {
    if exchange.contains('|') {
        return (exchange.to_owned(), exchange.to_owned());
    }
    with(|r| {
        let currencies = r.currencies(exchange);
        let catalog_quote = r.quotes.get(&(Arc::from(exchange), market.to_owned()));
        let quote = market_quote(exchange_kind(exchange), market, catalog_quote.map(|q| &**q));
        let group = currencies
            .iter()
            .find(|currency| currency.eq_ignore_ascii_case(quote))
            .or_else(|| currencies.first());
        let storage = storage_key(
            exchange,
            group.map(|c| &**c),
            currencies.first().map(|c| &**c),
        );
        let ring = storage_key(exchange, group.map(|c| &**c), None);
        (storage, ring)
    })
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

/// Serialize source updates and claims without holding a lock while reading market rows.
fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut guard = REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(guard.get_or_insert_with(Registry::default))
}

/// A core named its exchange (`"{code}:{dex:08x}"`) and normalized account currency.
pub(super) fn set_exchange(core: CoreId, exchange_key: &str, currency: Option<Box<str>>) {
    with(|r| {
        let key = SourceKey {
            exchange: Arc::from(exchange_key),
            currency,
        };
        if r.exchange_of.get(&core) != Some(&key) {
            release(r, core);
            r.exchange_of.insert(core, key);
        }
    });
}

/// A core went away or restarts: it owns nothing until it names its exchange again.
pub(super) fn forget(core: CoreId) {
    with(|r| {
        release(r, core);
        r.exchange_of.remove(&core);
    });
}

/// Hand the groups `core` owns to whoever delivers next.
fn release(r: &mut Registry, core: CoreId) {
    r.owner.retain(|_, (owner, _)| *owner != core);
}

/// Return the group's storage key and routing snapshot when `core` owns its list.
/// A previous owner whose fallback group changed no longer blocks this group's next source.
pub(super) fn claim(core: CoreId, now_ms: i64) -> Option<Claim> {
    with(|r| {
        let key = r.key(core)?;
        if r.owner
            .get(&key)
            .is_some_and(|&(owner, _)| r.key(owner).as_ref() != Some(&key))
        {
            r.owner.remove(&key);
        }
        match r.owner.get_mut(&key) {
            Some((owner, last)) if *owner == core => *last = now_ms,
            Some((_, last)) if now_ms.saturating_sub(*last) <= TAKEOVER_MS => return None,
            _ => {
                r.owner.insert(key.clone(), (core, now_ms));
            }
        }
        let currencies = r.currencies(&key.exchange);
        Some(Claim {
            storage: Arc::from(storage_key(
                &key.exchange,
                key.currency.as_deref(),
                currencies.first().map(|c| &**c),
            )),
            exchange: key.exchange,
            currency: key.currency,
            currencies,
        })
    })
}

/// The marked rows assigned to this owner's currency group from the list just applied.
///
/// moonproto clears every market's "mark found" before it applies a list and sets it on the rows
/// that carry one, so this reads exactly the futures rows of the latest list. Spot carries no
/// mark and drops out. Prices only, read through `MarketHandle::price` — never a copy of the
/// whole market (`STATION.md` §7.7 measured what that costs).
///
/// A market absent from a list and one the list carried without a mark look the same here, and
/// both keep their last recorded state: the client itself cannot tell them apart, and its prices
/// stand as they were until a list changes them.
///
/// Args:
///     markets: The client's markets, right after `PricesUpdated`.
///     included_funding: Whether the list carried funding at all.
///     claim: The elected owner's routing snapshot.
pub(super) fn rows_of(markets: &MarketsState, included_funding: bool, claim: &Claim) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut quotes = Vec::new();
    for handle in markets.iter() {
        let accepted = handle.with(|row| {
            let quote = market_quote(
                exchange_kind(&claim.exchange),
                handle.name(),
                Some(&row.base_currency),
            );
            quotes.push((
                handle.name().to_owned(),
                Box::<str>::from(quote.to_ascii_uppercase()),
            ));
            claim.accepts(handle.name(), Some(&row.base_currency))
        });
        if !accepted {
            continue;
        }
        let price = handle.price();
        if !price.mark_price_found {
            continue;
        }
        let funding = included_funding.then(|| {
            let next_ms = price.funding_time().unix_millis();
            (price.funding_rate, next_ms)
        });
        rows.push(Row {
            market: handle.name().to_owned(),
            bid: price.bid,
            ask: price.ask,
            mark: price.mark_price,
            funding,
        });
    }
    with(|r| {
        for (market, quote) in quotes {
            r.quotes.insert((claim.exchange.clone(), market), quote);
        }
    });
    rows
}

#[cfg(test)]
mod tests;
