//! Chronological train/holdout evaluation of the exact schedule actually suggested.

use super::{
    Query, ReadResult, TimeAxes, TimeSuggest, accepts, read_tuner_rows, time_suggest_from_rows,
};
use crate::db::metrics::Tally;

/// Fitted schedule and independent validation figures from one pinned report snapshot.
#[derive(Clone, Debug, Default)]
pub struct TimeEvaluation {
    /// Bounds to write into the searched fields.
    pub suggestion: TimeSuggest,
    /// Fitted schedule on the oldest share, absent when fitting was refused.
    pub train: Option<Tally>,
    /// Fitted schedule on the unseen newest tail.
    pub holdout: Option<Tally>,
    /// Fact on precisely the same unseen tail, without any schedule restriction.
    pub fact_holdout: Option<Tally>,
}

impl TimeEvaluation {
    /// Whether unseen profit falls below Fact, including positive but weaker holdouts.
    pub fn below_fact(&self) -> bool {
        self.holdout
            .as_ref()
            .zip(self.fact_holdout.as_ref())
            .is_some_and(|(window, fact)| window.profit < fact.profit)
    }
}

/// Search only training rows and tally the final saved bounds separately on both cohorts.
pub fn suggest_time_checked(
    q: &Query,
    min_n: Option<i64>,
    round: bool,
    axes: TimeAxes,
    train_frac: f64,
) -> ReadResult<TimeEvaluation> {
    if axes.is_empty() {
        return Ok(TimeEvaluation::default());
    }
    let rows = read_tuner_rows(q, |conn, q, src| {
        let mut rows = Vec::new();
        super::super::visit_time_rows_with_close(
            conn,
            q,
            src,
            "tuner: suggest_time_checked",
            |wd, mn, p, close| rows.push((wd, mn, p, close)),
        )?;
        Ok(rows)
    })?;
    Ok(evaluate(rows, min_n, round, axes, train_frac))
}

/// Sort by UTC close/profit, preserving equal-close groups through the filter tuner's split.
fn evaluate(
    mut rows: Vec<(i64, i64, f64, i64)>,
    min_n: Option<i64>,
    round: bool,
    axes: TimeAxes,
    train_frac: f64,
) -> TimeEvaluation {
    rows.sort_by(|a, b| {
        a.3.cmp(&b.3)
            .then_with(|| a.2.total_cmp(&b.2))
            .then(a.0.cmp(&b.0))
            .then(a.1.cmp(&b.1))
    });
    let closes: Vec<_> = rows.iter().map(|r| r.3).collect();
    let cut = super::super::threshold_search::train_split(&closes, train_frac);
    let min_n = min_n.unwrap_or((cut / 10) as i64).max(1);
    let train_rows: Vec<_> = rows[..cut].iter().map(|r| (r.0, r.1, r.2)).collect();
    if train_rows
        .iter()
        .filter(|&&(wd, mn, _)| accepts(wd, mn, axes.fixed_week, axes.fixed_tod))
        .count()
        < min_n as usize
    {
        return TimeEvaluation::default();
    }
    let suggestion = time_suggest_from_rows(&train_rows, min_n, 512, round, axes);
    let tally = |part: &[(i64, i64, f64, i64)], fact| {
        let mut tally = Tally::default();
        for &(wd, mn, p, _) in part {
            if fact
                || accepts(
                    wd,
                    mn,
                    suggestion.week_span.or(axes.fixed_week),
                    suggestion.tod.or(axes.fixed_tod),
                )
            {
                tally.push(p);
            }
        }
        tally
    };
    TimeEvaluation {
        suggestion,
        train: Some(tally(&rows[..cut], false)),
        holdout: (cut < rows.len()).then(|| tally(&rows[cut..], false)),
        fact_holdout: (cut < rows.len()).then(|| tally(&rows[cut..], true)),
    }
}

#[cfg(test)]
mod tests;
