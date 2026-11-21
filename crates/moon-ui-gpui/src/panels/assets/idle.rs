//! Money the bots cannot trade with, marked on the wallets core list.
//!
//! A core trades from ONE wallet of its exchange account — the futures wallet of a futures core,
//! the Unified Trading account of a Bybit core — and money sitting in any other wallet of that
//! account (spot on a futures account, Bybit Funding, Binance COIN-M) does nothing until someone
//! moves it. To find it, a user had to click every core and read its containers. The panel already
//! holds every scoped core's wallets — it asks each core once when it opens — so the core list can
//! say it directly: a quiet `Funding 120.00$` right after the name of the core whose account holds
//! it. The row click opens that core's containers as before; the transfer itself stays manual.
//!
//! Cores on one exchange account read the same wallets ([`moon_core::venue::wallets_key`]), so an
//! account is judged once:
//! - against the union of the wallets ALL its cores trade from, visible or not — a spot core and a
//!   futures core on one Gate account trade from both wallets, and judging the visible spot core
//!   alone would mark the other core's working margin as idle;
//! - on its newest snapshot (`CoreData::transfer_at_ms`) — a transfer refreshes only the core it
//!   was made from, and a sibling's older snapshot would keep showing money already moved;
//! - and marked once, on its first core the panel shows — the same Funding balance on two rows
//!   would read as twice the money.
//!
//! Cores are known to share an account only once each has stated its identity (`AuthCheck`). A
//! sibling that is offline or still connecting is not part of the account yet, so until it
//! connects the wallet it trades from can carry a mark.
//!
//! The footer names the same money as one sum (`assets.balances_tip_idle`), and there it follows
//! the footer's own scope: an account whose shown cores are all set to stay out of the total
//! (`TotalMode::Exclude`) keeps its mark here but stays out of that sum ([`footer_idle_total`]) —
//! otherwise «Not traded» and «Combined» would add money the total itself leaves out.
//!
//! Only exchanges whose wallet layout is verified are marked: Binance, Bybit (a Bybit core always
//! runs the Unified Trading layout, see `WalletKind::label_key`) and Gate. Elsewhere which wallet a
//! core trades from is not known, and a false "this money is idle" is worse than none.

use moon_core::feed::{TransferAssetsSnapshot, WalletKind};
use moon_core::session::CoreId;
use moon_core::venue::{Brand, MarketKind, Venue, WalletAccountKey};
use std::collections::HashMap;

/// One configured core's inputs, in canonical core order. Borrows the store's snapshot: this runs
/// on every cache rebuild, and copying every core's wallet lists there would be wasted work.
pub(super) struct IdleInput<'a> {
    /// Core id.
    pub(super) core: CoreId,
    /// Whether the panel shows this core, so it may carry its account's mark.
    pub(super) visible: bool,
    /// Whether the footer total counts this core: the panel shows it and its total setting is
    /// not `TotalMode::Exclude`.
    pub(super) counted: bool,
    /// Resolved venue, or `None` while unknown.
    pub(super) venue: Option<Venue>,
    /// Exchange account the wallets belong to, or `None` (the core is its own account).
    pub(super) key: Option<WalletAccountKey>,
    /// Latest wallet snapshot and the unix-ms it arrived, or `None` before the first answer.
    pub(super) wallets: Option<(&'a TransferAssetsSnapshot, i64)>,
}

/// One wallet holding money the account's cores do not trade with.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct IdleWallet {
    /// Wallet code.
    pub(super) kind: WalletKind,
    /// USDT value of its coins at or above the dust threshold.
    pub(super) value: f64,
    /// `(coin, USDT value)` of those coins, largest first.
    pub(super) coins: Vec<(String, f64)>,
}

/// Mark for one core row: every idle wallet of its account.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct IdleMark {
    /// The account's wallets holding unused money, in `WalletKind::ALL` order.
    pub(super) wallets: Vec<IdleWallet>,
    /// Unix-ms the snapshot these figures come from arrived, so the hover can say how old they
    /// are: wallets are fetched when the panel opens and on a refresh, never on their own.
    pub(super) as_of_ms: i64,
    /// Whether the footer total counts the account: some core of it is [`IdleInput::counted`].
    /// Only those marks enter the footer's sum ([`footer_idle_total`]).
    pub(super) in_total: bool,
}

