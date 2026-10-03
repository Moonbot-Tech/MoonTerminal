use chrono::NaiveDate;
use moon_core::config::telegram_menu::{BotMenu, ReportView};
use moon_core::telegram::api::InlineKeyboardButton;
use moon_core::telegram::menu_action::MenuAction;
use moon_core::telegram::report::{Period, ReportRequest};

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// What a button asks, decoded the way the transport decodes it.
#[derive(Debug, PartialEq)]
enum Press {
    Report(ReportRequest),
    Menu(MenuAction),
}

fn press(button: &InlineKeyboardButton) -> Press {
    let data = button.callback_data.as_deref().expect("a callback button");
    assert!(data.len() <= 64, "{data}");
    ReportRequest::parse_callback(data)
        .map(Press::Report)
        .or_else(|| MenuAction::parse_callback(data).map(Press::Menu))
        .unwrap_or_else(|| panic!("undecodable callback {data}"))
}

/// The calendar cell showing `number`, wherever it sits.
fn cell<'a>(rows: &'a [Vec<InlineKeyboardButton>], number: &str) -> &'a InlineKeyboardButton {
    rows[2..rows.len() - 1]
        .iter()
        .flatten()
        .find(|b| b.text.trim_start_matches('\u{2022}') == number)
        .unwrap_or_else(|| panic!("no cell {number}"))
}

/// The Report section lays its buttons out as the menu says and opens reports in the bot's view.
#[test]
fn the_report_section_opens_reports_in_the_bot_view() {
    let rows = super::report_rows(&BotMenu::default(), ReportView::Cores);
    assert_eq!(rows.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 2, 2]);
    let Press::Report(today) = press(&rows[0][0]) else {
        panic!("Today opens a report")
    };
    assert_eq!(today.period, Period::Today);
    assert!(!today.by_exchange && !today.daily);
    let Press::Report(daily) = press(&rows[2][0]) else {
        panic!("Daily opens a report")
    };
    assert!(daily.daily);
    assert_eq!(
        press(&rows[2][1]),
        Press::Menu(MenuAction::Custom {
            month: None,
            from: None
        })
    );
}

/// The custom screen: presets, this month with nothing after today, no way into the future, and
/// the way back; every button decodes and the whole stays inside Telegram's 100 buttons.
#[test]
fn the_custom_screen_offers_this_month_up_to_today() {
    // 2026-10-03 is a Saturday; October 2026 starts on a Thursday.
    let rows = super::custom_rows(None, None, day(2026, 10, 3), ReportView::Exchanges);
    assert_eq!(rows[0].len(), 3, "presets");
    assert_eq!(
        press(&rows[0][2]),
        Press::Menu(MenuAction::Preset(
            moon_core::telegram::report::Preset::LastWeek
        ))
    );
    let calendar = &rows[1..];
    assert_eq!(
        press(&calendar[0][2]),
        Press::Menu(MenuAction::Noop),
        "no next month"
    );
    assert_eq!(
        press(&calendar[0][0]),
        Press::Menu(MenuAction::Custom {
            month: Some(day(2026, 9, 1)),
            from: None
        })
    );
    assert_eq!(calendar[1].len(), 7, "weekdays");
    assert!(
        calendar[2..calendar.len() - 1]
            .iter()
            .all(|week| week.len() == 7)
    );
    assert_eq!(calendar.len() - 3, 5, "five weeks of October 2026");
    assert_eq!(
        press(cell(&rows, "3")),
        Press::Menu(MenuAction::Custom {
            month: Some(day(2026, 10, 1)),
            from: Some(day(2026, 10, 3))
        })
    );
    assert_eq!(press(cell(&rows, "\u{b7}")), Press::Menu(MenuAction::Noop));
    assert!(
        !rows.iter().flatten().any(|b| b.text == "4"),
        "a day after today cannot be picked"
    );
    assert_eq!(
        press(rows.last().unwrap().first().unwrap()),
        Press::Menu(MenuAction::Report)
    );
    let buttons = rows.iter().map(Vec::len).sum::<usize>();
    assert!(buttons <= 100, "{buttons}");
    for button in rows.iter().flatten() {
        assert!(!button.text.trim().is_empty() || button.text == "\u{2800}");
        press(button);
    }
}

/// With the first day picked, a later day opens the report over both in the bot's view, the first
/// day itself opens that day, and an earlier day starts the pick again.
#[test]
fn the_second_pick_opens_the_report() {
    let from = day(2026, 9, 28);
    let rows = super::custom_rows(
        Some(day(2026, 10, 1)),
        Some(from),
        day(2026, 10, 3),
        ReportView::Days,
    );
    let Press::Report(request) = press(cell(&rows, "2")) else {
        panic!("the second pick opens a report")
    };
    assert_eq!(request.period, Period::Dates(from, day(2026, 10, 2)));
    assert!(request.daily && !request.follow_view);
    let rows = super::custom_rows(
        Some(day(2026, 9, 1)),
        Some(from),
        day(2026, 10, 3),
        ReportView::Days,
    );
    assert!(rows.iter().flatten().any(|b| b.text == "\u{2022}28"));
    let Press::Report(single) = press(cell(&rows, "28")) else {
        panic!("the first day opens its own report")
    };
    assert_eq!(single.period, Period::Dates(from, from));
    assert_eq!(
        press(cell(&rows, "27")),
        Press::Menu(MenuAction::Custom {
            month: Some(day(2026, 9, 1)),
            from: Some(day(2026, 9, 27))
        })
    );
}

/// A period never reaches past a year from its first day.
#[test]
fn a_period_stops_at_a_year() {
    let from = day(2025, 10, 1);
    let rows = super::custom_rows(
        Some(day(2026, 10, 1)),
        Some(from),
        day(2026, 10, 3),
        ReportView::Exchanges,
    );
    let Press::Report(longest) = press(cell(&rows, "1")) else {
        panic!("365 days on is still a period")
    };
    assert_eq!(longest.period, Period::Dates(from, day(2026, 10, 1)));
    assert!(
        !rows.iter().flatten().any(|b| b.text == "2"),
        "366 days on is out of reach"
    );
}
