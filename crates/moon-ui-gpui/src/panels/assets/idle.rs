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
//! - wallet by wallet, on the list that arrived last among its cores
//!   (`TransferAssetsSnapshot::arrived_at`) — a transfer refreshes only the core it was made
//!   from, and a core answers each wallet request on its own, so neither the cores of an account
//!   nor the lists of one snapshot are all the same age, and an older list would keep showing
//!   money already moved. For the same reason a wallet is not judged at all while the newest
//!   list any of those cores ever held is gone ([`IdleInput::seen_ms`]): a replacement feed
//!   starts from empty lists, and a sibling's older one must not stand in for what was lost;
//! - and marked once, on its first core the panel shows — the same Funding balance on two rows
//!   would read as twice the money.
//!
//! A mark tells the user to move money, so it is drawn only for an account that is established:
//! - a core that has not stated which account it is on carries no mark, and neither does one that
//!   is not connected and ready right now, whatever it stated on an earlier connection
//!   ([`IdleInput::ready`]): the terminal cannot see what such a core does meanwhile, and it may
//!   come back on another account;
//! - and while such a core exists it may be the very sibling that trades the wallet a mark would
//!   name — on its own exchange when that much is known, on any exchange while it has not said
//!   even that. The accounts it could belong to are not judged until it identifies itself. A core
//!   on an exchange this terminal does not model is on none of the marked ones and holds nothing
//!   back.
//!
//! The footer names the same money as one sum (`assets.balances_tip_idle`), and there it follows
//! the footer's own scope: an account whose shown cores are all set to stay out of the total
//! (`TotalMode::Exclude`) keeps its mark here but stays out of that sum, and out of the footer's
//! note about money that waits for a core ([`footer_idle`]). That note speaks only of money held
//! back on an established account: an account whose own cores are all away is not established,
//! so its money is neither marked nor announced. The sum is a wallet snapshot, not a live figure,
//! so the footer states its age and never adds it to the live total: right after a transfer the
//! total already holds the moved money and the snapshot still does.
//!
//! Only exchanges whose wallet layout is verified are marked: Binance, Bybit (a Bybit core always
//! runs the Unified Trading layout, see `WalletKind::label_key`) and Gate. Elsewhere which wallet a
//! core trades from is not known, and a false "this money is idle" is worse than none.

use moon_core::feed::{TransferAssetsSnapshot, WalletKind};
use moon_core::session::CoreId;
use moon_core::venue::{Brand, MarketKind, Venue, WalletAccountKey};
use std::collections::{HashMap, HashSet};

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
    /// Whether the core is connected and ready right now.
    ///
    /// The session keeps what a core stated — its exchange, its account, its wallets — across a
    /// lost connection. None of it is trusted to establish an account while the core is away: a
    /// core that is not ready counts as unidentified, however well it was known a moment ago.
    pub(super) ready: bool,
    /// Whether the core has said which exchange it is on — even one this terminal does not
    /// model, which leaves `venue` empty all the same.
    pub(super) exchange_known: bool,
    /// Resolved venue, or `None` while unknown or not modelled.
    pub(super) venue: Option<Venue>,
    /// Exchange account the wallets belong to, or `None` while the core has not stated it.
    pub(super) key: Option<WalletAccountKey>,
    /// Latest wallet snapshot, or `None` before the first answer. Each wallet list in it carries
    /// its own arrival time.
    pub(super) wallets: Option<&'a TransferAssetsSnapshot>,
    /// The newest arrival time each wallet list has EVER had on this core
    /// (`CoreData::transfer_seen_ms`), in `WalletKind::ALL` order. It can be newer than anything
    /// in `wallets`: a replacement feed starts from empty lists and takes the old ones with it.
    pub(super) seen_ms: [i64; 3],
}

impl IdleInput<'_> {
    /// The account this core establishes right now, or `None` for an unidentified core: one that
    /// has not stated its account, or is not ready ([`Self::ready`]).
    fn account(&self) -> Option<&WalletAccountKey> {
        self.key.as_ref().filter(|_| self.ready)
    }
}