/// The wallet code a core trades from, on an exchange whose layout is verified.
///
/// A Bybit core of either market trades from its Unified Trading account, which the core reports
/// under the `Spot` code; on Binance and Gate the code follows the market.
///
/// Returns:
///     The trading wallet, or `None` on an exchange this module does not mark.
pub(super) fn trading_wallet(venue: Venue) -> Option<WalletKind> {
    match venue.brand {
        Brand::Bybit => Some(WalletKind::Spot),
        Brand::Binance | Brand::Gate => Some(match venue.kind {
            MarketKind::Spot => WalletKind::Spot,
            MarketKind::Futures => WalletKind::Futures,
            MarketKind::Quarterly => WalletKind::Quarterly,
        }),
        Brand::Htx | Brand::BitGet | Brand::Hyperliquid | Brand::Okx => None,
    }
}

/// Find every account's idle money and attach it to the account's first visible core.
///
/// An account with a core of unknown or unverified venue is skipped: without knowing what that
/// core trades from, any wallet might be its trading wallet.
///
/// Args:
///     inputs: Every configured core, in canonical order.
///     thr: Dust threshold in USDT; coins below it are ignored, a non-positive value keeps every
///         priced coin.
///
/// Returns:
///     Marks keyed by the core that carries them; cores without idle money are absent.
pub(super) fn idle_marks(inputs: &[IdleInput<'_>], thr: f64) -> HashMap<CoreId, IdleMark> {
    let mut accounts: Vec<Vec<&IdleInput<'_>>> = Vec::new();
    for input in inputs {
        let known = input.key.as_ref().and_then(|key| {
            accounts
                .iter()
                .position(|members| members[0].key.as_ref() == Some(key))
        });
        match known {
            Some(index) => accounts[index].push(input),
            None => accounts.push(vec![input]),
        }
    }
    let mut marks = HashMap::new();
    for members in accounts {
        let Some(host) = members.iter().find(|member| member.visible) else {
            continue;
        };
        let Some(trading) = members
            .iter()
            .map(|member| member.venue.and_then(trading_wallet))
            .collect::<Option<Vec<WalletKind>>>()
        else {
            continue;
        };
        let Some((wallets, as_of_ms)) = members
            .iter()
            .filter_map(|member| member.wallets)
            .max_by_key(|(_, at_ms)| *at_ms)
        else {
            continue;
        };
        let idle: Vec<IdleWallet> = WalletKind::ALL
            .iter()
            .filter(|kind| !trading.contains(kind))
            .filter_map(|&kind| idle_wallet(kind, wallets, thr))
            .collect();
        if !idle.is_empty() {
            marks.insert(
                host.core,
                IdleMark {
                    wallets: idle,
                    as_of_ms,
                    in_total: members.iter().any(|member| member.counted),
                },
            );
        }
    }
    marks
}

/// USDT outside the trading wallets of the accounts the footer total counts.
///
/// The footer shows it beside its total, so it follows the total's scope: a mark whose account
/// is left out of the total by every shown core's own setting is not added.
///
/// Args:
///     marks: The core list's marks ([`idle_marks`]).
///
/// Returns:
///     The sum of every counted account's idle wallets, `0` when there are none.
pub(super) fn footer_idle_total<'a>(marks: impl IntoIterator<Item = &'a IdleMark>) -> f64 {
    marks
        .into_iter()
        .filter(|mark| mark.in_total)
        .flat_map(|mark| mark.wallets.iter())
        .map(|wallet| wallet.value)
        .sum()
}

/// Sum one wallet's coins at or above the threshold, or `None` when nothing is left.
fn idle_wallet(kind: WalletKind, wallets: &TransferAssetsSnapshot, thr: f64) -> Option<IdleWallet> {
    let mut coins: Vec<(String, f64)> = wallets
        .wallet(kind)
        .iter()
        .filter(|row| row.value_usdt.is_finite() && row.value_usdt > 0.0)
        .filter(|row| thr <= 0.0 || row.value_usdt >= thr)
        // Uppercased for display: Gate's futures wallet reports `usdt`.
        .map(|row| (row.currency.to_ascii_uppercase(), row.value_usdt))
        .collect();
    if coins.is_empty() {
        return None;
    }
    coins.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Some(IdleWallet {
        kind,
        value: coins.iter().map(|(_, value)| value).sum(),
        coins,
    })
}

#[cfg(test)]
mod tests;
