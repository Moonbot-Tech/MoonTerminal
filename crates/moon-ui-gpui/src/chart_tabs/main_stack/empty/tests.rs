use super::detect::{crowd_cards, crowd_rule, field_text};
use super::{EmptyScreen, SWITCHES};
use moon_core::config::layout::WindowLayout;
use moon_core::crowd::detect::{DEFAULT_PROFIT, DEFAULT_TRADES};

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
        !screen.detect,
        "the crowd rule was watched without being asked for"
    );
    assert!(
        !screen.parts().any(),
        "a fresh profile opened a connection to the statistics service"
    );
    // The rule is the one switch that costs a connection while nothing is drawn at all, so its
    // default is the one that matters most here.
    let rule = crowd_rule(&WindowLayout::default());
    assert!(
        !rule.enabled,
        "a fresh profile watched the market for detections"
    );
    assert_eq!(rule.profit, DEFAULT_PROFIT);
    assert_eq!(rule.trades, DEFAULT_TRADES);
}

/// The switch and the service read the SAME three keys, through one function: two readers of a
/// persisted setting is how a checkbox and the thing it controls come to disagree.
#[test]
fn the_rule_the_service_watches_is_the_one_the_popup_shows() {
    let mut layout = WindowLayout::default();
    layout.main_empty_detect = Some(true);
    layout.main_empty_detect_profit = Some(2500.0);
    layout.main_empty_detect_trades = Some(25);

    let rule = crowd_rule(&layout);
    assert!(rule.enabled);
    assert_eq!(rule.profit, 2500.0);
    assert_eq!(rule.trades, 25);
    assert!(
        EmptyScreen::restore(&layout).detect,
        "the checkbox and the rule read the same key differently"
    );
}

/// A threshold is written back as a person would type it, and reads back as the SAME number.
///
/// The popup writes what it shows, so a rounding on the way to the field would edit the threshold
/// every time somebody merely opened the popup and closed it again.
#[test]
fn a_threshold_survives_a_trip_through_its_field() {
    assert_eq!(field_text(1000.0), "1000");
    assert_eq!(field_text(1234.5), "1234.5");
    for value in [1000.0, 1234.5, 1234.567, 0.01, 999_999.99] {
        assert_eq!(
            field_text(value).parse::<f64>(),
            Ok(value),
            "the field rounded {value} on its way to the screen"
        );
    }
}

/// A hand-edited file cannot hand the service a rule a rule cannot be.
///
/// Mutation: drop the sanity pass and `nan` reaches the detector, where it compares unequal to
/// itself — so every render hands the service a "changed" rule, which re-seeds and silences it for
/// good while re-syncing the feed at frame rate.
#[test]
fn a_hand_edited_file_cannot_break_the_rule() {
    let mut layout = WindowLayout::default();
    layout.main_empty_detect = Some(true);

    layout.main_empty_detect_profit = Some(f64::NAN);
    let rule = crowd_rule(&layout);
    assert!(rule.profit.is_finite(), "a NaN threshold reached the rule");
    assert_eq!(
        rule,
        crowd_rule(&layout),
        "the rule is not even equal to itself"
    );

    // A negative line is KEPT — it is read as written, `> -500` being "not worse than five hundred
    // down" — while an absurd one is held to what a line can be.
    layout.main_empty_detect_profit = Some(-500.0);
    assert_eq!(crowd_rule(&layout).profit, -500.0);
    layout.main_empty_detect_profit = Some(-1e300);
    assert!(crowd_rule(&layout).profit.is_finite());
    assert!(
        crowd_rule(&layout).profit < 0.0,
        "a huge negative line lost its sign"
    );

    layout.main_empty_detect_profit = Some(f64::INFINITY);
    assert!(crowd_rule(&layout).profit.is_finite());

    // Zero SURVIVES sanitising on both lines: it is what switches a line off, and clamping it away
    // would silently turn a switched-off line back on.
    layout.main_empty_detect_profit = Some(0.0);
    layout.main_empty_detect_trades = Some(0);
    let quiet = crowd_rule(&layout);
    assert_eq!(quiet.profit, 0.0);
    assert_eq!(quiet.trades, 0);
    assert!(
        !quiet.armed(),
        "a rule with both lines off still claimed the feed"
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

/// A card's lifetime is held to what a card can actually have, and its seat policy has a default
/// that follows the market.
///
/// Mutation: drop the clamp. A hand-edited `0` then prunes every card on the pass that takes it in,
/// so the rule looks broken rather than fast; an hour makes the feed a list of history occupying
/// every seat it has.
#[test]
fn a_card_lifetime_is_held_to_what_a_card_can_be() {
    let mut layout = WindowLayout::default();
    let default = crowd_cards(&layout);
    assert_eq!(
        default.keep_ms(),
        moon_core::crowd::minute::WINDOW_MS as f64,
        "the default is the window the figures were computed from"
    );
    assert!(default.evict, "the feed stopped following the market");

    layout.main_empty_detect_keep = Some(0);
    assert!(
        crowd_cards(&layout).keep_secs >= 5,
        "a card has to survive being noticed and clicked"
    );
    layout.main_empty_detect_keep = Some(9_000);
    assert_eq!(crowd_cards(&layout).keep_secs, 300);
    layout.main_empty_detect_keep = Some(120);
    assert_eq!(crowd_cards(&layout).keep_secs, 120);

    layout.main_empty_detect_evict = Some(false);
    assert!(!crowd_cards(&layout).evict);
}
