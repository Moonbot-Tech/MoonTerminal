//! Folding cores that report one exchange account into one contribution to a balance total.
//!
//! Several cores often trade on one account, and each reports the whole account's balance; a
//! plain sum would count that money once per core. This module decides which figures reach the
//! sum. It takes plain values and no GPUI types, so the rule is testable on its own.

use super::BalanceFigures;
use crate::config::TotalMode;
use crate::venue::{AccountMergeKey, Brand};

/// One in-scope core as the fold sees it.
pub struct TotalMember {
    /// Display name, used to explain a fold in the tooltip.
    pub name: String,
    /// The core's own balance reading.
    pub figures: BalanceFigures,
    /// Which account and wallet the balance is, or `None` when unknown (never merged).
    pub merge: Option<AccountMergeKey>,
    /// The core's persisted total setting.
    pub mode: TotalMode,
}

/// Cores left out of the sum because another core already counts their account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldedGroup {
    /// Name of the core whose reading was counted.
    pub kept: String,
    /// Names of the cores dropped in its favour, in input order.
    pub folded: Vec<String>,
    /// Brand of the shared account, which decides caveats the tooltip must add.
    pub brand: Option<Brand>,
}

/// What reaches the sum, and what was left out and why.
pub(crate) struct FoldOutcome {
    /// One reading per independent contribution, in first-seen order.
    pub(crate) rows: Vec<BalanceFigures>,
    /// Account groups where more than one core reported and only one was counted.
    pub(crate) folded: Vec<FoldedGroup>,
    /// Names of the cores the user set to `Exclude`.
    pub(crate) excluded_by_user: Vec<String>,
}

/// Fold the members into independent contributions to the total.
///
/// `Exclude` drops a core entirely; `Separate` or an unknown account keeps it as its own row.
/// `Auto` cores sharing a merge key form one group, in first-seen order, and contribute ONE
/// reading: the usable member (a value-bearing state and finite figures) with the largest total,
/// the earliest on a tie. Cores report the same wallet through different formulas, so the readings
/// differ slightly; taking the largest avoids understating the account, and taking one member
/// whole keeps free and total from one consistent reading — a stale choice therefore counts
/// stale. Only usable members other than the kept one are reported as folded; an unusable member
/// of such a group is neither counted nor named. A group with no usable member keeps its first member's row, so the total still says the
/// account is missing, and reports nothing folded.
///
/// Args:
///     members: In-scope cores in display order.
///
/// Returns:
///     The rows to aggregate, the folds made, and the user exclusions.
pub(crate) fn fold_accounts(members: &[TotalMember]) -> FoldOutcome {
    let mut out = FoldOutcome {
        rows: Vec::new(),
        folded: Vec::new(),
        excluded_by_user: Vec::new(),
    };
    // Group slots in first-seen order; a solo member is a group of one.
    let mut groups: Vec<Vec<&TotalMember>> = Vec::new();
    for m in members {
        match (m.mode, &m.merge) {
            (TotalMode::Exclude, _) => out.excluded_by_user.push(m.name.clone()),
            (TotalMode::Separate, _) | (TotalMode::Auto, None) => groups.push(vec![m]),
            (TotalMode::Auto, Some(key)) => {
                let existing = groups
                    .iter_mut()
                    .find(|g| g[0].mode == TotalMode::Auto && g[0].merge.as_ref() == Some(key));
                match existing {
                    Some(g) => g.push(m),
                    None => groups.push(vec![m]),
                }
            }
        }
    }
    let usable = |m: &&TotalMember| m.figures.usable();
    for group in groups {
        // `max_by` keeps the LAST of equal maxima; reversing keeps the earliest instead.
        let best = group
            .iter()
            .rev()
            .filter(|m| usable(m))
            .max_by(|a, b| a.figures.total.total_cmp(&b.figures.total));
        let Some(best) = best else {
            out.rows.push(group[0].figures);
            continue;
        };
        out.rows.push(best.figures);
        // Only usable members were folded into the kept reading; an unusable one contributed
        // nothing either way, so naming it as "counted elsewhere" would misstate its account.
        let folded: Vec<String> = group
            .iter()
            .filter(|m| usable(m) && !std::ptr::eq(**m, *best))
            .map(|m| m.name.clone())
            .collect();
        if !folded.is_empty() {
            out.folded.push(FoldedGroup {
                kept: best.name.clone(),
                folded,
                brand: best.merge.as_ref().map(AccountMergeKey::brand),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests;
