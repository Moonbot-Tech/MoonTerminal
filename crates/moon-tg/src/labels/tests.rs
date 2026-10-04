//! Navigation keyboard regressions: every visible button resolves to its command.

use moon_core::telegram::{
    commands::{ParsedCommand, parse_reply_button},
    report::Period,
};

/// A station bot answering a non-owner keeps the terminal's two-row layout, without Status or
/// Settings — the same as a terminal's viewer.
#[test]
fn station_non_owner_keyboard_matches_the_terminal() {
    let _locale = crate::test_locale::force("en");
    let rows = |host, owner| {
        let moon_core::telegram::api::ReplyMarkup::Reply(markup) =
            super::navigation_keyboard(host, owner, &moon_core::config::TelegramConfig::default())
        else {
            panic!("expected a reply keyboard")
        };
        markup
            .keyboard
            .into_iter()
            .map(|row| row.into_iter().map(|b| b.text).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rows(crate::HostKind::Station, false),
        rows(crate::HostKind::Terminal, false)
    );
    assert_eq!(rows(crate::HostKind::Station, false).len(), 2);
}

/// The reply keyboard keeps its rows: short period labels with Help, then the two month labels;
/// the owner's has Settings below them, and Control once shown.
#[test]
fn the_keyboard_keeps_its_rows() {
    let _locale = crate::test_locale::force("en");
    let labels = super::telegram_labels(crate::HostKind::Terminal);
    let mut telegram = moon_core::config::TelegramConfig::default();
    telegram
        .bot
        .menu
        .set_shown(moon_core::config::telegram_menu::MenuItem::Control, true);
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) =
        super::navigation_keyboard(crate::HostKind::Terminal, true, &telegram)
    else {
        panic!("expected a reply keyboard")
    };
    assert_eq!(
        markup.keyboard.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![3, 2, 2]
    );
    let command =
        |row: usize, col: usize| parse_reply_button(&markup.keyboard[row][col].text, &labels);
    assert!(matches!(
        command(0, 0),
        ParsedCommand::Report(request) if request.period == Period::Today
    ));
    assert!(matches!(
        command(0, 1),
        ParsedCommand::Report(request) if request.period == Period::Yesterday
    ));
    assert!(matches!(command(0, 2), ParsedCommand::Help));
    assert!(matches!(
        command(1, 0),
        ParsedCommand::Report(request) if request.period == Period::Month
    ));
    assert!(matches!(
        command(1, 1),
        ParsedCommand::Report(request) if request.period == Period::LastMonth
    ));
    assert!(matches!(
        command(2, 1),
        ParsedCommand::Menu(moon_core::telegram::menu_action::MenuAction::Control(_))
    ));
}

/// Every visible emoji button must resolve through exact localized aliases without guessing text.
#[test]
fn navigation_buttons_have_recognized_commands() {
    let _locale = crate::test_locale::force("en");
    let labels = super::telegram_labels(crate::HostKind::Terminal);
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) = super::navigation_keyboard(
        crate::HostKind::Terminal,
        true,
        &moon_core::config::TelegramConfig::default(),
    ) else {
        panic!("expected a reply keyboard")
    };
    // Foldable: Android's Back must not be swallowed by a keyboard that cannot hide.
    assert!(!markup.is_persistent);
    for row in markup.keyboard {
        for button in row {
            assert_ne!(
                moon_core::telegram::commands::parse_reply_button(&button.text, &labels),
                moon_core::telegram::commands::ParsedCommand::Unknown,
                "{}",
                button.text
            );
        }
    }
}

