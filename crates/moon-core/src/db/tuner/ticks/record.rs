//! The core's own record of a trade — the report row and the order archive — turned into the
//! model's inputs, and the rule for which trades a search may be run on.
//!
//! One place for both callers: the axis' load (`moon-ui-gpui`, `ticks/load.rs`) and the
//! `tests::real_data` bench prepare every deal through [`prepare_deal`], so the bench measures
//! what the table shows.
//!
//! **What the fact proves about the stop** ([`StopAnchor`]). The stop watches the book — the
//! BID, averaged, or the price our size would sell at — and the tape carries no book, so the
//! model fires it by a proxy that reproduces about half of the book stops (2026-09-23: 83 of
//! 173). Its sale is a panic limit or a market order walked through that book, 0.4–6 % past
//! the trigger print. Neither is
//! a guess on the trade itself: the core's stop, under its own settings, fired when the core's
//! record says and sold at the report's price, and did NOT fire before — before its activation
//! when the trade was stopped, before the close when it was not. A variant that keeps the
//! entry and every stop setting inherits exactly that; one that changes either is back on the
//! proxy, and the verdict (which replays the proxy, never the anchor) is what says how far the
//! proxy may be trusted.

use super::exit::{ExitParams, archived_pre_spike_ask, archived_take, stop_pct};
use super::verify::{
    POINT_TIME_TOLERANCE_MS, REASON_STOP, Verdict, archived_stop_jump, reason_starts_with,
    stated_stop_level,
};
use super::{Deal, EntryParams, Fill};

/// The fact's stop, as a variant running the same one may lean on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StopAnchor {
    /// The entry the fact's stop counted from — the buy's price and moment.
    pub entry_price: f64,
    pub entry_ms: i64,
    /// The stop the fact ran: its adjusted distance ([`stop_pct`]), delay and trigger.
    pub stop_pct: f64,
    pub delay_s: f64,
    pub fast: bool,
    pub ema: f64,
    /// When the fact's stop fired and the price it sold at; `None` when the fact did not stop.
    pub fired: Option<(i64, f64)>,
    /// Up to when the fact proves the stop quiet: its activation, or the close.
    pub quiet_until_ms: i64,
}

impl StopAnchor {
    /// The anchor of a trade whose fact ran `exit`.
    ///
    /// The moment a stopped trade's stop fired: the order archive's first move at or past the
    /// stop's level (the panic sell's jump), else the close — the sale completes within a second
    /// or two of the activation, and on a trade with no record of the moment that is the nearest
    /// the fact gets.
    ///
    /// Args:
    ///     deal: The trade.
    ///     exit: The sell parameters of the fact.
    ///     exit_points: The archived Exit line, when the archive holds it.
    pub fn of(deal: &Deal, exit: &ExitParams, exit_points: Option<&[(i64, f64)]>) -> Self {
        let pct = stop_pct(exit, deal);
        // The verdict's own test of a stopped fact (`verify::reason_starts_with`), not a copy.
        let stopped = reason_starts_with(deal.sell_reason.trim(), REASON_STOP);
        let fired = stopped.then(|| {
            // The level the jump is read against: the core's own when its reason kept it.
            let level = stated_stop_level(&deal.sell_reason).unwrap_or(if deal.is_long() {
                deal.buy_price * (1.0 + pct / 100.0)
            } else {
                deal.buy_price * (1.0 - pct / 100.0)
            });
            let at = archived_stop_jump(deal, level, exit_points).unwrap_or(deal.close_ms);
            (at, deal.sell_price)
        });
        Self {
            entry_price: deal.buy_price,
            entry_ms: deal.buy_ms,
            stop_pct: pct,
            delay_s: exit.stop_loss_delay_s,
            fast: exit.fast_stop_loss,
            ema: exit.stop_loss_ema,
            fired,
            quiet_until_ms: fired.map_or(deal.close_ms, |(t, _)| t),
        }
    }

    /// Whether a walk from `fill` under `params` runs the fact's own stop — the same entry (to
    /// the price, and within the point tolerance in time) and the same stop settings.
    ///
    /// Args:
    ///     deal: The trade, for the adjusted stop distance.
    ///     fill: The walk's entry.
    ///     params: The walk's sell parameters.
    pub fn holds(&self, deal: &Deal, fill: Fill, params: &ExitParams) -> bool {
        fill.price == self.entry_price
            && (fill.t_ms - self.entry_ms).abs() <= POINT_TIME_TOLERANCE_MS
            && stop_pct(params, deal) == self.stop_pct
            && params.stop_loss_delay_s == self.delay_s
            && params.fast_stop_loss == self.fast
            && params.stop_loss_ema == self.ema
    }
}

/// Fill the model inputs the core's own record gives: the ask a MoonShot's take was lifted to,
/// read back off the take as placed, the take itself, the stop anchor, and the entry settings
/// the trade ran with ([`Deal::own_entry`]).
///
/// Args:
///     deal: The trade, filled in place.
///     entry: The entry parameters as of the buy.
///     exit: The sell parameters as of the buy.
///     exit_points: The archived Exit line, when the archive holds it.
pub fn prepare_deal(
    deal: &mut Deal,
    entry: &EntryParams,
    exit: &ExitParams,
    exit_points: Option<&[(i64, f64)]>,
) {
    deal.pre_spike_ask = archived_pre_spike_ask(exit_points, exit, deal.is_short);
    deal.archived_take = archived_take(exit_points);
    deal.stop_anchor = Some(StopAnchor::of(deal, exit, exit_points));
    deal.own_entry = Some(entry.clone());
}

/// The deal as the verdict replays it: without what the fact proves ([`Deal::stop_anchor`],
/// [`Deal::own_entry`]) — the verdict exists to test the model, and a model handed the fact
/// passes by construction.
pub fn unanchored(deal: &Deal) -> Deal {
    Deal {
        stop_anchor: None,
        own_entry: None,
        ..deal.clone()
    }
}

/// Whether a trade may be searched over: the model reproduced it — the exit judged and right,
/// the entry right or taken from the fact.
///
/// A trade the model does not reproduce under the strategy's own parameters is one whose
/// behaviour it cannot model — a book it has no copy of, a rule it does not have, an input the
/// record did not keep — and what it answers for a variant of that trade is not an answer. The
/// developer's call (2026-09-23): such trades stay in the table with their verdict, and out of
/// the variants and the search. An exit left unjudged (`None`) is out too: it is a rule the
/// model does not have, not a pass.
///
/// Args:
///     verdict: The trade's verdict on its own parameters.
pub fn fit_for_search(verdict: &Verdict) -> bool {
    verdict.entry != Some(false) && verdict.exit == Some(true)
}

#[cfg(test)]
mod tests;
