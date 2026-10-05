use moon_core::config::telegram_menu::{BotSettings, MenuItem, ReportView};
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
                SettingsAction::ShowButton(item, _) => Some(item),
                _ => None,
            })
            .collect();
        assert!(!toggles.contains(&MenuItem::Settings));
        assert_eq!(
            toggles.contains(&MenuItem::Status),
            host == HostKind::Station
        );
        // Keyboard buttons only, each once: the Report section is fixed.
        let expected = MenuItem::ALL.len() - 1 - usize::from(host == HostKind::Terminal);
        assert_eq!(toggles.len(), expected);
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

/// The notifications screen switches every rule and automatic report and offers the down presets
/// with the current one marked.
#[test]
fn the_notify_screens_switch_and_pick() {
    let _locale = crate::test_locale::force("en");
    let notify = NotifySettings::default();
    let (_, lines, rows) = super::notify_screen(&notify);
    assert_eq!(lines.len(), 7);
    let all = actions(&rows);
    for wanted in [
        SettingsAction::Opened(true),
        SettingsAction::Detects(true),
        SettingsAction::Auto(moon_core::telegram::notify::AutoReport::Hourly, true),
        SettingsAction::Auto(moon_core::telegram::notify::AutoReport::Today, true),
        SettingsAction::Auto(moon_core::telegram::notify::AutoReport::Month, true),
        SettingsAction::Trades(true),
        SettingsAction::Down(true),
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
}

/// The automatic reports stand two a row, so their names fit; the down delay offers 1 and 5
/// minutes, side by side.
#[test]
fn the_notify_screen_keeps_its_buttons_readable() {
    let _locale = crate::test_locale::force("ru");
    let (_, _, rows) = super::notify_screen(&NotifySettings::default());
    let auto: Vec<usize> = rows
        .iter()
        .filter(|row| {
            row.iter().any(|b| {
                matches!(
                    action(b),
                    Some(MenuAction::Settings(SettingsAction::Auto(..)))
                )
            })
        })
        .map(Vec::len)
        .collect();
    assert_eq!(auto, vec![2, 1]);
    let delays: Vec<SettingsAction> = actions(&rows)
        .into_iter()
        .filter(|action| matches!(action, SettingsAction::DownAfter(_)))
        .collect();
    assert_eq!(
        delays,
        vec![SettingsAction::DownAfter(1), SettingsAction::DownAfter(5)]
    );
}

/// Losing schedule words or reverting to vague report names would revive the hourly confusion.
/// Reviewed English labels are pinned at the actual callback buttons, not just in the dictionary.
#[test]
fn report_buttons_name_their_schedule() {
    let _locale = crate::test_locale::force("en");
    let (_, lines, rows) = super::notify_screen(&NotifySettings::default());
    for (kind, label) in [
        (
            moon_core::telegram::notify::AutoReport::Hourly,
            "Each hour · separate",
        ),
        (
            moon_core::telegram::notify::AutoReport::Today,
            "Today · hourly",
        ),
        (
            moon_core::telegram::notify::AutoReport::Month,
            "Month · midnight",
        ),
    ] {
        let button = rows
            .iter()
            .flatten()
            .find(|button| {
                action(button) == Some(MenuAction::Settings(SettingsAction::Auto(kind, true)))
            })
            .expect("report toggle");
        assert_eq!(button.text, format!("\u{2b1c} {label}"));
    }
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Turning off Hourly does not turn off Today"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("full previous day at midnight"))
    );
}

/// Translations must keep two report buttons readable on a phone; moving explanations into
/// button labels would exceed this reviewed compact-text budget.
#[test]
fn report_buttons_remain_compact_in_every_locale() {
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let (_, _, rows) = super::notify_screen(&NotifySettings::default());
        for button in rows.iter().flatten().filter(|button| {
            matches!(
                action(button),
                Some(MenuAction::Settings(SettingsAction::Auto(..)))
            )
        }) {
            assert!(
                button.text.chars().count() <= 24,
                "{}: {}",
                language.code(),
                button.text
            );
        }
    }
}

/// Removing the shared header or using the wrong host would make independent bot settings
/// indistinguishable. Exercise the rendered HTML for every local Settings subsection.
#[test]
fn every_settings_screen_names_the_bot_host() {
    let _locale = crate::test_locale::force("en");
    use moon_core::config::telegram_menu::ReportBasis;
    use moon_core::telegram::api::{InlineKeyboardMarkup, ReplyMarkup};
    use moon_core::telegram::runtime::Response;
    let root = crate::notify::test_host::TempRoot::new("settings-host-title");
    let fixture = crate::notify::test_host::TickHost::open(root.notifications());
    for (host, expected, other) in [
        (HostKind::Terminal, "Bot of this terminal", "Station bot"),
        (HostKind::Station, "Station bot", "Bot of this terminal"),
    ] {
        for screen in [
            super::root(&fixture, true),
            super::buttons(&BotSettings::default(), host),
            super::view(ReportView::Exchanges),
            super::basis(ReportBasis::Close),
            super::notify_screen(&NotifySettings::default()),
        ] {
            let Response::Rich { html, .. } = super::render(
                host,
                screen,
                ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(Vec::new())),
            ) else {
                panic!("settings must be a rich message")
            };
            assert!(
                html.starts_with(&format!("<p><b>{expected}</b></p>")),
                "{html}"
            );
            assert!(!html.contains(other), "{html}");
        }
    }
}
