//! Which cores are already trading a searched coin, for the dropdown's "in trade" marker.
//!
//! A core is TRADING a market when it holds either an open order on it (a row in its order table
//! whose job is not done) or an open position (an asset row with a non-zero position size). The
//! match is by the core's own market KEY — the same key a hit opens — never by the bare coin, so a
//! perpetual and a dated contract of one coin (`BTCUSDT` vs `BTCUSDT_0925`) stay two instruments.
//!
//! Read from local per-core state only; nothing asks the core. The per-core set is rebuilt when
//! that core's order-table or assets revision moves, not per frame: the popup re-runs its search
//! on every render while it is open.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use moon_core::session::CoreId;

use super::CoinHit;
use crate::Backend;

/// Revisions a cached set was built under: order table, assets, connection epoch.
type Stamp = (u64, u64, u64);

thread_local! {
    /// Per-core traded-market sets, keyed by the revisions they were built from. UI-thread only.
    static CACHE: RefCell<HashMap<CoreId, (Stamp, Rc<HashSet<String>>)>> =
        RefCell::new(HashMap::new());
}

/// The markets one core is trading, from its order rows and its position rows.
///
/// Args:
///     orders: `(market, job_is_done)` for every row in the core's order table.
///     positions: `(market, pos_size)` for every asset row the core reported.
///
/// Returns:
///     The market keys holding an open order or a non-zero, finite position.
pub(super) fn trading_markets<'a>(
    orders: impl IntoIterator<Item = (&'a str, bool)>,
    positions: impl IntoIterator<Item = (&'a str, f64)>,
) -> HashSet<String> {
    let open_orders = orders
        .into_iter()
        .filter(|(_, done)| !done)
        .map(|(market, _)| market);
    let open_positions = positions
        .into_iter()
        .filter(|(_, size)| size.is_finite() && *size != 0.0)
        .map(|(market, _)| market);
    open_orders
        .chain(open_positions)
        .map(str::to_string)
        .collect()
}

/// The markets `core` is trading, rebuilt only when its store revisions moved.
fn core_markets(b: &Backend, core: CoreId) -> Rc<HashSet<String>> {
    let store = b.session.store();
    let Some(data) = store.core(core) else {
        return Rc::new(HashSet::new());
    };
    let stamp = (data.orders_table_rev, data.assets_rev, data.conn_epoch);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((at, set)) = cache.get(&core)
            && *at == stamp
        {
            return set.clone();
        }
        let set = Rc::new(trading_markets(
            data.orders
                .iter()
                .map(|o| (o.market.as_str(), o.job_is_done)),
            data.assets
                .rows
                .iter()
                .map(|r| (r.market.as_str(), r.pos_size)),
        ));
        cache.insert(core, (stamp, set.clone()));
        set
    })
}

/// Set [`CoinHit::in_trade`] on every hit through `trading`, which answers per core and market.
///
/// Args:
///     hits: Resolved hits; only their flag changes.
///     trading: Whether a core is trading a market key.
pub(super) fn mark_hits(hits: &mut [CoinHit], mut trading: impl FnMut(CoreId, &str) -> bool) {
    for hit in hits {
        hit.in_trade = trading(hit.core, &hit.market);
    }
}

/// [`mark_hits`] against the live store.
pub(super) fn mark_live(b: &Backend, hits: &mut [CoinHit]) {
    let mut sets: HashMap<CoreId, Rc<HashSet<String>>> = HashMap::new();
    mark_hits(hits, |core, market| {
        sets.entry(core)
            .or_insert_with(|| core_markets(b, core))
            .contains(market)
    });
}

/// The cores under one coin row in display order: those trading it first, each side keeping its
/// canonical order.
///
/// Display only. The coin row still opens [`super::pick_core`]'s choice over the UNSORTED members,
/// so marking a core never changes which core a click or `Enter` opens.
pub(crate) fn in_trade_first(members: &[CoinHit]) -> Vec<&CoinHit> {
    let mut out: Vec<&CoinHit> = members.iter().collect();
    out.sort_by_key(|hit| !hit.in_trade);
    out
}

#[cfg(test)]
mod tests;