/// One wallet holding money the account's cores do not trade with.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct IdleWallet {
    /// Wallet code.
    pub(super) kind: WalletKind,
    /// USDT value of its coins above the dust threshold.
    pub(super) value: f64,
    /// `(coin, USDT value)` of those coins, largest first.
    pub(super) coins: Vec<(String, f64)>,
}

/// Mark for one core row: every idle wallet of its account.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct IdleMark {
    /// The account's wallets holding unused money, in `WalletKind::ALL` order.
    pub(super) wallets: Vec<IdleWallet>,
    /// Unix-ms the OLDEST of the lists behind `wallets` arrived, so the hover can say how old the
    /// figures are: the wallet lists are not a live feed — each is as fresh as the core's last
    /// answer for it — and a mark naming two wallets is only as fresh as the staler of them.
    pub(super) as_of_ms: i64,
    /// Whether the footer total counts the account: some core of it is [`IdleInput::counted`].
    /// Only those marks enter the footer's sum ([`footer_idle`]).
    pub(super) in_total: bool,
}

/// What judging every account found.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct IdleReport {
    /// Marks keyed by the core row that carries them; cores without idle money are absent.
    pub(super) marks: HashMap<CoreId, IdleMark>,
    /// How many unidentified cores keep idle money the footer would count from being named: the
    /// distinct cores an account that holds idle money, and that the footer total counts, waits
    /// for. `0` when nothing the footer sums is withheld — an unidentified core that hides no
    /// such money is not worth a caveat, and neither is one that could not be on any marked
    /// account.
    pub(super) waiting_for: usize,
}

/// The footer's share of an [`IdleReport`]: one sum, how old it is, and what keeps it short.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FooterIdle {
    /// USDT outside the trading wallets of the accounts the footer total counts; `0` when none.
    pub(super) total: f64,
    /// Unix-ms of the OLDEST wallet list behind `total`: a sum is only as fresh as its stalest
    /// part. `None` when nothing is summed.
    pub(super) as_of_ms: Option<i64>,
    /// How many cores keep idle money of the counted accounts unnamed until they identify
    /// themselves ([`IdleReport::waiting_for`]); `0` when none.
    pub(super) waiting_for: usize,
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

/// Find every established account's idle money and attach it to the account's first visible core.
///
/// Not judged, in this order of checks: an account with no visible core; an account with a core
/// of unverified venue (without knowing what that core trades from, any wallet might be its
/// trading wallet); and an account an unidentified core might belong to (see the module docs).
/// In any account a wallet is left out while no core of the account has been answered for it,
/// or while the newest list one of them held is gone ([`newest_list`]).
///
/// Args:
///     inputs: Every configured core, in canonical order.
///     thr: Dust threshold in USDT; coins at or below it are ignored, as in the wallet columns
///         ([`super::wallets::above_dust`]).
///
/// Returns:
///     The marks, and how many unidentified cores keep counted idle money unnamed.
pub(super) fn idle_report(inputs: &[IdleInput<'_>], thr: f64) -> IdleReport {
    let mut accounts: Vec<Vec<&IdleInput<'_>>> = Vec::new();
    let mut unidentified: Vec<&IdleInput<'_>> = Vec::new();
    for input in inputs {
        let Some(key) = input.account() else {
            unidentified.push(input);
            continue;
        };
        let known = accounts
            .iter()
            .position(|members| members[0].account() == Some(key));
        match known {
            Some(index) => accounts[index].push(input),
            None => accounts.push(vec![input]),
        }
    }
    // An unidentified core may be on any account of its exchange — or of any exchange at all
    // while it has not said which one it is on. A core on an exchange this terminal does not
    // model has said so and has no venue: it matches no brand and holds nothing back.
    let may_be_on = |brand: Brand| {
        unidentified.iter().filter(move |core| {
            !core.exchange_known || core.venue.is_some_and(|venue| venue.brand == brand)
        })
    };
    let mut report = IdleReport::default();
    let mut waited_for: HashSet<CoreId> = HashSet::new();
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
        // One key is one brand, and `trading` resolved, so every member has this venue's brand.
        let Some(brand) = host.venue.map(|venue| venue.brand) else {
            continue;
        };
        let idle: Vec<(IdleWallet, i64)> = WalletKind::ALL
            .iter()
            .filter(|kind| !trading.contains(kind))
            .filter_map(|&kind| {
                let (wallets, at_ms) = newest_list(&members, kind)?;
                idle_wallet(kind, wallets, thr).map(|wallet| (wallet, at_ms))
            })
            .collect();
        let Some(as_of_ms) = idle.iter().map(|(_, at_ms)| *at_ms).min() else {
            continue;
        };
        let in_total = members.iter().any(|member| member.counted);
        let mut blockers = may_be_on(brand).peekable();
        if blockers.peek().is_some() {
            // Withheld. The footer hears of it only where its own sum would have counted the
            // money: an account every shown core keeps out of the total is not its subject.
            if in_total {
                waited_for.extend(blockers.map(|core| core.core));
            }
            continue;
        }
        report.marks.insert(
            host.core,
            IdleMark {
                wallets: idle.into_iter().map(|(wallet, _)| wallet).collect(),
                as_of_ms,
                in_total,
            },
        );
    }
    report.waiting_for = waited_for.len();
    report
}

