//! The optimistic Panic Sell override every host of a Panic Sell button keeps: a pressed toggle
//! shows its new state at once, before the core's order update confirms it.
//!
//! The rules live here, once; the map of overrides belongs to each host — the terminal shares its
//! map between the chart button and the Mini App, the station keeps one for the Mini App alone.

use std::time::{Duration, Instant};

use super::{CoreId, CoreStore};

/// How long a fresh `PanicLocal` override outranks the core snapshot.
///
/// The override's only job is bridging one core round trip. 3 s is >= 3x the slowest in-app data
/// cadence (the 1000 ms background-panel floor) and covers a WAN round trip to a VPS-hosted core
/// plus one order-publish tick. Matches the in-repo `stop_overlay` TTL constant and now its
/// lifecycle too: on expiry we prefer the core's truth over our optimistic guess, which is
/// correct on the money path where the core is the authority.
pub const PANIC_LOCAL_TTL: Duration = Duration::from_secs(3);

/// Optimistic Panic Sell override for one `(core, market)`.
///
/// It records both arm and disarm requests. The reconciliation tick drops it when the core agrees
/// or its TTL expires, returning authority to the retained core snapshot.
#[derive(Clone, Copy, Debug)]
pub struct PanicLocal {
    /// The armed state this override asserts, pending core confirmation.
    pub want: bool,
    /// When this override was recorded, for TTL and settle comparisons.
    pub at: Instant,
}

/// Resolve the effective armed state from an optional fresh local override and the core snapshot.
///
/// `local` carries `(want, age)` when a `PanicLocal` exists. While `age < PANIC_LOCAL_TTL` the
/// override outranks the snapshot in both directions (arm and disarm); once stale, or absent, the
/// snapshot is authoritative. `snapshot_armed` is supplied LAZILY and is not evaluated at all while
/// a fresh override decides the answer: the terminal calls this on the chart render path, and the
/// snapshot walk is `order_lines.iter_market`, so skipping it on the common post-press path matters.
///
/// Args:
///     local: Requested state and age for the optional local override.
///     snapshot_armed: Deferred lookup of the retained core state.
///
/// Returns:
///     The fresh local state when available, otherwise the retained core state.
pub fn effective_panic_armed(
    local: Option<(bool, Duration)>,
    snapshot_armed: impl FnOnce() -> bool,
) -> bool {
    match local {
        Some((want, age)) if age < PANIC_LOCAL_TTL => want,
        _ => snapshot_armed(),
    }
}

/// Whether a `PanicLocal` override has settled and may be dropped by the reconciliation tick.
///
/// Settled once the TTL has elapsed (the override can no longer influence `effective_panic_armed`)
/// or the moment the core snapshot agrees with what the override asserts -- dropping it as soon as
/// the core agrees, rather than only on the user's next press, is what stops a transient agreement
/// from being forgotten and turning an intended re-arm into a disarm.
///
/// Args:
///     want: Armed state asserted by the local override.
///     age: Time since the override was accepted.
///     snapshot_armed: Current state from the retained core snapshot.
///
/// Returns:
///     `true` when the override cannot change the effective state any longer.
pub fn panic_local_settled(want: bool, age: Duration, snapshot_armed: bool) -> bool {
    age >= PANIC_LOCAL_TTL || snapshot_armed == want
}

/// Return whether the retained order-line snapshot shows panic sell armed for `(core, market)`.
///
/// Args:
///     store: The sessions' account plane.
///     core: Core whose retained order lines are queried.
///     market: Market whose open order lines are queried.
///
/// Returns:
///     `true` when an open retained order line has panic sell armed.
pub fn panic_snapshot_armed(store: &CoreStore, core: CoreId, market: &str) -> bool {
    store.core(core).is_some_and(|data| {
        data.order_lines
            .iter_market(market)
            .any(|order| order.closed_ms.is_none() && order.panic_sell)
    })
}

#[cfg(test)]
mod tests;
