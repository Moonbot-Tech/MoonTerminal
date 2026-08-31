//! The shared core UPDATE control: enqueue a plain release update for one core or a whole
//! server, retry a core whose last attempt failed, and update the fleet.
//!
//! A SIBLING of [`crate::controls::core_run`], not a fork of it -- read that module before this
//! one. What is REUSED is its vocabulary and its rules, never its code:
//! - only REACHABLE cores are ever commanded, same reason -- the per-core command channel
//!   outlives a disconnect;
//! - a shared literal element id would migrate GPUI hover/press state between rows, so every
//!   identity here is derived from the core or server it belongs to, never from a row index;
//! - a `rev()` folds into the hosting panel's repaint gate (`core_update_rev()`, wired in
//!   `panels/core_status/mod.rs`).
//!
//! What is NOT reused is [`crate::controls::core_run::RunPending`]: a run intent is answered
//! within seconds by the same connection and dies with the process, while an update is answered
//! by a version change over MINUTES, its state already lives on `SessionManager`
//! (`core_update_phase`), and its phase is what a badge beside this control already draws. There
//! is no second pending register here -- read the phase.
//!
//! Every action goes through `session.enqueue_core_update(s)`, never `update_core_version`
//! directly: bulk fills the queue, it never bursts commands.

mod actions;
mod view;

pub(crate) use actions::{retry_core, update_core, update_fleet, update_scope};
pub(crate) use view::update_button;

use moon_core::feed::ConnStatus;
use moon_core::session::core_update::CoreUpdatePhase;

/// Design-reference edge of the button's slot, matching [`crate::controls::core_run`]'s own
/// `SLOT_W` so a hover-revealed update control lines up with the run buttons beside it. A
/// separate constant on purpose -- this module reuses the geometry, not the type, and the two
/// columns must be free to diverge later without one edit touching the other's meaning.
pub(crate) const SLOT_W: f32 = 18.0;

/// One core's relationship to the update queue for drawing a control. The rendering rule reads
/// the lifecycle, build, endpoint, and phase facts that `moon_core::session::core_update`'s private
/// `eligible` gate also considers; the authoritative accept/reject call is still
/// `SessionManager::enqueue_core_update(s)`, made when the button is actually pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OfferState {
    /// Ready, with a known build and a known address, and not already tracked in a live attempt
    /// -- the queue would accept this core right now.
    Offerable,
    /// Tracked in a live (non-`Done`) attempt already: `Queued`, `Sent`, or `Waiting`. The badge
    /// beside this cell already says so.
    Tracked,
    /// Not `Ready`, or missing a reported build or a known address -- everything `eligible`
    /// rejects for a reason other than already being tracked.
    Offline,
}

/// Classify one core against the update queue.
///
/// Args:
///     status: The core's latest connection lifecycle state.
///     server_version: The core's last reported build, when it reported one.
///     endpoint_known: Whether the core's address has reached the store.
///     update: The core's current update-queue phase, when it has ever been enqueued.
///
/// Returns:
///     What this core currently offers the update control.
pub(crate) fn offer_state(
    status: &ConnStatus,
    server_version: Option<u32>,
    endpoint_known: bool,
    update: Option<&CoreUpdatePhase>,
) -> OfferState {
    if matches!(update, Some(phase) if !matches!(phase, CoreUpdatePhase::Done(_))) {
        return OfferState::Tracked;
    }
    if *status == ConnStatus::Ready && server_version.is_some() && endpoint_known {
        OfferState::Offerable
    } else {
        OfferState::Offline
    }
}

/// Tally of [`OfferState`] over a scope's cores, for the server-row control's tooltip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OfferCounts {
    pub(crate) offerable: usize,
    pub(crate) tracked: usize,
    pub(crate) offline: usize,
}

impl OfferCounts {
    /// Fold one more core's classification into the running tally.
    pub(crate) fn add(&mut self, state: OfferState) {
        match state {
            OfferState::Offerable => self.offerable += 1,
            OfferState::Tracked => self.tracked += 1,
            OfferState::Offline => self.offline += 1,
        }
    }
}
