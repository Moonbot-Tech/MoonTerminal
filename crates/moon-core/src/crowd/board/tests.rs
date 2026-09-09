use super::*;

#[test]
fn a_hidden_trader_is_not_printed_as_an_at_sign() {
    let hidden = Trader {
        place: 4,
        id: 2361,
        handle: "@".to_string(),
        profit: 26.83,
        trades: 68,
    };
    assert!(hidden.anonymous());
    assert_eq!(hidden.name(), "—");
}

#[test]
fn a_named_trader_loses_the_at_sign_and_nothing_else() {
    let named = Trader {
        place: 1,
        id: 404,
        handle: "@moon_bot_kurilka".to_string(),
        profit: 45_550.51,
        trades: 54_052,
    };
    assert!(!named.anonymous());
    assert_eq!(named.name(), "moon_bot_kurilka");
}

#[test]
fn an_empty_handle_counts_as_hidden() {
    // The service has been seen to send a bare `@`; an empty string is the same thing said
    // differently, and printing it would leave a blank cell that reads as a broken row.
    let blank = Trader {
        handle: "  ".to_string(),
        ..Trader::default()
    };
    assert!(blank.anonymous());
    assert_eq!(blank.name(), "—");
}

#[test]
fn a_handle_with_a_space_in_front_still_loses_its_at_sign() {
    // `anonymous` trims and `name` did not, so a handle the service padded printed its `@`.
    let padded = Trader {
        handle: "  @bob".to_string(),
        ..Trader::default()
    };
    assert!(!padded.anonymous());
    assert_eq!(padded.name(), "bob");
}
