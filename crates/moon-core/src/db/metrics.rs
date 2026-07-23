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

#[cfg(test)]
mod tests;
