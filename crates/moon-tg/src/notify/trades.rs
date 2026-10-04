//! Which closed trades a chat should hear about.

use std::collections::BTreeSet;

use moon_core::db::QuoteCurrency;
use moon_core::telegram::notify::{CoreScope, NotifyLedger, TradeRule};

/// How long an announced close stays in the ledger, in seconds.
pub(crate) const SEEN_WINDOW_SECS: i64 = 72 * 3600;

/// How long a close waits for its dollar value when a threshold needs one, in seconds. After
/// that it is sent with a note that its thresholds were not checked.
pub(crate) const HOLD_SECS: i64 = 300;

/// One closed trade the report reader hands to [`decide`].
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// Entry notional in USDT through the Report's valuation; absent when unusable or unvalued.
    pub volume_usd: Option<f64>,
    /// Report-valued profit in USDT; absent when unvalued.
    pub profit_usd: Option<f64>,
    /// Profit percent already scaled by 100, absent when the row cannot evidence it.
    pub profit_pct: Option<f64>,
    /// The currency the trade's own money is in; absent when the row's quote is unknown.
    pub quote: Option<QuoteCurrency>,
    /// Settled profit in [`Self::quote`], without any valuation.
    pub profit_native: Option<f64>,
    /// Entry notional in [`Self::quote`], where the Report's volume gates prove it.
    pub volume_native: Option<f64>,
    /// Entry price, as the row stored it.
    pub buy_price: Option<f64>,
    /// Exit price, as the row stored it.
    pub sell_price: Option<f64>,
}

impl ClosedTrade {
    /// Whether the trade's own currency is a USD stablecoin, taken 1:1 with the dollar for the
    /// rule thresholds (LinKvo 03.10) — the stablecoin list the Assets panel uses.
    pub(crate) fn stable_quote(&self) -> bool {
        self.quote
            .is_some_and(|quote| moon_core::symbol::is_usd_stable(quote.ticker()))
    }

    /// Profit in dollars for the thresholds: the valuation's, else the stablecoin amount itself.
    pub(crate) fn rule_profit_usd(&self) -> Option<f64> {
        self.profit_usd
            .or_else(|| self.profit_native.filter(|_| self.stable_quote()))
    }

    /// Entry notional in dollars for the volume floor, by the same rule as
    /// [`Self::rule_profit_usd`].
    pub(crate) fn rule_volume_usd(&self) -> Option<f64> {
        self.volume_usd
            .or_else(|| self.volume_native.filter(|_| self.stable_quote()))
    }

    /// The trade's key in the ledger.
    pub(crate) fn key(&self) -> moon_core::telegram::notify::CardKey {
        moon_core::telegram::notify::CardKey {
            core: self.core,
            rec_id: self.rec_id,
        }
    }
}

/// One trade [`decide`] announces.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Announced {
    pub trade: ClosedTrade,
    /// A threshold needed the trade's dollar value and it never came within [`HOLD_SECS`]: the
    /// card is sent anyway and says its thresholds were not checked.
    pub unchecked: bool,
}

/// How a trade fares against the rule's thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail,
    /// A threshold needs a dollar figure the trade does not have yet.
    Unknown,
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

/// When the earliest held close stops waiting for its dollar value, if any close is held.
///
/// Args:
///     ledger: Announce-once state. Only `held` is read.
///
/// Returns:
///     UTC seconds; a read at or after it sends that close unchecked.
pub(crate) fn hold_until(ledger: &NotifyLedger) -> Option<i64> {
    ledger
        .held
        .values()
        .flat_map(|rows| rows.values())
        .min()
        .map(|first| first.saturating_add(HOLD_SECS))
}

