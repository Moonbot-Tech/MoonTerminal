//! Which core's list feeds the recorder, one per exchange.
//!
//! Every core of an exchange receives the same market list, so one of them is enough. The first
//! core whose list arrives owns the exchange for as long as its lists keep coming; one silent for
//! [`TAKEOVER_MS`] — disconnected, stuck, removed — is replaced by the next core of the exchange
//! whose list arrives. Nothing is elected ahead of time: the owner is whoever is actually
//! delivering, so no core is relied upon that is not.
//!
//! The other cores' check costs one lock per list, about every two seconds each.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use moonproto::state::MarketsState;

use super::ring::Row;
use crate::session::CoreId;

/// An owner whose lists stopped this long ago is replaced. Under [`super::ring::GAP_MS`], so a
/// takeover does not break the run of lists.
pub(super) const TAKEOVER_MS: i64 = 5_000;

#[derive(Default)]
struct Registry {
    /// Each running core's exchange key, once it named its exchange.
    exchange_of: HashMap<CoreId, Arc<str>>,
    /// Per exchange key: the owning core and the stamp of its last list.
    owner: HashMap<Arc<str>, (CoreId, i64)>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut guard = REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(guard.get_or_insert_with(Registry::default))
}

/// A core named its exchange (`"{code}:{dex:08x}"`).
pub(super) fn set_exchange(core: CoreId, exchange_key: &str) {
    with(|r| {
        let same = r
            .exchange_of
            .get(&core)
            .is_some_and(|k| &**k == exchange_key);
        if !same {
            release(r, core);
            r.exchange_of.insert(core, Arc::from(exchange_key));
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

/// Hand the exchanges `core` owns to whoever delivers next.
fn release(r: &mut Registry, core: CoreId) {
    r.owner.retain(|_, (owner, _)| *owner != core);
}

/// A list of `core` arrived at `now_ms`: its exchange key when this core is the one to record
/// it, `None` when another core owns the exchange or the core has not named its exchange yet.
pub(super) fn claim(core: CoreId, now_ms: i64) -> Option<Arc<str>> {
    with(|r| {
        let exchange = r.exchange_of.get(&core)?.clone();
        match r.owner.get_mut(&exchange) {
            Some((owner, last)) if *owner == core => *last = now_ms,
            Some((_, last)) if now_ms.saturating_sub(*last) <= TAKEOVER_MS => return None,
            _ => {
                r.owner.insert(exchange.clone(), (core, now_ms));
            }
        }
        Some(exchange)
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
