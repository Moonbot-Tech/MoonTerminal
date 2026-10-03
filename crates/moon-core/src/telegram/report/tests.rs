//! Calendar and callback regressions without network access.
use super::{Period, ReportRequest};
use chrono::{TimeZone, Utc};

/// Scoped custom-date navigation stays within Telegram's 64-byte callback limit.
#[test]
fn scoped_custom_callback_preserves_exchange_and_dates() {
    // A callback names its view; a request still following the bot's is resolved first.
    let mut request = ReportRequest::dates("2026-01-01", "2026-12-31")
        .unwrap()
        .in_view(crate::config::telegram_menu::ReportView::Cores);
    request.window = request.bounds(0, chrono_tz::UTC);
    request.scope = super::ReportScope::Venue(crate::feed::ExchangeId {
        code: 255,
        dex: u32::MAX,
    });
    request.page = 10000;
    request.exchanges_open = true;
    request.basis = Some(crate::config::telegram_menu::ReportBasis::Open);
    let encoded = request.callback();
    assert!(encoded.len() <= 64, "{encoded}");
    assert_eq!(ReportRequest::parse_callback(&encoded), Some(request));
}

/// The exchange-list bit round-trips; a missing or unknown flag stays collapsed.
#[test]
fn callback_exchange_list_flag_defaults_to_collapsed() {
    let mut request = ReportRequest::new(Period::Today, false);
    assert!(!request.exchanges_open);
    assert_eq!(request.callback(), "r:e:t:0");
    assert!(
        !ReportRequest::parse_callback("r:e:t:0")
            .unwrap()
            .exchanges_open
    );
    request.exchanges_open = true;
    assert_eq!(request.callback(), "r:ek:t:0");
    assert_eq!(
        ReportRequest::parse_callback(&request.callback()),
        Some(request)
    );
    assert_eq!(
        ReportRequest::parse_callback("r:eX:t:0"),
        None,
        "an unknown scope is still rejected"
    );
    assert!(
        !ReportRequest::parse_callback("r:e:t:0")
            .unwrap()
            .exchanges_open,
        "legacy callbacks without the flag stay collapsed"
    );
}

/// Paging an old Yesterday report must not silently move to a new day at midnight.
#[test]
fn paging_freezes_bounds_until_refresh() {
    let now = Utc
        .with_ymd_and_hms(2026, 9, 10, 23, 59, 0)
        .unwrap()
        .timestamp();
    let mut request = ReportRequest::new(Period::Yesterday, false);
    let original = request.bounds(now, chrono_tz::UTC).unwrap();
    request.window = Some(original);
    let mut decoded = ReportRequest::parse_callback(&request.callback()).unwrap();
    assert_eq!(decoded.bounds(now + 120, chrono_tz::UTC), Some(original));
    decoded.window = None;
    assert_ne!(decoded.bounds(now + 120, chrono_tz::UTC), Some(original));
}

/// A calendar day across DST must not be replaced by an arbitrary 86400-second window.
#[test]
fn explicit_days_follow_dst_and_include_the_last_second() {
    let request = ReportRequest::dates("2026-03-29", "2026-03-29").unwrap();
    let (a, b) = request.bounds(0, chrono_tz::Europe::Warsaw).unwrap();
    assert_eq!(
        a,
        Utc.with_ymd_and_hms(2026, 3, 28, 23, 0, 0)
            .unwrap()
            .timestamp()
    );
    assert_eq!(
        b,
        Utc.with_ymd_and_hms(2026, 3, 29, 21, 59, 59)
            .unwrap()
            .timestamp()
    );
    assert_eq!(b - a + 1, 23 * 3600);
    let request = ReportRequest::dates("2026-10-25", "2026-10-25").unwrap();
    let (a, b) = request.bounds(0, chrono_tz::Europe::Warsaw).unwrap();
    assert_eq!(b - a + 1, 25 * 3600);
}

/// Month navigation must cross a year boundary without including the next month's midnight.
#[test]
fn last_month_crosses_year_boundary() {
    let now = Utc
        .with_ymd_and_hms(2026, 1, 10, 12, 0, 0)
        .unwrap()
        .timestamp();
    let bounds = ReportRequest::new(Period::LastMonth, false)
        .bounds(now, chrono_tz::UTC)
        .unwrap();
    assert_eq!(
        bounds.0,
        Utc.with_ymd_and_hms(2025, 12, 1, 0, 0, 0)
            .unwrap()
            .timestamp()
    );
    assert_eq!(
        bounds.1,
        Utc.with_ymd_and_hms(2025, 12, 31, 23, 59, 59)
            .unwrap()
            .timestamp()
    );
}

/// Forged callbacks must not widen the date range, allocate huge pages, or become actions.
#[test]
fn callback_parser_rejects_invalid_and_unbounded_inputs() {
    for text in [
        "r:c:2026-09-10,2026-09-01:0",
        "r:c:2020-01-01,2026-09-10:0",
        "r:c:t:10001",
        "r:c:t:0:extra",
        "r:x:t:0",
    ] {
        assert!(ReportRequest::parse_callback(text).is_none(), "{text}");
    }
    assert_eq!(
        ReportRequest::parse_callback("r:d:2026-09-01,2026-09-10:2")
            .unwrap()
            .callback(),
        "r:d:2026-09-01,2026-09-10:2"
    );
}

