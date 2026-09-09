use super::{EmptyScreen, SWITCHES};
use moon_core::config::layout::WindowLayout;

#[test]
fn a_profile_that_has_never_chosen_gets_the_screen_the_terminal_always_had() {
    // The logo and its one line of help, and nothing else. The three tables read a public service
    // over the network, which is not something an update may start doing on somebody's behalf —
    // so they are off until a person switches them on.
    let screen = EmptyScreen::restore(&WindowLayout::default());
    assert!(screen.logo, "the logo was off on a fresh profile");
    assert!(screen.hint, "the hint was off on a fresh profile");
    assert!(!screen.minute, "the minute was on without being asked for");
    assert!(
        !screen.traders,
        "the trader board was on without being asked for"
    );
    assert!(
        !screen.coins,
        "the coin board was on without being asked for"
    );
    assert!(
        !screen.parts().any(),
        "a fresh profile opened a connection to the statistics service"
    );
}

#[test]
fn a_choice_that_was_made_outlives_the_default() {
    // Each switch is stored as an OPTION so that "never chosen" and "chosen off" stay different
    // things: a default that changes later must not overrule somebody who switched it off.
    let mut layout = WindowLayout::default();
    for switch in &SWITCHES {
        (switch.store)(&mut layout, !switch.default);
    }
    let screen = EmptyScreen::restore(&layout);
    for switch in &SWITCHES {
        assert_eq!(
            (switch.read)(&screen),
            !switch.default,
            "{} came back at its default after being set",
            switch.id
        );
    }
}
