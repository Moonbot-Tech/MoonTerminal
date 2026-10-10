//! Civil-month bounds and the bot's shared closed-trade day totals, without GPUI.

use chrono::{DateTime, Datelike, Days, Months, NaiveDate, Utc};
use chrono_tz::Tz;
use moon_core::db::{self, QuoteBreakdown, ReportFilter, TotalsSlice};
use moon_core::util::display_time;

/// One closed-trade day, retaining native and valued facts from the shared totals reader.
#[derive(Clone, Debug)]
pub(super) struct DayRow {
    /// Civil date on the report's selected display axis.
    pub date: NaiveDate,
    /// The same breakdown the Telegram day row consumes.
    pub quotes: QuoteBreakdown,
}

/// One pinned month read, including the exact instant its label represents.
#[derive(Clone, Debug)]
pub(super) struct DayReport {
    /// Only days with closed deals, including days with zero profit.
    pub rows: Vec<DayRow>,
    /// Complete-scope total read on the same snapshot, never summing unlike currencies.
    pub total: QuoteBreakdown,
    /// Refresh instant used for the current-month period label.
    pub refreshed: DateTime<Utc>,
}

/// Return the first civil date of the current month in the selected zone.
pub(super) fn current_month(now: DateTime<Utc>, zone: Tz) -> NaiveDate {
    now.with_timezone(&zone)
        .date_naive()
        .with_day(1)
        .expect("first day exists")
}

/// Step by one civil month, rejecting a next month beyond the current one.
pub(super) fn step_month(month: NaiveDate, forward: bool, current: NaiveDate) -> Option<NaiveDate> {
    let next = if forward {
        month.checked_add_months(Months::new(1))?
    } else {
        month.checked_sub_months(Months::new(1))?
    };
    (next <= current).then_some(next)
}

/// Resolve inclusive bot-compatible bounds: month start through now, or a complete past month.
pub(super) fn month_range(month: NaiveDate, now: DateTime<Utc>, zone: Tz) -> (i64, i64) {
    let start = display_time::day_start(month, zone).expect("month start has a nearby instant");
    let next = month
        .checked_add_months(Months::new(1))
        .expect("next month exists");
    let end = display_time::day_start(next, zone).expect("month end has a nearby instant") - 1;
    (start, end.min(now.timestamp()))
}

/// State the full past range or the current range and its last refresh time, as in the mockup.
pub(super) fn period_label(month: NaiveDate, now: DateTime<Utc>, zone: Tz) -> String {
    let (_, to) = month_range(month, now, zone);
    let end = DateTime::from_timestamp(to, 0)
        .expect("civil bounds are valid")
        .with_timezone(&zone);
    format!(
        "{} — {}",
        month.format("%d.%m"),
        end.format(if month == current_month(now, zone) {
            "%d.%m %H:%M"
        } else {
            "%d.%m"
        })
    )
}

/// Build clipped civil-day slices using the bot's inclusive bounds, including DST-short days.
pub(super) fn day_slices(from: i64, to: i64, zone: Tz) -> Vec<(NaiveDate, TotalsSlice)> {
    let mut slices = Vec::new();
    let Some(mut date) = display_time::date(from, zone) else {
        return slices;
    };
    let Some(end) = display_time::date(to, zone) else {
        return slices;
    };
    while date <= end {
        let Some(next) = date.checked_add_days(Days::new(1)) else {
            break;
        };
        if let (Some(start), Some(stop)) = (
            display_time::day_start(date, zone),
            display_time::day_start(next, zone),
        ) {
            let start = start.max(from);
            let stop = (stop - 1).min(to);
            // A skipped civil date must never duplicate the following day's trades.
            if start <= stop {
                slices.push((
                    date,
                    TotalsSlice {
                        core_uids: None,
                        date_from: Some(start),
                        date_to: Some(stop),
                    },
                ));
            }
        }
        date = next;
    }
    slices
}

/// Retain the bot's activity rule rather than treating zero profit as an empty day.
pub(super) fn active_rows(
    rows: impl IntoIterator<Item = (NaiveDate, QuoteBreakdown)>,
) -> Vec<DayRow> {
    rows.into_iter()
        .filter(|(_, quotes)| quotes.orders > 0)
        .map(|(date, quotes)| DayRow { date, quotes })
        .collect()
}

/// Read the bot's actual aggregation with the monitor's scope, valuation and loaded report axis.
///
/// Returns a classified failure; absence or a failed statement never becomes an empty month.
pub(super) fn read_days(
    filter: ReportFilter,
    month: NaiveDate,
    now: DateTime<Utc>,
    zone: Tz,
) -> db::ReadResult<DayReport> {
    let conn = db::open_reader()?;
    read_days_on(&conn, filter, month, now, zone)
}

/// Select Slavic one/few/many nouns, or singular/plural nouns for the other shipped languages.
pub(super) fn core_count_key(count: usize, locale: &str) -> &'static str {
    let form = if matches!(locale, "ru" | "uk") {
        if (11..=14).contains(&(count % 100)) {
            2
        } else {
            match count % 10 {
                1 => 0,
                2..=4 => 1,
                _ => 2,
            }
        }
    } else if count == 1 {
        0
    } else {
        2
    };
    [
        "profit_monitor.days.cores_one",
        "profit_monitor.days.cores_few",
        "profit_monitor.days.cores_many",
    ][form]
}

/// Connection-injected production read so synthetic fixtures verify scope, totals and failures.
pub(super) fn read_days_on(
    conn: &rusqlite::Connection,
    mut filter: ReportFilter,
    month: NaiveDate,
    now: DateTime<Utc>,
    zone: Tz,
) -> db::ReadResult<DayReport> {
    // Fallback slices must see the same current-rate generation as the complete-period total.
    let _rates = db::valuation::pin_current_rates();
    let snap = db::read_snapshot(conn)?;
    filter.axis = db::ReportAxis::load(&snap, zone)?;
    let (from, to) = month_range(month, now, zone);
    filter.date_from = Some(from);
    filter.date_to = Some(to);
    let dates = day_slices(from, to, zone);
    let mut slices = vec![TotalsSlice {
        core_uids: None,
        date_from: Some(from),
        date_to: Some(to),
    }];
    slices.extend(dates.iter().map(|(_, slice)| slice.clone()));
    let mut totals = db::query_totals_sliced(&snap, &filter, &slices)?.into_iter();
    let total = totals
        .next()
        .expect("shared reader returns one total per slice")
        .quotes;
    let rows = active_rows(
        dates
            .into_iter()
            .zip(totals)
            .map(|((date, _), totals)| (date, totals.quotes)),
    );
    Ok(DayReport {
        rows,
        total,
        refreshed: now,
    })
}

#[cfg(test)]
mod tests;