/// A station owner's default keyboard is the terminal's with Status on a row of its own, and only
/// a station's labels parse its Status: a terminal's bot has no station to report on.
#[test]
fn only_the_station_keyboard_has_its_status() {
    let _locale = crate::test_locale::force("en");
    let labels = super::telegram_labels(crate::HostKind::Station);
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) = super::navigation_keyboard(
        crate::HostKind::Station,
        true,
        &moon_core::config::TelegramConfig::default(),
    ) else {
        panic!("expected a reply keyboard")
    };
    assert_eq!(
        markup.keyboard.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![3, 2, 2]
    );
    let command =
        |row: usize, col: usize| parse_reply_button(&markup.keyboard[row][col].text, &labels);
    assert!(matches!(
        command(0, 0),
        ParsedCommand::Report(request) if request.period == Period::Today
    ));
    assert!(matches!(
        command(0, 1),
        ParsedCommand::Report(request) if request.period == Period::Yesterday
    ));
    assert!(matches!(command(0, 2), ParsedCommand::Help));
    assert!(matches!(
        command(1, 0),
        ParsedCommand::Report(request) if request.period == Period::Month
    ));
    assert!(matches!(
        command(1, 1),
        ParsedCommand::Report(request) if request.period == Period::LastMonth
    ));
    assert_eq!(command(2, 0), ParsedCommand::StationStatus);
    let status = &markup.keyboard[2][0].text;
    for row in &markup.keyboard {
        for button in row {
            assert_ne!(
                parse_reply_button(&button.text, &labels),
                ParsedCommand::Unknown,
                "{}",
                button.text
            );
        }
    }
    let terminal = super::telegram_labels(crate::HostKind::Terminal);
    assert_eq!(
        parse_reply_button(status, &terminal),
        ParsedCommand::Unknown
    );
}

/// A station's Mini App page and failure texts name the station, never a terminal that may be off;
/// a terminal keeps its own wording.
#[test]
fn station_labels_replace_the_terminal_wording() {
    let _locale = crate::test_locale::force("en");
    let terminal = super::telegram_labels(crate::HostKind::Terminal);
    let station = super::telegram_labels(crate::HostKind::Station);
    for key in super::STATION_WORDED {
        assert_ne!(terminal[*key], station[*key], "{key}");
        for locale in moon_core::config::Language::ALL.map(moon_core::config::Language::code) {
            let path = format!("telegram.{key}_station");
            let text = rust_i18n::t!(&path, locale = locale).to_lowercase();
            assert_ne!(text, path.to_lowercase(), "{path} is missing in {locale}");
            // tr/pt keep the loanword "terminal". Ukrainian uses "термінал" and Vietnamese
            // "thiết bị đầu cuối"; a denied-chat key may still name where to turn the app on.
            assert!(
                !text.contains("terminal")
                    && !text.contains("терминал")
                    && !text.contains("термінал")
                    && !text.contains("thiết bị đầu cuối")
                    || key.contains("denied"),
                "{path} ({locale}) names the terminal: {text}"
            );
        }
    }
    assert_eq!(
        super::report_delivery_failed(crate::HostKind::Station),
        station["report_delivery_failed"]
    );
}

/// The keyboard as `telegram` configures it, as button texts row by row.
fn keyboard_texts(
    host: crate::HostKind,
    owner: bool,
    telegram: &moon_core::config::TelegramConfig,
) -> Vec<Vec<String>> {
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) =
        super::navigation_keyboard(host, owner, telegram)
    else {
        panic!("expected a reply keyboard")
    };
    markup
        .keyboard
        .into_iter()
        .map(|row| row.into_iter().map(|b| b.text).collect())
        .collect()
}

/// A configured menu lays the keyboard out as its rows say, hides what it hides, and keeps
/// Status for a station's owner only; every shown button resolves to its own item.
#[test]
fn the_keyboard_follows_the_configured_menu() {
    let _locale = crate::test_locale::force("ru");
    use moon_core::config::telegram_menu::{MenuEntry, MenuItem::*};
    let mut telegram = moon_core::config::TelegramConfig::default();
    telegram.bot.menu = full_menu(vec![
        vec![MenuEntry::shown(Report), MenuEntry::shown(Status)],
        vec![
            MenuEntry::shown(Custom),
            MenuEntry::hidden(Today),
            MenuEntry::shown(Yesterday),
            MenuEntry::shown(Daily),
        ],
        vec![MenuEntry::shown(Settings)],
    ]);
    let locale = rust_i18n::locale();
    let text = |item| super::button_text(item, locale.as_ref());
    assert_eq!(
        keyboard_texts(crate::HostKind::Station, true, &telegram),
        vec![
            vec![text(Report), text(Status)],
            vec![text(Custom), text(Yesterday), text(Daily)],
            vec![text(Settings)],
        ]
    );
    assert_eq!(
        keyboard_texts(crate::HostKind::Terminal, true, &telegram),
        vec![
            vec![text(Report)],
            vec![text(Custom), text(Yesterday), text(Daily)],
            vec![text(Settings)],
        ]
    );
    for host in [crate::HostKind::Station, crate::HostKind::Terminal] {
        assert_eq!(
            keyboard_texts(host, false, &telegram),
            vec![
                vec![text(Report)],
                vec![text(Custom), text(Yesterday), text(Daily)]
            ]
        );
    }
    let labels = super::telegram_labels(crate::HostKind::Station);
    for item in [Report, Status, Custom, Yesterday, Daily, Settings] {
        assert_eq!(
            parse_reply_button(&text(item), &labels),
            moon_core::telegram::commands::button_command(item),
            "{}",
            item.id()
        );
    }
}

