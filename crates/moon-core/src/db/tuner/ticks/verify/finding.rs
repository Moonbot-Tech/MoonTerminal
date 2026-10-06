//! Why a verdict came out as it did — the cause behind every ✗ and ·, kept beside the
//! `Option<bool>` the shares count.
//!
//! The ✓ shares say HOW MUCH of a sample the model reproduces; they cannot say where the rest
//! goes. On the developer's own cores the exit runs at ~90 %, and a user who reads 40 % on theirs
//! could be missing on the moment of a stop, on the level of a line, on a take the record did
//! not keep, or on a rule nobody here runs. The findings below are the verdict's own branches,
//! named, so a report can count them per segment without carrying a single trade
//! (`ticks::diag`). Each is built in the same place as the verdict it explains, and the verdict
//! is read back off it ([`EntryFinding::verdict`], [`ExitFinding::verdict`]), so the two cannot
//! disagree.

use super::super::exit::UnmodelledRule;
use super::super::exit::line::LinePoint;
use super::super::settings::ModelSettings;
use super::super::{Deal, EntryParams, ExitParams};
use super::{deviation_pct, same_move};
use crate::feed::types::Tick;

/// How far from the report's fill stamp a print at the fill's price is looked for when the
/// stamp is held against the tape ([`fill_clock_ms`]). Wide enough for a core whose clock runs
/// seconds off the exchange's to still be measured, narrow enough that a print at the same price
/// a minute away is not taken for the fill.
pub const FILL_CLOCK_WINDOW_MS: i64 = 10_000;

/// What the entry group found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryFinding {
    /// The kind has no entry model; the entry was taken from the fact.
    Fact,
    /// Reproduced within the corridor's tolerance.
    Hit,
    /// The modelled order never filled on the tape.
    Unfilled,
    /// Filled, outside the tolerance — `Verdict::entry_dev_pct` says by how much and which way.
    Off,
}

impl EntryFinding {
    /// The verdict this finding stands for: `None` unmodelled, `Some(true)` reproduced.
    pub fn verdict(self) -> Option<bool> {
        match self {
            Self::Fact => None,
            Self::Hit => Some(true),
            Self::Unfilled | Self::Off => Some(false),
        }
    }
}

/// What the exit group found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitFinding {
    /// Reproduced.
    Hit,
    /// Judged and missed.
    Miss(ExitMiss),
    /// Not judged — the trade says nothing about the rules the model has.
    Unjudged(Unjudged),
}

impl ExitFinding {
    /// The verdict this finding stands for: `None` unjudged, `Some(true)` reproduced.
    pub fn verdict(self) -> Option<bool> {
        match self {
            Self::Hit => Some(true),
            Self::Miss(_) => Some(false),
            Self::Unjudged(_) => None,
        }
    }
}

/// How a judged exit missed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitMiss {
    /// The core's exit was a stop; the model, holding a stop of its own, never fired it — its line
    /// sold first, or its stop never saw the price the core's did.
    StopNotFired,
    /// No level of the model stood at the close: the walk ran out of tape still holding.
    NoLevel,
    /// The model closed by the same rule and landed elsewhere; which parts of it disagreed.
    Off(MissParts),
}

/// The parts of a same-rule exit that disagreed with the fact. At least one of them is set: a
/// miss with every part agreeing would have been a hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissParts {
    /// The level at the close — or the stop's level against the one the core printed — sat
    /// outside the price tolerance; `Verdict::exit_dev_pct` holds the deviation when it could be
    /// formed.
    pub level: bool,
    /// The stop fired at a different moment than the core's: the model's firing minus the
    /// core's activation, milliseconds (positive — the model fired later). `None` when the
    /// moment agreed, and for every non-stop exit, whose moment is the core's close.
    pub late_ms: Option<i64>,
    /// The first archived move of the core's sell line the model did not re-place, as an index
    /// into the moves (0 — the take as placed). `None` when every move was re-placed or the
    /// archive holds no line.
    pub first_unmatched: Option<usize>,
}

/// Why an exit was not judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unjudged {
    /// The strategy ran a sell rule the model does not have.
    Rule(UnmodelledRule),
    /// The rule followed the price through a hole in the tape.
    InGap,
    /// The take a variant would place is unknown — the line under it would be invented.
    TakeUnknown,
    /// The core closed by a rule other than the one the model's line was under at the close: the
    /// two prices are not comparable.
    OtherRule,
}

/// How many flags [`RuleFlags::named`] lists — the length of every per-flag count, so the list
/// and the counters cannot differ in length. A new field of [`RuleFlags`] reaches the report
/// only once it is named there too.
pub const RULE_FLAG_COUNT: usize = 14;

