//! Navigation keyboard regressions: every visible button resolves to its command.

use moon_core::telegram::{
    commands::{ParsedCommand, parse_reply_button},
    report::Period,
};

/// The reply keyboard is two rows: short period labels with Help, then the two month labels.
#[test]
fn persistent_keyboard_fits_in_two_rows() {
    let labels = super::telegram_labels();
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) = super::navigation_keyboard() else {
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
    let labels = super::telegram_labels();
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) = super::navigation_keyboard() else {
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
