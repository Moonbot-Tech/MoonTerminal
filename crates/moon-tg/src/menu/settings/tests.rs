use moon_core::config::telegram_menu::{BotSettings, MenuItem, MenuLevel, ReportView};
use moon_core::telegram::api::InlineKeyboardButton;
use moon_core::telegram::menu_action::{MenuAction, SettingsAction};
use moon_core::telegram::notify::NotifySettings;

use crate::HostKind;

/// What a button asks, decoded as the transport decodes it.
fn action(button: &InlineKeyboardButton) -> Option<MenuAction> {
    let data = button.callback_data.as_deref()?;
    assert!(data.len() <= 64, "{data}");
    MenuAction::parse_callback(data)
}

/// Every Settings action among `rows`.
fn actions(rows: &[Vec<InlineKeyboardButton>]) -> Vec<SettingsAction> {
    rows.iter()
        .flatten()
        .filter_map(action)
        .filter_map(|action| match action {
            MenuAction::Settings(action) => Some(action),
            _ => None,
        })
        .collect()
}

/// The buttons screen offers every button but Settings itself, and Status only on a station; the
/// way back goes to the section.
#[test]
fn the_buttons_screen_offers_what_a_chat_may_toggle() {
    let bot = BotSettings::default();
    for host in [HostKind::Terminal, HostKind::Station] {
        let (_, _, rows) = super::buttons(&bot, host);
        let toggles: Vec<_> = actions(&rows)
            .into_iter()
            .filter_map(|action| match action {
                SettingsAction::ShowButton(level, item, _) => Some((level, item)),
                _ => None,
            })
            .collect();
        assert!(!toggles.iter().any(|(_, item)| *item == MenuItem::Settings));
        assert_eq!(
            toggles.contains(&(MenuLevel::Keyboard, MenuItem::Status)),
            host == HostKind::Station
        );
        assert!(toggles.contains(&(MenuLevel::Report, MenuItem::Custom)));
        assert_eq!(actions(&rows).last(), Some(&SettingsAction::Root));
    }
}

/// The view screen marks the current view and offers every one.
#[test]
fn the_view_screen_marks_the_current_view() {
    let (_, _, rows) = super::view(ReportView::Cores);
    assert_eq!(rows.len(), ReportView::ALL.len() + 1);
    for (row, view) in rows.iter().zip(ReportView::ALL) {
        assert_eq!(
            action(&row[0]),
            Some(MenuAction::Settings(SettingsAction::SetView(view)))
        );
        assert_eq!(
            row[0].text.starts_with('\u{1f518}'),
            view == ReportView::Cores
        );
    }
}

/// The notifications screen switches all three rules, offers the down presets with the current
/// one marked, and leads to the summary's hours; the hours screen offers all 24.
#[test]
fn the_notify_screens_switch_and_pick() {
    let _locale = crate::test_locale::force("en");
    let notify = NotifySettings::default();
    let (_, lines, rows) = super::notify_screen(&notify, chrono_tz::UTC);
    assert_eq!(lines.len(), 4);
    let all = actions(&rows);
    for wanted in [
        SettingsAction::Trades(true),
        SettingsAction::Down(true),
        SettingsAction::Daily(true),
        SettingsAction::DailyHours,
        SettingsAction::DownAfter(5),
        SettingsAction::Root,
    ] {
        assert!(all.contains(&wanted), "{wanted:?}");
    }
    let marked: Vec<_> = rows
        .iter()
        .flatten()
        .filter(|b| b.text.ends_with('\u{2022}'))
        .filter_map(action)
        .collect();
    assert_eq!(
        marked,
        vec![MenuAction::Settings(SettingsAction::DownAfter(5))]
    );
    let (_, _, hours) = super::daily_hours(21);
    let picks: Vec<_> = actions(&hours)
        .into_iter()
        .filter(|a| matches!(a, SettingsAction::DailyHour(_)))
        .collect();
    assert_eq!(picks.len(), 24);
    assert!(hours.iter().flatten().any(|b| b.text == "\u{2022}21"));
    assert_eq!(actions(&hours).last(), Some(&SettingsAction::Notify));
}
