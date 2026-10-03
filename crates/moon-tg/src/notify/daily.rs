//! Whether today's summary is due, and the figures that summary reports.

use chrono::{NaiveDate, NaiveTime};
use chrono_tz::Tz;
use moon_core::telegram::notify::DailyRule;
use moon_core::util::display_time::{self, LocalBoundary};

use super::trades::ClosedTrade;

/// Totals for one local calendar day.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DaySummary {
    /// Sum of valued `profit_usd`. Unvalued rows add nothing.
    pub profit_usd: f64,
    /// Every row, valued and unvalued.
    pub count: usize,
    /// Rows whose `profit_usd` is `None`.
    pub unvalued: usize,
    /// Valued row with the greatest profit. Equal profits keep the earlier row.
    pub best: Option<ClosedTrade>,
    /// Valued row with the least profit. Equal profits keep the earlier row.
    pub worst: Option<ClosedTrade>,
}

/// Local date whose daily summary should be sent now.
///
/// The clock is today's date in `zone` at `rule.hour`:`rule.minute`. A
/// spring-forward gap uses the first valid local minute after that clock, and
/// a repeated fall-back minute uses the earlier instant. Both go through
/// `display_time::unix_from_local`, the same resolver the civil picker uses.
/// Yesterday is never returned: a missed summary is sent only if today's time
/// has already passed.
///
/// Args:
///     now_utc: Current UTC Unix seconds.
///     zone: The host report zone (`TgHost::report_zone`).
///     rule: Daily summary rule.
///     last: Local date of the last summary, if one was sent.
///
/// Returns:
///     Today's local date when the rule is on, the local clock has passed, and
///     `last` is not already today. `None` before the clock, when the rule is
///     off, when today was already sent, or when `now_utc` is outside chrono's
///     range. An hour or minute chrono cannot represent is also `None`.
pub(crate) fn due(
    now_utc: i64,
    zone: Tz,
    rule: &DailyRule,
    last: Option<NaiveDate>,
) -> Option<NaiveDate> {
    if !rule.on {
        return None;
    }
    let today = display_time::at(now_utc, zone)?.date_naive();
    if last == Some(today) {
        return None;
    }
    let clock = NaiveTime::from_hms_opt(u32::from(rule.hour), u32::from(rule.minute), 0)?;
    // Gap and fold policy lives in the picker helper: first valid minute after
    // a missing civil time, earlier instant when the minute happens twice.
    let target = display_time::unix_from_local(today.and_time(clock), zone, LocalBoundary::Lower)?;
    (now_utc >= target).then_some(today)
}

/// Sum, count, and extremes for a day's closed trades.
///
/// Args:
///     trades: Closed rows that belong to the local day. Order is the tie break.
///
/// Returns:
///     Totals. `profit_usd` is 0 when no row is valued. `best` and `worst` are
///     `None` when every row is unvalued.
pub(crate) fn summarize(trades: &[ClosedTrade]) -> DaySummary {
    let mut summary = DaySummary {
        profit_usd: 0.0,
        count: trades.len(),
        unvalued: 0,
        best: None,
        worst: None,
    };
    for trade in trades {
        let Some(profit) = trade.profit_usd else {
            summary.unvalued += 1;
            continue;
        };
        summary.profit_usd += profit;
        if replace_extreme(&summary.best, profit, true) {
            summary.best = Some(trade.clone());
        }
        if replace_extreme(&summary.worst, profit, false) {
            summary.worst = Some(trade.clone());
        }
    }
    summary
}

/// `true` when `profit` should replace the stored extreme.
///
/// `high` selects the greater profit. A missing current row always loses, and
/// an equal profit does not replace the row already stored.
fn replace_extreme(current: &Option<ClosedTrade>, profit: f64, high: bool) -> bool {
    match current {
        None => true,
        Some(trade) => trade
            .profit_usd
            .is_some_and(|have| if high { profit > have } else { profit < have }),
    }
}

#[cfg(test)]
mod tests;
