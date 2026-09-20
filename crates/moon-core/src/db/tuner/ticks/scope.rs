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
