//! Shared numeric formulas for report aggregates.
//!
//! Profit factor, win rate, and the floating-point noise margin the threshold sweeps compare
//! against were each written out in four or five places (the Analytics summary, the coin and
//! strategy groups, the tuner variant KPIs, the automatic sweeps). One definition here keeps
//! them from drifting apart between the screens that sit side by side and must agree.

/// Sum of wins over the absolute sum of losses.
///
/// `lsum` is the ABSOLUTE loss total (positive). Returns 99 when there are wins but no losses,
/// and 0 when there are neither — the single definition every KPI table and variant sweep
/// shares, so profit factor cannot disagree between the summary, the coin groups, and the tuner.
pub fn profit_factor(wsum: f64, lsum: f64) -> f64 {
    if lsum > 0.0 {
        wsum / lsum
    } else if wsum > 0.0 {
        99.0
    } else {
        0.0
    }
}

/// Win percentage of `wins` out of `n`; 0 for an empty set.
pub fn winrate(wins: i64, n: i64) -> f64 {
    if n > 0 {
        wins as f64 / n as f64 * 100.0
    } else {
        0.0
    }
}

/// Minimum profit improvement worth acting on, scaled to `base`.
///
/// Bin sums and running totals accumulate in different orders, so an equivalent trade set can
/// appear to "win" by about 1e-12; a candidate must beat the baseline by more than this to
/// count as a real improvement rather than summation noise.
pub fn improvement_margin(base: f64) -> f64 {
    base.abs().max(1.0) * 1e-9
}

/// Running aggregate of one trade sequence: totals, win split, and the drawdown of the
/// cumulative curve.
///
/// Drawdown is a property of the ORDER the trades arrive in, unlike every other figure here, so
/// a tally must be fed CHRONOLOGICALLY — an unordered feed reports a fall that never happened.
/// The running cumulative profit is [`Self::profit`] itself; only its peak needs keeping.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    /// Trades counted.
    pub n: i64,
    /// Trades that finished positive.
    pub wins: i64,
    /// Sum of every result, and therefore the cumulative curve's current value.
    pub profit: f64,
    /// Sum of the winning results.
    pub wsum: f64,
    /// ABSOLUTE sum of the losing results.
    pub lsum: f64,
    /// Deepest fall of the cumulative curve below its running peak.
    pub max_dd: f64,
    /// Highest cumulative profit seen so far, which the next trade measures its fall against.
    peak: f64,
}

impl Tally {
    /// Add one trade's result, in chronological order.
    ///
    /// A zero result counts as a loss, matching every other report aggregate: "win" means the
    /// trade actually made money.
    pub fn push(&mut self, profit: f64) {
        self.n += 1;
        self.profit += profit;
        if profit > 0.0 {
            self.wins += 1;
            self.wsum += profit;
        } else {
            self.lsum -= profit;
        }
        self.peak = self.peak.max(self.profit);
        self.max_dd = self.max_dd.max(self.peak - self.profit);
    }

    /// Win percentage of the counted trades.
    pub fn winrate(&self) -> f64 {
        winrate(self.wins, self.n)
    }

    /// Profit factor of the counted trades.
    pub fn profit_factor(&self) -> f64 {
        profit_factor(self.wsum, self.lsum)
    }

    /// Average result per trade; 0 for an empty tally.
    pub fn avg(&self) -> f64 {
        if self.n > 0 {
            self.profit / self.n as f64
        } else {
            0.0
        }
    }

    /// Average winning result; 0 when nothing won.
    pub fn avg_win(&self) -> f64 {
        if self.wins > 0 {
            self.wsum / self.wins as f64
        } else {
            0.0
        }
    }

    /// Average losing result, as a POSITIVE magnitude; 0 when nothing lost.
    ///
    /// Derived here rather than at the call site so the "no losses" guard lives with the two
    /// fields it protects, next to [`Self::avg_win`] and [`Self::profit_factor`].
    pub fn avg_loss(&self) -> f64 {
        let losses = self.n - self.wins;
        if losses > 0 {
            self.lsum / losses as f64
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests;
