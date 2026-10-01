//! How much riskier than the fact an answer may be. The search ranks by profit alone, so a
//! point that tripled the max drawdown for a larger profit won (LinKvo, 2026-10-01: "it found
//! more profit with three times the drawdown — too risky, I do not want that"). A limit holds a
//! point's max drawdown and win rate, on the deals it is fitted on, to within a share of the
//! fact's there — the "Fact" column the variant is read against: a point past either is refused
//! like one that breaks a corridor.

use crate::db::metrics::Tally;

/// The share worse than the fact each limit allows when the settings do not say, per cent.
pub const DEFAULT_WORSE_PCT: f64 = 20.0;

/// The limits of one search; `None` leaves that measure free.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RiskLimits {
    /// How much deeper than the fact's max drawdown a point's may fall, per cent of the fact's:
    /// 20 lets a fact's 10 go to 12. A fact without a drawdown allows none.
    pub drawdown_pct: Option<f64>,
    /// How much lower than the fact's win rate a point's may be, per cent of the fact's: 20
    /// lets a fact's 85 % go to 68 %.
    pub winrate_pct: Option<f64>,
}

impl RiskLimits {
    /// Whether `point` stays within the limits of `base` — both over the same deals.
    ///
    /// Args:
    ///     point: The tally a point makes.
    ///     base: The tally it is held to: the fact's.
    pub fn allows(&self, point: &Tally, base: &Tally) -> bool {
        // A rounding hair is not a step past the limit: the reference itself always passes.
        let hair = 1e-9;
        let drawdown = self
            .drawdown_pct
            .is_none_or(|pct| point.max_dd <= base.max_dd * (1.0 + pct.max(0.0) / 100.0) + hair);
        let winrate = self.winrate_pct.is_none_or(|pct| {
            point.winrate() >= base.winrate() * (1.0 - pct.clamp(0.0, 100.0) / 100.0) - hair
        });
        drawdown && winrate
    }
}

#[cfg(test)]
mod tests;