/// What the footer says about the idle money of the accounts its total counts.
///
/// The footer shows the sum beside its total, so it follows the total's scope: a mark whose
/// account is left out of the total by every shown core's own setting is not added.
///
/// Args:
///     report: The core list's marks ([`idle_report`]).
///
/// Returns:
///     The sum of every counted account's idle wallets with its oldest list's time, and the
///     number of cores those accounts still wait for.
pub(super) fn footer_idle(report: &IdleReport) -> FooterIdle {
    let counted = || report.marks.values().filter(|mark| mark.in_total);
    FooterIdle {
        total: counted()
            .flat_map(|mark| mark.wallets.iter())
            .map(|wallet| wallet.value)
            .sum(),
        as_of_ms: counted().map(|mark| mark.as_of_ms).min(),
        waiting_for: report.waiting_for,
    }
}

/// The list of one wallet that arrived last among an account's cores, with its unix-ms.
///
/// Per wallet, because a snapshot's lists are not one age: a refresh can bring a fresh `Spot`
/// list while the `Futures` request fails, and that core's old `Futures` list must not win over
/// a sibling's newer one.
///
/// And only while it really is the newest the account has had. A core whose feed was replaced
/// holds empty, unanswered lists; if it had heard of a transfer the others had not, the list that
/// said so is gone, and the newest one LEFT still shows the money. Nothing is known about that
/// wallet until some core of the account is answered for it again.
///
/// Returns:
///     The snapshot holding the freshest list of `kind` and when that list arrived, or `None`
///     while no core of the account has been answered for that wallet, or while a newer list than
///     any that is left once existed.
fn newest_list<'a>(
    members: &[&IdleInput<'a>],
    kind: WalletKind,
) -> Option<(&'a TransferAssetsSnapshot, i64)> {
    let slot = usize::from(kind.to_u8());
    let ever_ms = members.iter().map(|member| member.seen_ms[slot]).max()?;
    members
        .iter()
        .filter_map(|member| {
            let wallets = member.wallets?;
            Some((wallets, wallets.arrived_at(kind)?))
        })
        .max_by_key(|(_, at_ms)| *at_ms)
        .filter(|(_, at_ms)| *at_ms >= ever_ms)
}

/// Sum one wallet's coins above the dust threshold, or `None` when nothing is left.
fn idle_wallet(kind: WalletKind, wallets: &TransferAssetsSnapshot, thr: f64) -> Option<IdleWallet> {
    let mut coins: Vec<(String, f64)> = wallets
        .wallet(kind)
        .iter()
        .filter(|row| row.value_usdt.is_finite() && row.value_usdt > 0.0)
        // The very rule the wallet columns hide dust by: a coin the mark counts must be one the
        // user finds in the wallet the mark sends them to.
        .filter(|row| super::wallets::above_dust(row.value_usdt, thr))
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
