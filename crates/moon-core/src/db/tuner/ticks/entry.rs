//! The entry side: "where would the buy order have stood, and which print would have filled
//! it" — one implementation per strategy kind whose entry is a function of the tape alone.
//!
//! A kind is listed here only when its buy order's position can be derived from the prints and
//! the strategy's own parameters, with no order book and no core state. MoonShot qualifies (a
//! limit at a fixed distance below the price, walking a corridor); a Combo, a Strike or a
//! volume detect does not — their entry is a detect the core made on data the tape does not
//! carry, and the axis takes it from the report as it happened. MoonHook (the same corridor,
//! `Hook*` fields) is the first candidate for a second implementation; it adds one arm to
//! [`entry_model_for`] and nothing to the UI.

use super::mshot::MshotEntry;
use super::{Deal, Fill};
use crate::feed::types::Tick;

/// The kind name of MoonShot as the strategy list spells it (`feed::strategies` ordinal 6).
pub const KIND_MOONSHOT: &str = "MoonShot";

/// An entry model: given the tape, where the order would have filled.
pub trait EntryModel {
    /// The fill, or `None` when no print reached the order over the whole tape.
    ///
    /// Args:
    ///     deal: The report row; the model reads its deltas, side and price step.
    ///     ticks: The window's prints, ascending by time.
    ///     start: `(t_ms, price)` the real order was first seen at, from the order archive,
    ///         when known — the model starts there instead of at the tape's first print.
    fn fill(&self, deal: &Deal, ticks: &[Tick], start: Option<(i64, f64)>) -> Option<Fill>;
}

/// Whether the kind has an entry model at all — the UI's "Entry group available" test.
///
/// Args:
///     kind: The strategy kind name as the list carries it.
pub fn entry_model_for(kind: &str) -> bool {
    kind == KIND_MOONSHOT
}

impl EntryModel for MshotEntry<'_> {
    fn fill(&self, deal: &Deal, ticks: &[Tick], start: Option<(i64, f64)>) -> Option<Fill> {
        self.run(deal, ticks, start)
    }
}
