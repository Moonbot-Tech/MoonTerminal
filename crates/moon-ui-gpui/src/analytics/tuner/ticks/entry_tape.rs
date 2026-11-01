//! Which deals the sample owes the tape of their entry order's whole life.
//!
//! A deal's window opens at its entry order's creation (`ticks::model_window`), so the entry
//! model can walk the order from where the core placed it. The exit model starts at the fill
//! and needs none of that stretch. The station and the terminal's close-time capture both record
//! from the fill back by the margin, so an order that waited longer than the margin less the
//! model's run-up leaves the start of the window uncovered on every source: requiring it from a
//! search that does not vary the entry dropped deals whose exit tape was whole.
//!
//! The requirement therefore follows the grid's ticks, per kind whose entry the search varies:
//! a deal of such a kind owes its order's life while an entry field that kind's grid shows and
//! the entry method reads is ticked, or В1 holds a value of one; otherwise only the stretch from
//! the fill is owed, and the deal is judged with the fact's entry. A kind without an entry model
//! never owes it; a kind whose entry has a model the search does not vary always does. Per kind, not per scope: the strategy list's readiness counts every strategy at once,
//! and a scope picked in the table must not move another kind's count.
//!
//! Held for the whole process like the tail floor (`tail.rs`) and the model settings
//! (`model_cfg`): the load, the fetch job and the strategy list's readiness all judge rows, and
//! every one of them reads it here. Part of [`super::signature::SamplePolicy`], so a change
//! re-judges the settled misses and defeats a skipped reload. Put in force when an Analytics
//! window opens, before every tape stage, and on every tick or В1 change; a change of field
//! that leaves every kind's answer as it was changes nothing.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU8, Ordering};

use moon_core::db::tuner::ticks::entry::KIND_MOONSHOT;
use moon_core::db::tuner::ticks::params::{ParamGroup, params_for};

/// The kinds whose entry the search varies (`ticks::entry_model_for`) — the only ones whose
/// grid shows entry fields to tick — one bit each in [`owed`]. Any other kind keeps the full
/// requirement ([`required_for`]).
const ENTRY_KINDS: [&str; 1] = [KIND_MOONSHOT];

/// Every kind with an entry model owes its order's life: the full requirement, in force until
/// a window says otherwise.
pub(in crate::analytics::tuner) const ALL: u8 = (1 << ENTRY_KINDS.len()) - 1;

/// The kinds owing their order's life, as bits over [`ENTRY_KINDS`].
static OWED: AtomicU8 = AtomicU8::new(ALL);

/// The kinds owing their order's life, as bits — what a sample policy is compared by.
pub(in crate::analytics::tuner) fn owed() -> u8 {
    OWED.load(Ordering::Relaxed)
}

/// Whether a deal of `kind` must hold its entry order's whole life. A kind the search never
/// varies the entry of answers `true`: a kind without an entry model is judged with the fact's
/// entry whatever this says, and one with a model of its own keeps the full requirement.
pub(in crate::analytics::tuner) fn required_for(kind: &str) -> bool {
    let owed = owed();
    ENTRY_KINDS
        .iter()
        .position(|k| *k == kind)
        .is_none_or(|bit| owed & (1 << bit) != 0)
}

/// Put a requirement in force; answers whether any kind's answer changed.
pub(in crate::analytics::tuner) fn replace(owed: u8) -> bool {
    OWED.swap(owed, Ordering::Relaxed) != owed
}

/// The kinds whose entry can move off the strategy's, as bits: an entry field the kind's grid
/// shows (`params::params_for`) and the entry method reads is ticked for the search, or В1
/// holds a value of one. An unticked field is held at В1's value where В1 has one, so a typed
/// entry value replays the entry model as surely as a search does. A field of another kind
/// counts for that kind only: one the kind never shows can never be unticked there.
///
/// Args:
///     locked: The unticked fields (`TicksState::locked`).
///     variant: The fields В1 changes (`TicksState::variant_changes`).
pub(in crate::analytics::tuner) fn wanted<'a>(
    locked: &HashSet<String>,
    variant: impl IntoIterator<Item = &'a str>,
) -> u8 {
    let method = super::model_cfg::current().entry_method;
    let variant: HashSet<&str> = variant.into_iter().collect();
    ENTRY_KINDS
        .iter()
        .enumerate()
        .filter(|(_, kind)| {
            params_for(ParamGroup::Entry, kind).any(|p| {
                method.reads(p.key) && (!locked.contains(p.key) || variant.contains(p.key))
            })
        })
        .fold(0, |bits, (bit, _)| bits | (1 << bit))
}

#[cfg(test)]
mod tests;
