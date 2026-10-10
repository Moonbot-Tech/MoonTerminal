//! Synthetic civil-month and activity-rule regressions.

use super::{
    active_rows, current_month, day_slices, month_range, period_label, read_days_on, step_month,
};
use chrono::{DateTime, NaiveDate, Utc};
use moon_core::db::QuoteBreakdown;

/// Parse a synthetic UTC instant without depending on the machine clock.
fn instant(value: &str) -> DateTime<Utc> {
    value.parse().expect("synthetic RFC3339 timestamp")
}

/// Parse an independent expected civil date.
fn date(value: &str) -> NaiveDate {
    value.parse().expect("synthetic ISO date")
}

/// Replacing civil stepping with a fixed thirty-day duration breaks January and leap February.
#[test]
fn stepping_crosses_years_and_never_reaches_a_future_month() {
    let current = date("2024-03-01");
    assert_eq!(
        step_month(date("2024-01-01"), false, current),
        Some(date("2023-12-01"))
    );
    assert_eq!(step_month(date("2024-02-01"), true, current), Some(current));
    assert_eq!(step_month(current, true, current), None);
    assert_eq!(
        current_month(instant("2024-02-29T23:30:00Z"), chrono_tz::Europe::Warsaw),
        current
    );
}

/// Ending the current month at tomorrow or using the monitor preset overstates its last refresh.
#[test]
fn labels_and_bounds_distinguish_full_months_from_month_to_now() {
    let now = instant("2024-03-10T17:16:00Z");
    let zone = chrono_tz::Europe::Warsaw;
    assert_eq!(
        period_label(date("2024-03-01"), now, zone),
        "01.03 — 10.03 18:16"
    );
    assert_eq!(period_label(date("2024-02-01"), now, zone), "01.02 — 29.02");
    assert_eq!(
        month_range(date("2024-02-01"), now, zone),
        (
            instant("2024-01-31T23:00:00Z").timestamp(),
            instant("2024-02-29T22:59:59Z").timestamp()
        )
    );
    assert_eq!(
        month_range(date("2024-03-01"), now, zone).1,
        now.timestamp()
    );
}

/// Fixed 86400-second slices duplicate or omit closes on both Warsaw DST boundaries.
#[test]
fn day_slices_cover_short_and_long_days_once_with_inclusive_bounds() {
    for (month, now, day, hours) in [
        ("2024-03-01", "2024-04-02T00:00:00Z", "2024-03-31", 23),
        ("2024-10-01", "2024-11-02T00:00:00Z", "2024-10-27", 25),
    ] {
        let zone = chrono_tz::Europe::Warsaw;
        let (from, to) = month_range(date(month), instant(now), zone);
        let slices = day_slices(from, to, zone);
        assert_eq!(slices.len(), 31);
        let slice = &slices
            .iter()
            .find(|(date_value, _)| *date_value == date(day))
            .unwrap()
            .1;
        assert_eq!(
            slice.date_to.unwrap() - slice.date_from.unwrap() + 1,
            hours * 3600
        );
        assert_eq!(slices.first().unwrap().1.date_from, Some(from));
        assert_eq!(slices.last().unwrap().1.date_to, Some(to));
        for pair in slices.windows(2) {
            assert_eq!(pair[0].1.date_to.unwrap() + 1, pair[1].1.date_from.unwrap());
        }
    }
}

/// Filtering by profit rather than closed-deal count would lose break-even and unknown-quote days.
#[test]
fn activity_keeps_zero_profit_and_unknown_money_but_omits_days_without_deals() {
    let rows = active_rows([
        (date("2024-03-01"), QuoteBreakdown::default()),
        (
            date("2024-03-02"),
            QuoteBreakdown::from_groups([(Some(0), 0.0, 2)]),
        ),
        (
            date("2024-03-03"),
            QuoteBreakdown::from_groups([(None, 9.0, 3)]),
        ),
    ]);
    assert_eq!(
        rows.iter().map(|row| row.date).collect::<Vec<_>>(),
        [date("2024-03-02"), date("2024-03-03")]
    );
    assert_eq!(rows.iter().map(|row| row.quotes.orders).sum::<i64>(), 5);
}

