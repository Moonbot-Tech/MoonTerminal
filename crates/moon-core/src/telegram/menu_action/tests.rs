use super::*;
use crate::telegram::report::ReportRequest;

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// Every action survives its callback and stays inside Telegram's 64 bytes.
#[test]
fn actions_round_trip_through_their_callbacks() {
    let actions = [
        MenuAction::Report,
        MenuAction::Noop,
        MenuAction::Preset(Preset::Days7),
        MenuAction::Preset(Preset::Days30),
        MenuAction::Preset(Preset::LastWeek),
        MenuAction::Custom {
            month: None,
            from: None,
        },
        MenuAction::Custom {
            month: Some(day(2026, 9, 1)),
            from: None,
        },
        MenuAction::Custom {
            month: Some(day(2026, 10, 1)),
            from: Some(day(2026, 9, 28)),
        },
    ];
    for action in actions {
        let data = action.callback();
        assert!(data.len() <= 64, "{data}");
        assert_eq!(MenuAction::parse_callback(&data), Some(action), "{data}");
    }
}

/// A picked first day with no month opens on that day's month.
#[test]
fn a_first_day_alone_names_its_month() {
    let data = MenuAction::Custom {
        month: None,
        from: Some(day(2026, 2, 14)),
    }
    .callback();
    assert_eq!(data, "m:c:2026-02:2026-02-14");
}

/// Malformed data and the other namespaces are not menu actions.
#[test]
fn foreign_and_malformed_data_is_refused() {
    for data in [
        "r:e:t:0",
        "station:update",
        "m:",
        "m:x",
        "m:p:14",
        "m:c:2026-13",
        "m:c:26-09",
        "m:c:2026-9",
        "m:c:1969-12",
        "m:c:2026-09:2026-02-30",
        "m:c:2026-09:2026-09-01:x",
        "m:r:extra",
    ] {
        assert_eq!(MenuAction::parse_callback(data), None, "{data}");
    }
}

/// Presets count back from today; last week is the Monday-to-Sunday before this one.
#[test]
fn presets_count_back_from_today() {
    // 2026-10-03 is a Saturday.
    let today = day(2026, 10, 3);
    assert_eq!(Preset::Days7.dates(today), Some((day(2026, 9, 27), today)));
    assert_eq!(Preset::Days30.dates(today), Some((day(2026, 9, 4), today)));
    assert_eq!(
        Preset::LastWeek.dates(today),
        Some((day(2026, 9, 21), day(2026, 9, 27)))
    );
    // On a Monday, last week still ends yesterday.
    assert_eq!(
        Preset::LastWeek.dates(day(2026, 9, 28)),
        Some((day(2026, 9, 21), day(2026, 9, 27)))
    );
    for preset in Preset::ALL {
        let (from, to) = preset.dates(today).unwrap();
        assert!(ReportRequest::span(from, to).is_some());
    }
}
