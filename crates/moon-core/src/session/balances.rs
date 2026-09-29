//! Summing per-core balances ("free / total" in USDT) into one account total.
//!
//! Trust is not decided here: [`BalanceState`] is classified by the store that owns the data. This
//! module adds up what that classification allows. Two surfaces state such a total — the desktop
//! Assets footer and the Telegram Mini App — and both call [`aggregate_account_figures`] or
//! [`aggregate_accounts`], so a reading is never counted by one and dropped by the other. Cores
//! that report one exchange account are folded into one contribution first; the raw sum and the
//! fold stay private, so no caller can add up a shared account twice.

use super::{BalanceState, CoreId, SessionManager};
use crate::config::{ServerConfig, TotalMode};

mod fold;

use fold::fold_accounts;
pub use fold::{FoldedGroup, TotalMember};

/// One core's free and total USDT plus the store's trust classification.
///
/// The caller decides which cores are in scope. An empty slice is an empty account reading,
/// not "every core".
#[derive(Clone, Copy, Debug)]
pub struct BalanceFigures {
    /// Store-owned trust classification for `free` and `total`.
    pub state: BalanceState,
    /// Free balance in USDT.
    pub free: f64,
    /// Total balance in USDT, including unrealized PnL.
    pub total: f64,
}

impl BalanceFigures {
    /// Whether this reading can be added to a sum: a value-bearing state and finite figures.
    ///
    /// The one test the footer sum, the account fold and the Mini App share, so a reading is
    /// never shown by one and dropped by another.
    pub fn usable(&self) -> bool {
        self.state.has_value() && self.free.is_finite() && self.total.is_finite()
    }
}

/// Sum of usable balances. `free` and `total` are `None` when `counted == 0`.
///
/// `stale` counts cores inside `counted`. `excluded` is `awaiting + unpriced`, including a
/// non-finite figure whatever its state says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BalanceAggregate {
    /// Sum of usable free balances, or `None` when nothing was counted.
    pub free: Option<f64>,
    /// Sum of usable total balances, or `None` when nothing was counted.
    pub total: Option<f64>,
    /// Cores that contributed a finite figure the state allows into the sum.
    pub counted: u32,
    /// Cores inside `counted` whose state is [`BalanceState::Stale`].
    pub stale: u32,
    /// Cores left out of the sum: awaiting, unpriced, or a non-finite figure.
    pub excluded: u32,
    /// Cores with [`BalanceState::Awaiting`], including a non-finite awaiting figure.
    pub awaiting: u32,
    /// Cores left out for any reason other than awaiting.
    pub unpriced: u32,
}

/// Sum usable balances. Awaiting and unpriced cores are excluded; stale cores are included.
///
/// A non-finite free or total is not a contribution. `Awaiting` still counts as awaiting; every
/// other unusable figure counts as unpriced. `free` and `total` are `None` when `counted == 0`
/// so an empty sum is not reported as zero.
///
/// Args:
///     rows: Per-core figures already limited to the caller's scope.
///
/// Returns:
///     The scope sum and the trust counts. An empty `rows` has `counted == 0` and `None` totals.
pub(crate) fn aggregate_balance_figures(rows: &[BalanceFigures]) -> BalanceAggregate {
    let mut free = 0.0;
    let mut total = 0.0;
    let mut counted = 0u32;
    let mut stale = 0u32;
    let mut awaiting = 0u32;
    let mut unpriced = 0u32;
    for row in rows {
        // A figure that cannot be added is not a contribution, whatever its state says. The
        // producer validates these values, but keeping the check structural prevents a malformed
        // aggregate from being counted while its arithmetic is silently skipped.
        if !row.usable() {
            if row.state == BalanceState::Awaiting {
                awaiting = awaiting.saturating_add(1);
            } else {
                unpriced = unpriced.saturating_add(1);
            }
            continue;
        }
        counted = counted.saturating_add(1);
        if row.state == BalanceState::Stale {
            stale = stale.saturating_add(1);
        }
        free += row.free;
        total += row.total;
    }
    let summed = counted > 0;
    BalanceAggregate {
        free: summed.then_some(free),
        total: summed.then_some(total),
        counted,
        stale,
        excluded: awaiting.saturating_add(unpriced),
        awaiting,
        unpriced,
    }
}

/// A total with cores sharing one exchange account counted once.
pub struct AccountAggregate {
    /// The sum over the folded rows.
    pub sum: BalanceAggregate,
    /// Account groups where only one of several reporting cores was counted.
    pub folded: Vec<FoldedGroup>,
    /// Names of the cores the user set to stay out of the total.
    pub excluded_by_user: Vec<String>,
}

/// Fold members sharing an account ([`fold_accounts`]), then sum ([`aggregate_balance_figures`]).
///
/// The one path every account-aware total takes, so the Assets footer and the Mini App agree.
pub fn aggregate_accounts(members: &[TotalMember]) -> AccountAggregate {
    let outcome = fold_accounts(members);
    AccountAggregate {
        sum: aggregate_balance_figures(&outcome.rows),
        folded: outcome.folded,
        excluded_by_user: outcome.excluded_by_user,
    }
}

/// Sum per-core figures, counting cores that share one exchange account once.
///
/// Looks up each core's total setting in `servers` and its account key in `session`, as the
/// Assets footer does.
///
/// Args:
///     session: Session holding the cores' venues and account identities.
///     servers: Configured cores, holding each one's persisted total setting.
///     rows: `(core, display name, figures)` already limited to the caller's scope.
///
/// Returns:
///     The folded sum, the folds made and the user exclusions.
pub fn aggregate_account_figures(
    session: &SessionManager,
    servers: &[ServerConfig],
    rows: &[(CoreId, String, BalanceFigures)],
) -> AccountAggregate {
    let members: Vec<TotalMember> = rows
        .iter()
        .map(|(id, name, figures)| TotalMember {
            name: name.clone(),
            figures: *figures,
            merge: session.account_merge_key(*id),
            mode: core_total_mode(servers, *id),
        })
        .collect();
    aggregate_accounts(&members)
}

/// How the core's balance counts toward a total, from its persisted setting.
///
/// Args:
///     servers: Configured cores.
///     core: Core to look up.
///
/// Returns:
///     The core's setting, or the default for a core that is not configured.
pub fn core_total_mode(servers: &[ServerConfig], core: CoreId) -> TotalMode {
    servers
        .iter()
        .find(|sv| sv.id == core)
        .map(|sv| sv.total_mode)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