/// The keyboard's former Mini App button, still installed in a chat, opens the app in any language.
#[test]
fn a_former_mini_app_button_still_opens_the_app() {
    let labels = super::telegram_labels(crate::HostKind::Terminal);
    for locale in ["ru", "en", "es"] {
        let text = format!(
            "\u{1f4f1} {}",
            rust_i18n::t!("telegram.button_miniapp", locale = locale)
        );
        assert_eq!(
            parse_reply_button(&text, &labels),
            moon_core::telegram::commands::ParsedCommand::MiniApp,
            "{text}"
        );
    }
}

/// A menu with nothing left to show for a chat keeps the Report section as a way in.
#[test]
fn an_empty_keyboard_keeps_the_report_section() {
    let _locale = crate::test_locale::force("ru");
    use moon_core::config::telegram_menu::{MenuEntry, MenuItem};
    let mut telegram = moon_core::config::TelegramConfig::default();
    telegram.bot.menu = full_menu(vec![vec![MenuEntry::shown(MenuItem::Status)]]);
    let locale = rust_i18n::locale();
    // A viewer: Status and Settings are not theirs.
    assert_eq!(
        keyboard_texts(crate::HostKind::Terminal, false, &telegram),
        vec![vec![super::button_text(MenuItem::Report, locale.as_ref())]]
    );
}

/// No two items share a button text in any language, so a press always resolves to one item.
#[test]
fn button_texts_are_distinct_in_every_locale() {
    use moon_core::config::telegram_menu::MenuItem;
    for locale in ["ru", "en", "es"] {
        let mut texts: Vec<String> = MenuItem::ALL
            .into_iter()
            .map(|item| super::button_text(item, locale))
            .collect();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), MenuItem::ALL.len(), "{locale}");
        for item in MenuItem::ALL {
            let key = format!("telegram.button_{}", item.id());
            assert_ne!(
                rust_i18n::t!(&key, locale = locale),
                key,
                "{key} is missing in {locale}"
            );
        }
    }
}

/// A keyboard of `rows` with every other item listed hidden, as a saved menu always lists them.
fn full_menu(
    rows: Vec<Vec<moon_core::config::telegram_menu::MenuEntry>>,
) -> moon_core::config::telegram_menu::BotMenu {
    use moon_core::config::telegram_menu::{BotMenu, MenuEntry, MenuItem};
    let listed: Vec<MenuItem> = rows.iter().flatten().map(|e| e.item).collect();
    let rest: Vec<MenuEntry> = MenuItem::ALL
        .iter()
        .filter(|item| !listed.contains(item))
        .map(|&item| MenuEntry::hidden(item))
        .collect();
    let mut keyboard = rows;
    keyboard.push(rest);
    BotMenu { keyboard }.normalized()
}

/// The command list has a description for every command every chat may use, on both hosts, and
/// nothing of the owner's (Settings, Status) that a viewer would only be refused.
#[test]
fn every_listed_command_has_a_description() {
    for host in [crate::HostKind::Terminal, crate::HostKind::Station] {
        let labels = super::telegram_labels(host);
        for command in [
            "today",
            "yesterday",
            "month",
            "lastmonth",
            "daily",
            "hour",
            "help",
        ] {
            let key = format!("command_{command}");
            assert!(
                labels.get(&key).is_some_and(|text| !text.is_empty()),
                "{key}"
            );
        }
        assert!(!labels.contains_key("command_status"));
        assert!(!labels.contains_key("command_settings"));
    }
}
