//! What one By-IP address cell shows, resolved away from the renderer.
//!
//! The cell has THREE states and they mean different things, which is the whole reason this is a
//! module and not an `if` inside the row: the terminal knows the address and shows it, it knows
//! the address and the user has hidden the column, or it has no address at all. The renderer used
//! to collapse the last two into "draw nothing" — an unknown endpoint got an EMPTY fixed-width
//! slot with no glyph and no control — so a server the terminal could not name was indistinguishable
//! from a panel that had failed to draw. That is the question this module exists to answer.
//!
//! Masking is a PANEL-WIDE, transient user act rather than a per-server one, and it is not the
//! default. A view whose entire purpose is "По IP" shows addresses; hiding them is something the
//! user does deliberately before sharing a screen, from the one control in the column header.
//! Nothing here is persisted, so a fresh panel always comes up showing addresses.
//!
//! The length-hiding property of the mask lives with the mask CONSTANT in the renderer, not here:
//! this module decides only WHICH state a cell is in.

use std::net::IpAddr;

/// What one server's IP cell shows this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IpCell {
    /// The terminal knows the address and the column is unmasked.
    Shown(IpAddr),
    /// The terminal knows the address and the user has masked the column.
    Masked,
    /// No endpoint has reached the store for this server, so there is no address to show or hide.
    Unknown,
}

/// Resolve what one server's IP cell shows.
///
/// `masked` is panel-wide, so it can only ever hide an address that EXISTS. A server with no
/// address stays [`IpCell::Unknown`] whether or not the column is masked, and that precedence is
/// load-bearing: masking an unknown address into a run of asterisks would have the panel claim an
/// address it does not have, which is a worse version of the bug this replaces.
///
/// Args:
///     address: The server's shared endpoint address, when one has reached the store.
///     masked: Whether the user has hidden the whole IP column.
///
/// Returns:
///     The state this frame's cell renders.
pub(super) fn ip_cell(address: Option<IpAddr>, masked: bool) -> IpCell {
    match address {
        None => IpCell::Unknown,
        Some(_) if masked => IpCell::Masked,
        Some(address) => IpCell::Shown(address),
    }
}

/// What the column's ONE mask control offers, given the current state.
///
/// Pure, and deliberately kept out of the button that renders it, because this pairing is the one
/// place in the panel where a wrong answer is invisible. The per-row control this replaced had the
/// OPPOSITE default — it showed a struck-through eye while the address was hidden — so anyone
/// porting that code forward, or reading it in history, writes the inverse pairing. The result
/// compiles, renders and screenshots cleanly while telling the user the exact opposite of the
/// truth, and nothing downstream can catch it. Here, a test can.
///
/// Args:
///     masked: Whether the address column is currently hidden.
///
/// Returns:
///     The icon asset path and the locale key of its tooltip, in that order. The control names
///     what a click DOES, not what the column currently is: while nothing is hidden it offers to
///     hide, and only once the column is masked does it offer to show.
pub(super) fn mask_affordance(masked: bool) -> (&'static str, &'static str) {
    if masked {
        ("icons/eye.svg", "core_status.show_ip")
    } else {
        ("icons/eye-off.svg", "core_status.hide_ip")
    }
}

#[cfg(test)]
mod tests;
