//! Pins for the daily clock and the day summary.

use chrono::{DateTime, LocalResult, NaiveDate, TimeZone as _};
use chrono_tz::Europe::Berlin;
use moon_core::telegram::notify::DailyRule;

use super::*;
use crate::notify::trades::ClosedTrade;

fn ymd(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("civil date")
}

fn rule_at(hour: u8, minute: u8) -> DailyRule {
    DailyRule {
        on: true,
        hour,
        minute,
    }
}

fn berlin_single(day: NaiveDate, hour: u32, minute: u32) -> i64 {
    let local = day.and_hms_opt(hour, minute, 0).expect("clock");
    match Berlin.from_local_datetime(&local) {
        LocalResult::Single(resolved) => resolved.timestamp(),
        other => panic!("expected one Berlin instant, got {other:?}"),
    }
}

fn row(rec_id: i64, profit: Option<f64>) -> ClosedTrade {
    ClosedTrade {
        core: 1,
        rec_id,
        close_utc: 0,
        coin: "BTC".to_string(),
        core_name: "alpha".to_string(),
        strategy: "grid".to_string(),
        volume_usd: None,
        profit_usd: profit,
        profit_pct: None,
        open_utc: 0,
        ..ClosedTrade::default()
    }
}

/// Firing on the UTC date, or firing again the same local day, would send the
/// summary at the wrong midnight or twice after the clock has passed.
#[test]
fn due_fires_once_on_the_local_day() {
    let day = ymd(2026, 6, 15);
    let at_clock = berlin_single(day, 21, 0);
    let rule = rule_at(21, 0);
    assert_eq!(due(at_clock, Berlin, &rule, None), Some(day));
    assert_eq!(due(at_clock + 60, Berlin, &rule, Some(day)), None);

    let mut off = rule;
    off.on = false;
    assert_eq!(due(at_clock, Berlin, &off, None), None);

    // 00:10 in Berlin is still the previous UTC date. The summary belongs to the 16th.
    let next_day = ymd(2026, 6, 16);
    let just_after_midnight = berlin_single(next_day, 0, 10);
    let utc_date = DateTime::from_timestamp(just_after_midnight, 0)
        .expect("timestamp")
        .date_naive();
    assert_eq!(utc_date, day);
    assert_eq!(
        due(just_after_midnight, Berlin, &rule_at(0, 5), None),
        Some(next_day)
    );
}

/// A restart that forgets `daily_last` would send today's summary a second time.
#[test]
fn the_same_local_day_is_not_due_again_after_a_restart() {
    let day = ymd(2026, 6, 15);
    let at_clock = berlin_single(day, 21, 0);
    let rule = rule_at(21, 0);
    assert_eq!(due(at_clock, Berlin, &rule, Some(day)), None);
    assert_eq!(due(at_clock + 3_600, Berlin, &rule, Some(day)), None);
}

/// Comparing the clock in UTC, or treating "before" as due, would send the
/// summary early.
#[test]
fn before_the_local_clock_is_not_due() {
    let day = ymd(2026, 6, 15);
    let at_clock = berlin_single(day, 21, 0);
    assert_eq!(due(at_clock - 1, Berlin, &rule_at(21, 0), None), None);
}

/// Resolving 02:30 through the gap as 01:30 UTC, or as 03:30, would send the
/// spring-forward summary an hour early or an hour late.
#[test]
fn berlin_spring_forward_uses_the_first_valid_instant_after_0230() {
    let day = ymd(2026, 3, 29);
    let missing = day.and_hms_opt(2, 30, 0).expect("clock");
    assert!(
        matches!(Berlin.from_local_datetime(&missing), LocalResult::None),
        "the fixture must be the spring-forward gap"
    );
    let first_valid = day.and_hms_opt(3, 0, 0).expect("clock");
    let LocalResult::Single(resolved) = Berlin.from_local_datetime(&first_valid) else {
        panic!("03:00 is the first valid Berlin instant after 02:30");
    };
    let target = resolved.timestamp();
    let rule = rule_at(2, 30);
    assert_eq!(due(target, Berlin, &rule, None), Some(day));
    assert_eq!(due(target - 1, Berlin, &rule, None), None);
}

/// Picking the later 02:30 on the fall-back night would hold the summary until
/// the second time that clock appears.
#[test]
fn berlin_fall_back_uses_the_earlier_0230() {
    let day = ymd(2026, 10, 25);
    let local = day.and_hms_opt(2, 30, 0).expect("clock");
    let LocalResult::Ambiguous(first, second) = Berlin.from_local_datetime(&local) else {
        panic!("02:30 must occur twice on the Berlin fall-back day");
    };
    let earliest = first.min(second).timestamp();
    let latest = first.max(second).timestamp();
    assert!(earliest < latest);
    let rule = rule_at(2, 30);
    assert_eq!(due(earliest, Berlin, &rule, None), Some(day));
    assert_eq!(due(earliest - 1, Berlin, &rule, None), None);
    assert_eq!(due(latest, Berlin, &rule, Some(day)), None);
}

/// Returning yesterday when today's clock has not arrived would send a summary
/// for a day the rule does not catch up.
#[test]
fn a_missed_yesterday_does_not_catch_up() {
    let today = ymd(2026, 6, 15);
    let yesterday = today.pred_opt().expect("previous civil date");
    let at_clock = berlin_single(today, 21, 0);
    let rule = rule_at(21, 0);
    assert_eq!(due(at_clock - 1, Berlin, &rule, None), None);
    assert_eq!(due(at_clock - 1, Berlin, &rule, Some(yesterday)), None);
    assert_eq!(due(at_clock, Berlin, &rule, Some(yesterday)), Some(today));
    assert_eq!(due(at_clock, Berlin, &rule, None), Some(today));
}

/// Swapping best and worst, or folding an unvalued row into the sum, would
/// mis-state the day's card. An equal profit must keep the earlier row.
#[test]
fn summary_names_best_worst_and_counts_unvalued() {
    let rows = vec![
        row(1, Some(10.0)),
        row(2, Some(-3.0)),
        row(3, None),
        row(4, Some(10.0)),
        row(5, Some(-3.0)),
    ];
    let summary = summarize(&rows);
    assert_eq!(summary.count, 5);
    assert_eq!(summary.unvalued, 1);
    assert_eq!(summary.profit_usd.to_bits(), 14.0_f64.to_bits());
    assert_eq!(summary.best.as_ref().map(|trade| trade.rec_id), Some(1));
    assert_eq!(summary.worst.as_ref().map(|trade| trade.rec_id), Some(2));

    let unvalued = summarize(&[row(1, None), row(2, None)]);
    assert_eq!(unvalued.count, 2);
    assert_eq!(unvalued.unvalued, 2);
    assert!(unvalued.best.is_none() && unvalued.worst.is_none());
    assert_eq!(unvalued.profit_usd.to_bits(), 0.0_f64.to_bits());

    let empty = summarize(&[]);
    assert_eq!(empty.count, 0);
    assert_eq!(empty.unvalued, 0);
    assert!(empty.best.is_none() && empty.worst.is_none());
}