/// Trades to announce now, oldest close first.
///
/// A trade outside the enable floor, the visible cores, or `rule.cores` is
/// left unmarked. A trade a threshold cannot judge yet — no dollar figure for
/// it — is held in `ledger.held`, unmarked, for [`HOLD_SECS`] from the first
/// read that saw it; once that passes it is announced unchecked. Anything else
/// is stored in `ledger.seen` even when a volume or profit filter suppresses
/// the card, so a later tick does not reconsider it. `rule.on == false` forgets
/// `seen` and `held` and announces nothing.
///
/// Args:
///     rule: The chat's trade card rule.
///     ledger: Announce-once state. `seen` and `held` are updated in place.
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
) -> Vec<Announced> {
    if !rule.on {
        ledger.seen.clear();
        ledger.held.clear();
        return Vec::new();
    }
    let Some(read_from) = read_from_utc(ledger, now_utc) else {
        ledger.prune_seen(now_utc.saturating_sub(SEEN_WINDOW_SECS));
        return Vec::new();
    };
    let mut announced = Vec::new();
    // Held closes this read still offers; any other is let go below, so a close whose core left
    // the chat, or whose row is gone, cannot keep forcing reads until the window drops it.
    let mut offered: BTreeSet<(u64, i64)> = BTreeSet::new();
    for trade in trades {
        if trade.close_utc < read_from || !core_selected(rule, visible, trade.core) {
            continue;
        }
        offered.insert((trade.core, trade.rec_id));
        if ledger
            .seen
            .get(&trade.core)
            .is_some_and(|rows| rows.contains_key(&trade.rec_id))
        {
            continue;
        }
        let verdict = verdict(rule, trade);
        if verdict == Verdict::Unknown {
            let first = *ledger
                .held
                .entry(trade.core)
                .or_default()
                .entry(trade.rec_id)
                .or_insert(now_utc);
            if now_utc < first.saturating_add(HOLD_SECS) {
                continue;
            }
        }
        unhold(ledger, trade);
        ledger
            .seen
            .entry(trade.core)
            .or_default()
            .insert(trade.rec_id, trade.close_utc);
        if verdict != Verdict::Fail {
            announced.push(Announced {
                trade: trade.clone(),
                unchecked: verdict == Verdict::Unknown,
            });
        }
    }
    announced.sort_by(|left, right| {
        left.trade
            .close_utc
            .cmp(&right.trade.close_utc)
            .then(left.trade.rec_id.cmp(&right.trade.rec_id))
    });
    let window_edge = now_utc.saturating_sub(SEEN_WINDOW_SECS);
    ledger.prune_seen(window_edge);
    ledger.held.retain(|core, rows| {
        rows.retain(|rec_id, _| offered.contains(&(*core, *rec_id)));
        !rows.is_empty()
    });
    announced
}

/// Drop `trade` from `ledger.held`, and its core when that empties it.
fn unhold(ledger: &mut NotifyLedger, trade: &ClosedTrade) {
    if let Some(rows) = ledger.held.get_mut(&trade.core) {
        rows.remove(&trade.rec_id);
        if rows.is_empty() {
            ledger.held.remove(&trade.core);
        }
    }
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

/// Volume floor and profit/loss floor. Both must pass; one known failure fails the trade even
/// while the other is still unknown.
///
/// Only a figure the valuation can still supply is worth waiting for: the trade's own amount
/// exists and is just not in dollars yet. A figure the row cannot evidence in any currency — a
/// volume the Report's gates cannot prove, a profit the row does not carry — fails a set
/// threshold at once, as it always did.
fn verdict(rule: &TradeRule, trade: &ClosedTrade) -> Verdict {
    let volume = volume_passes(
        rule.min_volume_usd,
        trade.rule_volume_usd(),
        trade.volume_native.is_some(),
    );
    let profit = profit_passes(
        rule.profit_at_least_usd,
        rule.loss_at_least_usd,
        trade.rule_profit_usd(),
        trade.profit_native.is_some(),
    );
    match (volume, profit) {
        (Some(false), _) | (_, Some(false)) => Verdict::Fail,
        (Some(true), Some(true)) => Verdict::Pass,
        _ => Verdict::Unknown,
    }
}

/// `None` minimum applies no floor. A set minimum requires a volume at or above it; `None` when
/// the volume exists but is not known in dollars yet (`pending`), a failure when it does not
/// exist at all.
fn volume_passes(minimum: Option<f64>, volume: Option<f64>, pending: bool) -> Option<bool> {
    match (minimum, volume) {
        (None, _) => Some(true),
        (Some(minimum), Some(volume)) => Some(volume >= minimum),
        (Some(_), None) => (!pending).then_some(false),
    }
}

/// Both thresholds unset accepts every row, including one without a dollar profit.
///
/// Otherwise the row passes when its profit is at least `profit_at_least` or
/// its loss (`-profit`) is at least `loss_at_least`; `None` when the profit
/// exists but is not known in dollars yet (`pending`), a failure when it does
/// not exist at all.
fn profit_passes(
    profit_at_least: Option<f64>,
    loss_at_least: Option<f64>,
    profit: Option<f64>,
    pending: bool,
) -> Option<bool> {
    if profit_at_least.is_none() && loss_at_least.is_none() {
        return Some(true);
    }
    let Some(profit) = profit else {
        return (!pending).then_some(false);
    };
    let profit_hit = profit_at_least.is_some_and(|floor| profit >= floor);
    let loss_hit = loss_at_least.is_some_and(|floor| -profit >= floor);
    Some(profit_hit || loss_hit)
}

#[cfg(test)]
mod tests;
