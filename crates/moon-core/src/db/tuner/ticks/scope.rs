//! Which report rows are TRADES the axis can read, as opposed to rows the report holds for
//! money's sake.
//!
//! The unified tuner source hands over every closed row of the scope — the same set the "Fact"
//! column sums — and money-wise that is right: a funding charge, a liquidation and a joined sell
//! all carry profit. None of them is a trade the tape can explain, though: a funding row has no
//! entry and no exit, a liquidation's exit is the exchange's, a joined sell closes several buys
//! at once, and a row with no strategy behind it (`strategyid = 0`: manual, `SellFromAssets`)
//! has no parameters to run a model with. Those are the SERVICE rows, and the axis never takes
//! them. Measured on the live replica (2026-09-20): 16 of 1217 rows with millisecond stamps.
//!
//! The axis narrows further, always (the developer's call, 2026-09-20: a trade the tuner cannot
//! be run on is noise in its table), to what the tuner can actually be RUN on: a kind whose
//! trades close on a sell line the exit model has
//! (every trading kind; a `Manual`, `Alerts` or `Watcher` strategy is a container, not a rule),
//! resolved in `strategies.sqlite` (a kind the database does not know has no versions to take
//! the parameters as of the buy from), and an exit the strategy made itself — a manual sell is
//! a fact the model can only ever miss, and a miss the sample is then judged by.

/// `sellreason` values of the service rows; matched exactly, as the core writes them.
pub const SERVICE_SELL_REASONS: [&str; 3] = ["Funding", "LIQUIDATION", "JoinedSell"];

/// Strategy kinds (`SignalType`) that hold no trading rule of their own.
const CONTAINER_KINDS: [&str; 3] = ["Manual", "Alerts", "Watcher"];

/// How much more than it bought a row may sell and still be one position: the fraction the
/// exchange's own rounding of a lot can add. Anything past it is coins the core topped the sale
/// up with from the wallet balance.
const SOLD_OVER_BOUGHT_EPS: f64 = 1e-6;

/// Whether the row sold MORE coins than it bought.
///
/// On spot, a position that comes out under the exchange's minimum lot is topped up from the
/// wallet balance, and the sale then covers coins this trade never bought: its `sellprice` is an
/// average over a different amount, and the level the model is judged against is not the level
/// the rule placed. Such a row is excluded like a manual sell — the tape cannot explain it.
///
/// Measured on this machine's replica (2026-09-22): 1 row of 606 767 by this signature, so the
/// exclusion costs nothing here and matters on a spot core that does it often. The opposite
/// direction — selling slightly LESS — is ordinary: the fee is taken in coin, and 2 242 rows sit
/// a fraction below their bought amount.
///
/// Args:
///     quantity: The row's `quantity` — what the sale moved.
///     bought: The row's `boughtq` — what the entry filled.
pub fn sold_more_than_bought(quantity: f64, bought: f64) -> bool {
    quantity.is_finite()
        && bought.is_finite()
        && bought > 0.0
        && quantity > bought * (1.0 + SOLD_OVER_BOUGHT_EPS)
}

/// Whether a report row is a service row — never a deal of the axis.
///
/// Args:
///     strategy_id: The row's effective `strategyid`; `0` is "no strategy".
///     sell_reason: The row's `sellreason`, as stored.
pub fn is_service_row(strategy_id: i64, sell_reason: &str) -> bool {
    strategy_id == 0 || SERVICE_SELL_REASONS.contains(&sell_reason)
}

/// Whether a strategy kind is one the tuner can be run on — see the module doc.
///
/// Args:
///     kind: The kind as `strategies.sqlite` spells it; empty when unresolved.
pub fn tunable_kind(kind: &str) -> bool {
    !kind.is_empty() && !CONTAINER_KINDS.contains(&kind)
}

/// Whether the exit was the operator's, not the strategy's: the `Manual …` family and a sell
/// from the Assets panel.
///
/// Args:
///     sell_reason: The row's `sellreason`, as stored.
pub fn is_manual_exit(sell_reason: &str) -> bool {
    sell_reason.starts_with("Manual") || sell_reason == "SellFromAssets"
}

/// Whether a resolved deal is one the tuner can be run on: a tunable kind that closed by its
/// own rule.
///
/// Args:
///     kind: The strategy kind, resolved.
///     sell_reason: The row's `sellreason`.
pub fn is_tunable(kind: &str, sell_reason: &str) -> bool {
    tunable_kind(kind) && !is_manual_exit(sell_reason)
}

#[cfg(test)]
mod tests;
