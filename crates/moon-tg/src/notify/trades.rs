//! Which closed trades a chat should hear about.

use moon_core::telegram::notify::{CoreScope, NotifyLedger, TradeRule};

/// How long an announced close stays in the ledger, in seconds.
pub(crate) const SEEN_WINDOW_SECS: i64 = 72 * 3600;

/// One closed trade the report reader hands to [`decide`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClosedTrade {
    /// Core that closed the trade.
    pub core: u64,
    /// Report row id. Unique per core.
    pub rec_id: i64,
    /// Close time, UTC Unix seconds.
    pub close_utc: i64,
    /// Market symbol.
    pub coin: String,
    /// Configured core name.
    pub core_name: String,
    /// Strategy name stored on the row.
    pub strategy: String,
    /// Entry notional in USDT for the USD-named rule threshold; absent when unusable.
    pub volume_usd: Option<f64>,
    /// Report-valued profit in USDT for the USD-named rule thresholds; absent when unvalued.
    pub profit_usd: Option<f64>,
    /// Profit percent already scaled by 100, absent when unvalued.
    pub profit_pct: Option<f64>,
    /// Open time, UTC Unix seconds.
    pub open_utc: i64,
}

/// Earliest close time `decide` may consider.
///
/// Args:
///     ledger: Announce-once state. Only `trades_enabled_utc` is read.
///     now_utc: Current UTC Unix seconds.
///
/// Returns:
///     `None` when the trade rule has never been enabled. Otherwise the later
///     of the enable moment and the 72-hour window edge. The subtraction
///     saturates at `i64::MIN`.
pub(crate) fn read_from_utc(ledger: &NotifyLedger, now_utc: i64) -> Option<i64> {
    let enabled = ledger.trades_enabled_utc?;
    Some(enabled.max(now_utc.saturating_sub(SEEN_WINDOW_SECS)))
}

/// Trades to announce now, oldest close first.
///
/// A trade outside the enable floor, the visible cores, or `rule.cores` is
/// left unmarked. Anything else is stored in `ledger.seen` even when a volume
/// or profit filter suppresses the card, so a later tick does not reconsider
/// it. `rule.on == false` forgets `seen` and announces nothing.
///
/// Args:
///     rule: The chat's trade card rule.
///     ledger: Announce-once state. `seen` is updated in place.
///     visible: Core ids this chat may see.
///     trades: Closed rows from the report. Order does not matter.
///     now_utc: Current UTC Unix seconds.
///
/// Returns:
///     The trades that pass the filters, sorted by `close_utc` then `rec_id`.
///     Empty when the rule is off, the enable moment is missing, or nothing
///     qualifies. No length cap.
pub(crate) fn decide(
    rule: &TradeRule,
    ledger: &mut NotifyLedger,
    visible: &[u64],
    trades: &[ClosedTrade],
    now_utc: i64,
) -> Vec<ClosedTrade> {
    if !rule.on {
        ledger.seen.clear();
        return Vec::new();
    }
    let Some(read_from) = read_from_utc(ledger, now_utc) else {
        ledger.prune_seen(now_utc.saturating_sub(SEEN_WINDOW_SECS));
        return Vec::new();
    };
    let mut announced = Vec::new();
    for trade in trades {
        if trade.close_utc < read_from || !core_selected(rule, visible, trade.core) {
            continue;
        }
        if ledger
            .seen
            .get(&trade.core)
            .is_some_and(|rows| rows.contains_key(&trade.rec_id))
        {
            continue;
        }
        ledger
            .seen
            .entry(trade.core)
            .or_default()
            .insert(trade.rec_id, trade.close_utc);
        if filters_pass(rule, trade) {
            announced.push(trade.clone());
        }
    }
    announced.sort_by(|left, right| {
        left.close_utc
            .cmp(&right.close_utc)
            .then(left.rec_id.cmp(&right.rec_id))
    });
    ledger.prune_seen(now_utc.saturating_sub(SEEN_WINDOW_SECS));
    announced
}

/// `true` when `core` is both visible to the chat and selected by the rule.
fn core_selected(rule: &TradeRule, visible: &[u64], core: u64) -> bool {
    if !visible.contains(&core) {
        return false;
    }
    match &rule.cores {
        CoreScope::All => true,
        CoreScope::Only(ids) => ids.contains(&core),
    }
}

/// Volume floor and profit/loss floor. Both must pass.
fn filters_pass(rule: &TradeRule, trade: &ClosedTrade) -> bool {
    volume_passes(rule.min_volume_usd, trade.volume_usd)
        && profit_passes(
            rule.profit_at_least_usd,
            rule.loss_at_least_usd,
            trade.profit_usd,
        )
}

/// `None` minimum applies no floor. A set minimum requires a volume at or above it.
fn volume_passes(minimum: Option<f64>, volume: Option<f64>) -> bool {
    match minimum {
        None => true,
        Some(minimum) => volume.is_some_and(|volume| volume >= minimum),
    }
}

/// Both thresholds unset accepts every row, including an unvalued profit.
///
/// Otherwise the row passes when its profit is at least `profit_at_least` or
/// its loss (`-profit`) is at least `loss_at_least`. A missing profit fails
/// as soon as either threshold is set.
fn profit_passes(
    profit_at_least: Option<f64>,
    loss_at_least: Option<f64>,
    profit: Option<f64>,
) -> bool {
    if profit_at_least.is_none() && loss_at_least.is_none() {
        return true;
    }
    let Some(profit) = profit else {
        return false;
    };
    let profit_hit = profit_at_least.is_some_and(|floor| profit >= floor);
    let loss_hit = loss_at_least.is_some_and(|floor| -profit >= floor);
    profit_hit || loss_hit
}

#[cfg(test)]
mod tests;
