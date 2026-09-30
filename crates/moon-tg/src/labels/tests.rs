//! Navigation keyboard regressions: every visible button resolves to its command.

use moon_core::telegram::{
    commands::{ParsedCommand, parse_reply_button},
    report::Period,
};

/// The reply keyboard is two rows: short period labels with Help, then the two month labels.
#[test]
fn persistent_keyboard_fits_in_two_rows() {
    let labels = super::telegram_labels(crate::HostKind::Terminal);
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) =
        super::navigation_keyboard(crate::HostKind::Terminal)
    else {
        panic!("expected persistent keyboard")
    };
    assert_eq!(markup.keyboard.len(), 2);
    assert_eq!(markup.keyboard[0].len(), 3);
    assert_eq!(markup.keyboard[1].len(), 2);
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
}

/// Every visible emoji button must resolve through exact localized aliases without guessing text.
#[test]
fn persistent_navigation_buttons_have_recognized_commands() {
    let labels = super::telegram_labels(crate::HostKind::Terminal);
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) =
        super::navigation_keyboard(crate::HostKind::Terminal)
    else {
        panic!("expected persistent keyboard")
    };
    assert!(markup.is_persistent);
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

/// A station's keyboard carries its "Status" after the months, and only a station's labels parse
/// it: a terminal's bot has no station to report on.
#[test]
fn only_the_station_keyboard_has_its_status() {
    let labels = super::telegram_labels(crate::HostKind::Station);
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) =
        super::navigation_keyboard(crate::HostKind::Station)
    else {
        panic!("expected persistent keyboard")
    };
    assert_eq!(markup.keyboard[0].len(), 3);
    assert_eq!(markup.keyboard[1].len(), 3);
    let status = &markup.keyboard[1][2].text;
    assert_eq!(
        parse_reply_button(status, &labels),
        ParsedCommand::StationStatus
    );
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
        for locale in ["ru", "en", "es"] {
            let path = format!("telegram.{key}_station");
            let text = rust_i18n::t!(&path, locale = locale).to_lowercase();
            assert_ne!(text, path.to_lowercase(), "{path} is missing in {locale}");
            assert!(
                !text.contains("terminal") && !text.contains("терминал") || key.contains("denied"),
                "{path} ({locale}) names the terminal: {text}"
            );
        }
    }
    assert_eq!(
        super::report_delivery_failed(crate::HostKind::Station),
        station["report_delivery_failed"]
    );
}