/// The rules a trade's strategy ran with, as the report counts them per segment. Only whether
/// each is ON — never its values: those are the user's settings, and the report stays a count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuleFlags {
    /// A stop-loss is armed.
    pub stop: bool,
    /// The stop watches the trades (`FastStopLoss`), not the REST ticker.
    pub fast_stop: bool,
    /// A second or third stop moves the stop (the ladder).
    pub ladder: bool,
    /// A trailing stop is on.
    pub trailing: bool,
    /// PriceDown steps the sell line.
    pub price_down: bool,
    /// SellLevel steps the sell line.
    pub sell_level: bool,
    /// PumpsDetection's one pump move.
    pub pump_move: bool,
    /// `SellDelay` holds the sell back.
    pub sell_delay: bool,
    /// MoonShot lifts the take to the pre-spike price (`MShotSellAtLastPrice`) — counted on the
    /// kinds that read the field only (`lifts_take_to_ask`).
    pub sell_at_last: bool,
    /// `SellModifier` moves the sell by the delta sum.
    pub sell_modifier: bool,
    /// `StopLossModifier` moves the stop by the delta sum.
    pub stop_modifier: bool,
    /// The sell side's `Add*Delta` terms are set.
    pub delta_terms: bool,
    /// MoonShot's `MShotAdd*` delta terms move the entry corridor.
    pub corridor_terms: bool,
    /// A price-bug term (`AddPriceBug`, `MShotAddPriceBug`) is set, on either side.
    pub price_bug: bool,
}

impl RuleFlags {
    /// The flags of one parameter set.
    ///
    /// Args:
    ///     entry: The entry parameters the trade ran with.
    ///     exit: The sell-line parameters the trade ran with.
    ///     kind: The trade's strategy kind — a field read only by some kinds counts only there.
    pub fn of(entry: &EntryParams, exit: &ExitParams, kind: &str) -> Self {
        use super::super::exit::delta_mods::has_delta_terms;
        let stop = exit.stop_loss_pct != 0.0;
        // MoonShot's corridor modifiers; no other kind has an entry model to carry them.
        let corridor = match entry {
            EntryParams::MoonShot(params) => Some(&params.modifiers),
            EntryParams::Fact => None,
        };
        Self {
            stop,
            fast_stop: stop && exit.fast_stop_loss,
            ladder: exit.second_stop.is_some() || exit.third_stop.is_some(),
            trailing: exit.trailing_pct != 0.0,
            price_down: exit.price_down_on(),
            sell_level: exit.sell_level_on(),
            pump_move: exit.pump_move_timer_s > 0.0,
            sell_delay: exit.sell_delay_ms > 0.0,
            sell_at_last: super::super::exit::sell_order::lifts_take_to_ask(exit, kind),
            sell_modifier: exit.sell_modifier != 0.0,
            stop_modifier: exit.stop_loss_modifier != 0.0,
            delta_terms: has_delta_terms(&exit.sell_mods),
            corridor_terms: corridor.is_some_and(has_delta_terms),
            price_bug: exit.sell_mods.add_pricebug != 0.0
                || corridor.is_some_and(|m| m.add_pricebug != 0.0),
        }
    }

    /// Every flag with the name the report prints it under, in a fixed order.
    pub fn named(&self) -> [(&'static str, bool); RULE_FLAG_COUNT] {
        [
            ("stop", self.stop),
            ("fast stop", self.fast_stop),
            ("stop ladder", self.ladder),
            ("trailing", self.trailing),
            ("PriceDown", self.price_down),
            ("SellLevel", self.sell_level),
            ("PumpMove", self.pump_move),
            ("SellDelay", self.sell_delay),
            ("SellAtLastPrice", self.sell_at_last),
            ("SellModifier", self.sell_modifier),
            ("StopLossModifier", self.stop_modifier),
            ("Add* terms", self.delta_terms),
            ("MShotAdd* terms", self.corridor_terms),
            ("price-bug term", self.price_bug),
        ]
    }
}

/// The first archived move no modelled point re-placed, as an index into `archived`; `None`
/// when every move was matched.
///
/// Args:
///     modelled: Every level the modelled line stood at.
///     archived: The core's moves, in time order.
///     model: The model's settings, for the tolerances.
pub(super) fn first_unmatched(
    modelled: &[LinePoint],
    archived: &[(i64, f64)],
    model: &ModelSettings,
) -> Option<usize> {
    archived
        .iter()
        .position(|&point| !modelled.iter().any(|m| same_move(m, point, model)))
}

/// The report's fill stamp held against the tape: `buydatems` minus the time of the nearest
/// print at the fill's price within [`FILL_CLOCK_WINDOW_MS`], milliseconds — positive when the
/// stamp is later than the tape. `None` when no print at that price lies in the window.
///
/// The stamp is the core's clock and the tape the exchange's. Every moment the verdict judges —
/// a PriceDown step, a stop's firing, the entry's re-place — is the one against the other, so a
/// core whose clock runs a second off misses on every moment at once while its levels are
/// right. One trade says little: several prints share a price, and the nearest one in time is a
/// bound, not the fill. A core's median over its trades is what the report prints.
///
/// Args:
///     deal: The report row — its fill stamp and price.
///     ticks: The window's prints, ascending.
///     price_pct: How far a print may sit from the fill's price, per cent.
pub fn fill_clock_ms(deal: &Deal, ticks: &[Tick], price_pct: f64) -> Option<i64> {
    let from = deal.buy_ms - FILL_CLOCK_WINDOW_MS;
    let to = deal.buy_ms + FILL_CLOCK_WINDOW_MS;
    let start = ticks.partition_point(|t| (t.time_ms as i64) < from);
    ticks[start..]
        .iter()
        .take_while(|t| (t.time_ms as i64) <= to)
        .filter(|t| {
            deviation_pct(f64::from(t.price), deal.buy_price).is_some_and(|d| d.abs() <= price_pct)
        })
        .map(|t| deal.buy_ms - t.time_ms as i64)
        .min_by_key(|dt| dt.abs())
}

#[cfg(test)]
mod tests;
