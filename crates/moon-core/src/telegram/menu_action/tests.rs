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

/// Every Settings screen and switch survives its callback, inside Telegram's 64 bytes; a button
/// of the retired Report-section level, or a value out of range, is refused.
#[test]
fn settings_actions_round_trip() {
    use crate::config::telegram_menu::{MenuItem, ReportBasis, ReportView};
    let mut actions = vec![
        SettingsAction::Root,
        SettingsAction::Buttons,
        SettingsAction::View,
        SettingsAction::Basis,
        SettingsAction::MiniApp(true),
        SettingsAction::MiniApp(false),
        SettingsAction::Notify,
        SettingsAction::Trades(true),
        SettingsAction::Down(false),
        SettingsAction::DownAfter(1),
        SettingsAction::DownAfter(1440),
        SettingsAction::Daily(true),
        SettingsAction::DailyHours,
        SettingsAction::DailyHour(0),
        SettingsAction::DailyHour(23),
        SettingsAction::StationStatus,
    ];
    actions.extend(ReportView::ALL.map(SettingsAction::SetView));
    actions.extend(ReportBasis::ALL.map(SettingsAction::SetBasis));
    actions.extend(
        MenuItem::ALL
            .iter()
            .flat_map(|&item| [true, false].map(|show| SettingsAction::ShowButton(item, show))),
    );
    for action in actions {
        let data = MenuAction::Settings(action).callback();
        assert!(data.len() <= 64, "{data}");
        assert_eq!(
            MenuAction::parse_callback(&data),
            Some(MenuAction::Settings(action)),
            "{data}"
        );
    }
    assert_eq!(MenuAction::Settings(SettingsAction::Root).callback(), "m:s");
    for data in [
        "m:s:b:r:help:1",
        "m:s:b:r:today:1",
        "m:s:b:k:miniapp:1",
        "m:s:b:x:today:1",
        "m:s:b:k:today",
        "m:s:b:k:today:2",
        "m:s:m",
        "m:s:n:t",
        "m:s:v:weekly",
        "m:s:n:d:0",
        "m:s:n:d:1441",
        "m:s:n:h:24",
        "m:s:zzz",
    ] {
        assert_eq!(MenuAction::parse_callback(data), None, "{data}");
    }
}
