//! Pure ready-core and unfinished-market selection.

use moon_core::feed::{ConnStatus, OrderRow};
use moon_core::session::CoreId;

/// Cores a "cancel buys on all cores" press addresses, and how many it skipped as offline.
///
/// Args:
///     statuses: Every core the store holds, with its connection status.
///
/// Returns:
///     The `Ready` cores, sorted so the requests go out in a stable order, and the count of the rest.
pub(in crate::backend) fn ready_cores(
    statuses: impl Iterator<Item = (CoreId, ConnStatus)>,
) -> (Vec<CoreId>, usize) {
    let mut offline = 0;
    let mut ready: Vec<CoreId> = statuses
        .filter_map(|(core, status)| match status {
            ConnStatus::Ready => Some(core),
            _ => {
                offline += 1;
                None
            }
        })
        .collect();
    ready.sort_unstable();
    (ready, offline)
}

/// Markets a "cancel all buys" press addresses: every market of the snapshot that still carries an
/// order the core has not finished with.
///
/// Nothing narrower on purpose. The per-market path this feeds (`feed::trade::cancel_market_buys`)
/// picks the entry-phase orders itself, by the wire's own `OrderWorkerStatus`, so a market listed
/// here that only holds a position costs one no-op core command and nothing on the wire. The gate
/// this replaces required `pending` — a pending buy CONDITION, which moonproto keeps only while
/// `status == None` and clears the moment the order is placed — so an ordinary limit buy resting
/// on the exchange never qualified, and the hotkey sent nothing exactly when the trader had real
/// buys to pull. Shorts were excluded by the same gate; Moonbot's action is "cancel all buy orders
/// on all markets", pendings included, both sides, and the per-market button already does both.
///
/// Args:
///     orders: The core's retained order rows.
///
/// Returns:
///     The distinct markets, sorted, so the requests go out in a stable order.
pub(in crate::backend) fn cancel_all_buys_markets(orders: &[OrderRow]) -> Vec<String> {
    let markets: std::collections::BTreeSet<&str> = orders
        .iter()
        .filter(|o| !o.job_is_done)
        .map(|o| o.market.as_str())
        .collect();
    markets.into_iter().map(str::to_owned).collect()
}
