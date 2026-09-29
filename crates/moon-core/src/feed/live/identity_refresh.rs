//! When the exchange identity a live client published has gone stale (#734).
//!
//! MoonProto fills `ServerInfo` only in its init (BaseCheck), so the feed can never re-read a
//! core's venue on the client it already has; it can only tell the session that the one it
//! published may be wrong. Two signals reach the feed:
//!
//! - `LifecycleEvent::ServerRestart`: a different MoonBot process answers the connection, and it
//!   may run on a different exchange.
//! - a hot exchange switch, where MoonBot changes venue with no restart at all. The feed never
//!   sees the switch itself. What it sees is the market-list refresh that follows: MoonProto
//!   MERGES a refreshed list into the retained one (old markets stay, so the list never shrinks)
//!   and reports only the names that were new, as `MarketsEvent::NewMarketsAdded`. A listing adds
//!   a handful of markets; a new venue adds most of its universe in one refresh. That turnover is
//!   the signal — the market count alone is not, since the merge keeps the old venue's markets.

use moonproto::LifecycleEvent;
use moonproto::state::MarketsEvent;

use crate::feed::IdentityStaleCause;

/// Fewest markets one refresh must add before its turnover can count as an exchange switch.
///
/// Keeps a small core, whose every listing is a large fraction of its few markets, from reading a
/// routine listing as a new venue.
const TURNOVER_MIN_ADDED: usize = 20;

/// Share of the retained universe one refresh must add, as `1 / TURNOVER_SHARE_DIVISOR`.
///
/// A quarter: a switched core retains the old venue's markets beside the new ones, so even a
/// complete replacement by an equal-sized venue shows up as half the merged list.
const TURNOVER_SHARE_DIVISOR: usize = 4;

/// Whether this lifecycle event makes the published identity stale.
///
/// Args:
///     ev: The lifecycle event being drained.
///
/// Returns:
///     The cause for a server restart, `None` for every other event.
pub(super) fn stale_on_lifecycle(ev: &LifecycleEvent) -> Option<IdentityStaleCause> {
    matches!(ev, LifecycleEvent::ServerRestart).then_some(IdentityStaleCause::ServerRestart)
}

/// Whether a market-list refresh replaced enough of the universe to mean a new exchange.
///
/// Args:
///     ev: The markets event being drained.
///     total: Markets the client retains after the refresh; asked only for an adding refresh, so
///         the snapshot read behind it is skipped for every other event.
///
/// Returns:
///     The turnover cause when the refresh added at least [`TURNOVER_MIN_ADDED`] markets and at
///     least a [`TURNOVER_SHARE_DIVISOR`]th of `total`; `None` otherwise.
pub(super) fn stale_on_markets(
    ev: &MarketsEvent,
    total: impl FnOnce() -> usize,
) -> Option<IdentityStaleCause> {
    let MarketsEvent::NewMarketsAdded { names } = ev else {
        return None;
    };
    let added = names.len();
    if added < TURNOVER_MIN_ADDED {
        return None;
    }
    let total = total();
    (added * TURNOVER_SHARE_DIVISOR >= total)
        .then_some(IdentityStaleCause::MarketTurnover { added, total })
}

#[cfg(test)]
mod tests;