/// A page carries the basis it was read on through its buttons; a callback without one (a menu
/// button) leaves it to the bot.
#[test]
fn the_basis_survives_the_callback() {
    use crate::config::telegram_menu::{ReportBasis, ReportView};
    for basis in [None, Some(ReportBasis::Close), Some(ReportBasis::Open)] {
        for open in [false, true] {
            let mut request = ReportRequest::preset(Period::Month).in_view(ReportView::Cores);
            request.scope = super::ReportScope::Unidentified;
            request.exchanges_open = open;
            request.basis = basis;
            assert_eq!(
                ReportRequest::parse_callback(&request.callback()),
                Some(request.clone()),
                "{}",
                request.callback()
            );
        }
    }
}

/// The days view splits only a period that covers several days; one day opens by exchanges.
#[test]
fn the_days_view_needs_more_than_one_day() {
    use crate::config::telegram_menu::ReportView;
    let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    for period in [
        Period::Hour,
        Period::Today,
        Period::Yesterday,
        Period::Dates(day, day),
    ] {
        let request = ReportRequest::preset(period.clone()).resolve_view(ReportView::Days);
        assert!(request.by_exchange && !request.daily, "{period:?}");
    }
    for period in [
        Period::Month,
        Period::LastMonth,
        Period::Dates(day, day.succ_opt().unwrap()),
    ] {
        let request = ReportRequest::preset(period.clone()).resolve_view(ReportView::Days);
        assert!(request.daily && !request.by_exchange, "{period:?}");
    }
}

/// UTC seconds of a civil time in `zone`.
fn local(zone: chrono_tz::Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    zone.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .earliest()
        .unwrap()
        .timestamp()
}

/// The hourly report fires at the local hh:00 and reports the 3600 seconds before it.
#[test]
fn the_hourly_slot_is_the_hour_that_just_ended() {
    use crate::telegram::notify::AutoReport;
    let zone = chrono_tz::Europe::Moscow;
    let window =
        super::auto_window(AutoReport::Hourly, local(zone, 2026, 10, 3, 14, 37), zone).unwrap();
    assert_eq!(window.at, local(zone, 2026, 10, 3, 14, 0));
    assert_eq!(window.from, local(zone, 2026, 10, 3, 13, 0));
    assert_eq!(window.to, window.at - 1);
    assert_eq!(window.period, Period::Hour);
    // A zone 45 minutes off the hour fires at its own hh:00.
    let nepal = chrono_tz::Asia::Kathmandu;
    let window =
        super::auto_window(AutoReport::Hourly, local(nepal, 2026, 10, 3, 9, 5), nepal).unwrap();
    assert_eq!(window.at, local(nepal, 2026, 10, 3, 9, 0));
    assert_eq!(window.at - window.from, 3600);
}

/// Across a daylight-saving change the hour before a slot is still one real hour.
#[test]
fn the_hourly_slot_spans_one_real_hour_across_a_clock_change() {
    use crate::telegram::notify::AutoReport;
    let zone = chrono_tz::Europe::Berlin;
    // 2026-10-25 03:00 CEST becomes 02:00 CET: 02:xx happens twice.
    let after = local(zone, 2026, 10, 25, 4, 10);
    let window = super::auto_window(AutoReport::Hourly, after, zone).unwrap();
    assert_eq!(window.at, local(zone, 2026, 10, 25, 4, 0));
    assert_eq!(window.at - window.from, 3600);
}

/// "Today" reports midnight up to the slot; at midnight, the whole day that just ended.
#[test]
fn the_today_slot_reports_today_so_far_or_the_day_just_ended() {
    use crate::telegram::notify::AutoReport;
    let zone = chrono_tz::Europe::Moscow;
    let window =
        super::auto_window(AutoReport::Today, local(zone, 2026, 10, 3, 14, 37), zone).unwrap();
    assert_eq!(window.at, local(zone, 2026, 10, 3, 14, 0));
    assert_eq!(window.from, local(zone, 2026, 10, 3, 0, 0));
    assert_eq!(window.period, Period::Today);
    let window =
        super::auto_window(AutoReport::Today, local(zone, 2026, 10, 4, 0, 20), zone).unwrap();
    assert_eq!(window.at, local(zone, 2026, 10, 4, 0, 0));
    assert_eq!(window.from, local(zone, 2026, 10, 3, 0, 0));
    assert_eq!(window.to, window.at - 1);
    assert_eq!(window.period, Period::Yesterday);
    // The day of a clock change lasts 25 hours.
    let berlin = chrono_tz::Europe::Berlin;
    let window =
        super::auto_window(AutoReport::Today, local(berlin, 2026, 10, 26, 0, 5), berlin).unwrap();
    assert_eq!(window.at - window.from, 25 * 3600);
}

/// The month report fires at midnight; on the 1st it reports the month that just ended whole.
#[test]
fn the_month_slot_reports_up_to_the_day_just_ended() {
    use crate::telegram::notify::AutoReport;
    let zone = chrono_tz::Europe::Moscow;
    let window =
        super::auto_window(AutoReport::Month, local(zone, 2026, 10, 3, 14, 37), zone).unwrap();
    assert_eq!(window.at, local(zone, 2026, 10, 3, 0, 0));
    assert_eq!(window.from, local(zone, 2026, 10, 1, 0, 0));
    assert_eq!(window.period, Period::Month);
    let window =
        super::auto_window(AutoReport::Month, local(zone, 2026, 11, 1, 0, 30), zone).unwrap();
    assert_eq!(window.at, local(zone, 2026, 11, 1, 0, 0));
    assert_eq!(window.from, local(zone, 2026, 10, 1, 0, 0));
    assert_eq!(window.to, window.at - 1);
    assert_eq!(window.period, Period::LastMonth);
    // January 1st reports December of the year before.
    let window =
        super::auto_window(AutoReport::Month, local(zone, 2027, 1, 1, 3, 0), zone).unwrap();
    assert_eq!(window.from, local(zone, 2026, 12, 1, 0, 0));
}
