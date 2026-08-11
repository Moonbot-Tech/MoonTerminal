//! Unit tests for feed-level order rules shared by display and packet assembly.

use super::stop_inherited_from_strategy;

/// A stop switched off after the entry filled must not be re-supplied by its strategy.
///
/// The core materializes a stop INTO the order at the fill, so from then on the order's own flag is
/// the state. Answering with the strategy past that point is the whole bug: the table redrew a
/// hand-disabled SL as ON one frame after the strategy snapshot arrived and on every restart, and
/// `resolve_stop_group` re-armed it in the core when the neighbouring stop was clicked.
///
/// Mutation: drop the fill check. Both defects return at once, in display and in packets.
///
/// Returns:
///     Nothing; a filled entry ends inheritance whatever the strategy says.
#[test]
fn a_filled_entry_ends_inheritance_from_the_strategy() {
    assert!(!stop_inherited_from_strategy(true, true));
    assert!(!stop_inherited_from_strategy(true, false));
}

/// An order whose entry has not filled inherits its strategy's stops, because it has none of its
/// own.
///
/// This is what the Orders table shows for a working order: the stop the core will apply at the
/// fill, not a bare OFF suggesting the trader is unprotected by choice. It also covers the order
/// that holds a position with no buy leg at all — a sale of an already-held asset, which the core
/// never ran `CheckBuyOrder` for, so nothing materialized a stop into it.
///
/// Mutation: return `false` before the fill. Every waiting order would read OFF — SL in red —
/// while the strategy is about to arm the stop anyway.
///
/// Returns:
///     Nothing; inheritance follows the strategy flag until the entry fills.
#[test]
fn an_unfilled_entry_inherits_what_its_strategy_enables() {
    assert!(stop_inherited_from_strategy(false, true));
    assert!(!stop_inherited_from_strategy(false, false));
}
