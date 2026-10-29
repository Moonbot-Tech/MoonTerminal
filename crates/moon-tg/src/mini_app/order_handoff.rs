//! Current-list identities and expiring chart viewers across an order's closing handoff.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moon_core::feed::OrderRow;
use moon_core::telegram::web::dto::LevelDto;

use crate::TelegramState;

/// A previously listed or viewed order supplies identity after the session removes its row.
#[derive(Clone)]
pub(crate) struct OrderSnapshot {
    pub(super) uid: u64,
    pub(super) coin: String,
    pub(super) market: String,
    pub(super) entry_ms: i64,
    pub(super) buy_price: f64,
    pub(super) levels: Vec<LevelDto>,
}

impl OrderSnapshot {
    /// Capture only the closing chart's identity, prices and levels on the background executor.
    pub(super) fn new(order: &OrderRow) -> Self {
        Self {
            uid: order.uid,
            coin: order.coin.clone(),
            market: order.market.clone(),
            entry_ms: order.entry_fill_time_ms as i64,
            buy_price: order.buy_price,
            levels: super::charts::live_levels(order),
        }
    }
}

/// One core's listed identities are prepared and indexed off the owner thread.
pub(crate) type CoreSnapshots = Arc<BTreeMap<u64, OrderSnapshot>>;

/// A complete set of per-core list projections, ready for constant-time publication per core.
#[derive(Default)]
pub(super) struct PreparedSnapshots(BTreeMap<u64, CoreSnapshots>);

impl FromIterator<(u64, OrderSnapshot)> for PreparedSnapshots {
    /// Index identity lookups before the Finish callback returns to the owner.
    fn from_iter<T: IntoIterator<Item = (u64, OrderSnapshot)>>(rows: T) -> Self {
        let mut cores: BTreeMap<u64, BTreeMap<u64, OrderSnapshot>> = BTreeMap::new();
        for (core, row) in rows {
            cores.entry(core).or_default().insert(row.uid, row);
        }
        Self(
            cores
                .into_iter()
                .map(|(core, rows)| (core, Arc::new(rows)))
                .collect(),
        )
    }
}

/// A chart poll renews its execution lease; disappearing viewers cannot pin history forever.
pub(crate) struct OrderHandoff {
    pub(super) entry_ms: i64,
    pub(super) viewed_at: Instant,
}

/// Prefer an actively observed identity over an older list completion for a reused UID.
pub(super) fn order_snapshot(state: &TelegramState, core: u64, uid: u64) -> Option<&OrderSnapshot> {
    state
        .mini_order_snapshots
        .get(&(core, uid))
        .or_else(|| state.mini_order_listed.get(&core)?.get(&uid))
}

/// Capture the live chart's execution; a new entry with the same UID supersedes its old lease.
pub(super) fn remember_order(state: &mut TelegramState, core: u64, order: &OrderRow) {
    let key = (core, order.uid);
    let row = OrderSnapshot::new(order);
    if state
        .mini_order_handoffs
        .get(&key)
        .is_some_and(|entry| entry.entry_ms != row.entry_ms)
    {
        state.mini_order_handoffs.remove(&key);
    }
    state.mini_order_snapshots.insert(key, row);
}

/// Lease only a viewed identity, including a pending order that has not filled its entry yet.
pub(super) fn begin_order_handoff(state: &mut TelegramState, core: u64, uid: u64) {
    if let Some(row) = order_snapshot(state, core, uid).cloned() {
        state.mini_order_handoffs.insert(
            (core, uid),
            OrderHandoff {
                entry_ms: row.entry_ms,
                viewed_at: Instant::now(),
            },
        );
        state.mini_order_snapshots.insert((core, uid), row);
    }
}

/// Expire chart viewers even when no further chart or Orders request arrives.
pub(crate) fn prune_order_handoffs(state: &mut TelegramState, now: Instant) {
    // Live charts poll every 500 ms. A minute after the last view includes closing/report
    // retries; a continuously viewed execution renews before expiry, without a fleet-size cap.
    state.mini_order_handoffs.retain(|key, handoff| {
        let keep = now.saturating_duration_since(handoff.viewed_at) < Duration::from_secs(60);
        if !keep {
            state.mini_order_snapshots.remove(key);
        }
        keep
    });
}

/// Publish prepared list handles without rebuilding the fleet's identities on the owner.
pub(super) fn remember_order_snapshots(
    state: &mut TelegramState,
    cores: &[u64],
    mut prepared: PreparedSnapshots,
) {
    for core in cores {
        let old = if let Some(rows) = prepared.0.remove(core) {
            state.mini_order_listed.insert(*core, rows)
        } else {
            state.mini_order_listed.remove(core)
        };
        if let Some(old) = old {
            state.mini_order_retired.push(old);
        }
    }
    // Only viewed executions need individual retention. Listed identities live in the shared
    // maps, so a late list result never overwrites the chart's newer observed execution.
    state.mini_order_snapshots.retain(|key, row| {
        !cores.contains(&key.0)
            || state
                .mini_order_handoffs
                .get(key)
                .is_some_and(|handoff| handoff.entry_ms == row.entry_ms)
    });
}

/// Release only the completed execution; a reused UID belongs to its newer observed entry.
pub(super) fn finish_order_handoff(state: &mut TelegramState, core: u64, uid: u64, entry_ms: i64) {
    if state
        .mini_order_handoffs
        .get(&(core, uid))
        .is_some_and(|handoff| handoff.entry_ms == entry_ms)
    {
        state.mini_order_handoffs.remove(&(core, uid));
    }
    if state
        .mini_order_snapshots
        .get(&(core, uid))
        .is_some_and(|row| row.entry_ms == entry_ms)
    {
        state.mini_order_snapshots.remove(&(core, uid));
    }
}

#[cfg(test)]
mod tests;
