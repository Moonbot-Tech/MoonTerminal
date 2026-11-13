//! Which core's list feeds the recorder, one per exchange and account currency.
//!
//! Cores with different account currencies carry different market lists. The first core whose
//! list arrives owns its currency group for as long as its lists keep coming; one silent for
//! [`TAKEOVER_MS`] — disconnected, stuck, removed — is replaced by the next core of the group
//! whose list arrives. Nothing is elected ahead of time: the owner is whoever is actually
//! delivering, so no core is relied upon that is not.
//!
//! The other cores' check costs one lock per list, about every two seconds each.
//! Storage keeps the coarse exchange key: markets already distinguish currencies on disk.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use moonproto::state::MarketsState;

use super::ring::Row;
use crate::session::CoreId;

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
}

impl Registry {
    /// Resolve an unknown currency as election does: USDT, then alphabetical, else unknown.
    fn key(&self, core: CoreId) -> Option<SourceKey> {
        let mut key = self.exchange_of.get(&core)?.clone();
        if key.currency.is_none() {
            key.currency = self
                .exchange_of
                .values()
                .filter(|other| other.exchange == key.exchange)
                .filter_map(|other| other.currency.as_ref())
                .min_by(|a, b| {
                    (a.as_ref() != "USDT", a.as_ref()).cmp(&(b.as_ref() != "USDT", b.as_ref()))
                })
                .cloned();
        }
        Some(key)
    }
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

/// Return the unchanged storage exchange key when `core` owns its currency group's list.
/// A previous owner whose fallback group changed no longer blocks this group's next source.
pub(super) fn claim(core: CoreId, now_ms: i64) -> Option<Arc<str>> {
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
        Some(key.exchange)
    })
}

/// The rows of the list just applied: every market the list carried a mark for.
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
pub(super) fn rows_of(markets: &MarketsState, included_funding: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    for handle in markets.iter() {
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
    rows
}

#[cfg(test)]
mod tests;