/// Forward-clamping a skipped date must not add the following date's profit twice.
#[test]
fn apia_skipped_date_is_not_a_second_slice_of_december_31() {
    let zone = chrono_tz::Pacific::Apia;
    let (from, to) = month_range(date("2011-12-01"), instant("2012-01-02T00:00:00Z"), zone);
    let slices = day_slices(from, to, zone);
    assert_eq!(slices.len(), 30);
    assert!(!slices.iter().any(|(day, _)| *day == date("2011-12-30")));
}

/// Losing the scope or loaded clock axis admits a hidden core or groups its close on the wrong day.
#[test]
fn shared_reader_scopes_corrected_days_and_total_on_one_synthetic_snapshot() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, newrecid INTEGER, closedate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER, sellreason TEXT);").unwrap();
    conn.execute_batch("CREATE TABLE core_time_offset (core_uid INTEGER, from_utc INTEGER, offset_secs INTEGER, observed_at INTEGER, source TEXT); INSERT INTO core_time_offset VALUES (11, 0, 7200, 0, 'fixture');").unwrap();
    let insert = |core: i64, id: i64, timestamp: &str, profit: f64, spent: f64| {
        conn.execute(
            "INSERT INTO orders_rep VALUES (?1, 'Fixture', ?2, ?3, ?4, ?5, 0, '')",
            rusqlite::params![core, id, instant(timestamp).timestamp(), profit, spent],
        )
        .unwrap();
    };
    // Core 11 is two hours ahead: its March 3 close belongs to March 2 on the report axis.
    insert(11, 1, "2024-03-03T01:00:00Z", 12.0, 100.0);
    insert(11, 2, "2024-03-03T14:00:00Z", -5.0, 200.0);
    insert(11, 3, "2024-03-04T14:00:00Z", 0.0, 300.0);
    insert(22, 4, "2024-03-03T14:00:00Z", 1000.0, 100.0);
    insert(11, 5, "2024-04-01T14:00:00Z", 100.0, 100.0);
    let filter = moon_core::db::ReportFilter {
        core_uids: vec![11],
        rows: moon_core::db::RowScope::Closed,
        emulator: Some(false),
        ..Default::default()
    };
    let report = read_days_on(
        &conn,
        filter.clone(),
        date("2024-03-01"),
        instant("2024-03-10T17:16:00Z"),
        chrono_tz::UTC,
    )
    .unwrap();
    assert_eq!(
        report.rows.iter().map(|row| row.date).collect::<Vec<_>>(),
        [date("2024-03-02"), date("2024-03-03"), date("2024-03-04")]
    );
    assert_eq!(report.total.orders, 3);
    assert_eq!(report.total.totals[0].profit, 7.0);
    assert_eq!(
        report
            .rows
            .iter()
            .map(|row| row.quotes.totals[0].profit)
            .sum::<f64>(),
        7.0
    );
    assert_eq!(
        report.total.average_order_return().unwrap().avg_order,
        200.0
    );
    let empty = read_days_on(
        &conn,
        moon_core::db::ReportFilter {
            core_uids: vec![moon_core::config::NO_MATCH_CORE_UID],
            ..filter
        },
        date("2024-03-01"),
        instant("2024-03-10T17:16:00Z"),
        chrono_tz::UTC,
    )
    .unwrap();
    assert!(empty.rows.is_empty());
    assert_eq!(empty.total.orders, 0);
    conn.execute_batch(
        "DROP TABLE core_time_offset; CREATE TABLE core_time_offset (broken INTEGER);",
    )
    .unwrap();
    assert!(
        read_days_on(
            &conn,
            Default::default(),
            date("2024-03-01"),
            instant("2024-03-10T17:16:00Z"),
            chrono_tz::UTC
        )
        .is_err(),
        "an axis read failure must not become an empty month"
    );
}
