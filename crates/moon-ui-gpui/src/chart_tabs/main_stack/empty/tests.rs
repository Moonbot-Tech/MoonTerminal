use super::detect::{crowd_cards, crowd_rule, field_text};
use super::{EmptyScreen, SWITCHES, empty_logo};
use moon_core::config::layout::{EmptyBlock, EmptyPlaces, EmptySlot, WindowLayout};
use moon_core::crowd::detect::{DEFAULT_PROFIT, DEFAULT_TRADES};

/// Even a viewport smaller than the usual popup must not inherit a hard minimum height.
#[test]
fn settings_scroll_budget_never_exceeds_the_window() {
    assert_eq!(
        super::popup_body_height(gpui::px(300.0), gpui::px(64.0)),
        gpui::px(236.0)
    );
    assert_eq!(
        super::popup_body_height(gpui::px(40.0), gpui::px(64.0)),
        gpui::px(0.0)
    );
}

/// Regress the wiring that previously left a second group's retained selects stale and treated
/// a nested menu click as an outside click. These controls cannot be instantiated without GPUI.
#[test]
fn placement_popup_keeps_nested_menus_and_shared_values_live() {
    let source = include_str!("../empty.rs");
    let settings = source
        .split("pub(super) fn empty_settings(")
        .nth(1)
        .expect("settings renderer");
    assert!(settings.contains(".overlay_closable(false)"));
    // The whole call, so the values shown are read from the shared layout on THIS render and not
    // from anything cached: `layout` is bound from `self.backend` right above the call.
    assert!(settings.contains("let layout = &self.backend.read(cx).layout;"));
    assert!(settings.contains(concat!(
        "selects.show(\n",
        "                &EmptyScreen::restore(layout),\n",
        "                EmptyPlaces::restore(layout),\n",
        "                cx,\n",
        "            );"
    )));
    let seed = include_str!("detect.rs");
    let seed = seed
        .split("pub(super) fn seed_empty_detect(")
        .nth(1)
        .expect("popup seeding");
    assert!(seed.contains("if let Some(selects) = &self.empty_places"));
    assert!(seed.contains("selects.show(&screen, places, cx)"));
}

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
        (switch.store)(&mut layout, Some(!switch.default));
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

/// The brand switch reaches every empty surface, the two readings of its key cannot drift, and the
/// cover under the brand follows neither.
///
/// The cover is not decoration: in a chart slot it hides a stale graph left in the own pass beneath
/// the GPUI scene, in a detached window the white window backing. A switch that took the cover with
/// the mark would not hide a logo, it would reveal a dead chart.
///
/// That invariant is held by CONSTRUCTION — one builder makes the cover and puts the mark on it —
/// so what is checked here is that the builder is what the surfaces use, and that the builder keeps
/// the cover outside its own gate. An earlier version of this test compared a `rfind` inside a
/// prefix against the end of that prefix, which is true by arithmetic: it could not fail.
#[test]
fn the_logo_switch_reaches_every_empty_surface_and_the_cover_stays() {
    let mut layout = WindowLayout::default();
    assert!(empty_logo(&layout), "a fresh profile lost the brand");
    layout.main_empty_logo = Some(false);
    assert!(!empty_logo(&layout));

    // The checkbox and the drawing sites read one key through two functions, and nothing but this
    // stops them drifting — the same thing pinned for the rule's own keys above.
    for chosen in [None, Some(true), Some(false)] {
        layout.main_empty_logo = chosen;
        assert_eq!(
            EmptyScreen::restore(&layout).logo,
            empty_logo(&layout),
            "the checkbox and the surfaces disagree about {chosen:?}"
        );
    }

    // The builder itself: everything that makes the cover a COVER is applied before the gate that
    // adds the mark. Searched inside that one function, so the comparison is a real one.
    let cover = include_str!("../../../design.rs")
        .split("pub fn empty_cover(")
        .nth(1)
        .and_then(|tail| {
            tail.split(
                "
}",
            )
            .next()
        })
        .expect("the shared cover must exist");
    let gate = cover
        .find(".when_some(")
        .expect("the mark must be the optional part");
    for piece in [".size_full()", ".bg(rgb(background))", ".flex()"] {
        let at = cover
            .find(piece)
            .unwrap_or_else(|| panic!("the cover must keep `{piece}`"));
        assert!(
            at < gate,
            "`{piece}` moved inside the mark's gate, so hiding the logo uncovers what the cover              was there to hide"
        );
    }

    // And the two surfaces outside this file use that builder rather than rolling their own, which
    // is what makes the invariant above theirs as well.
    for (name, src) in [
        (
            "an AddToChart stack holding no charts",
            include_str!("../../add_stack.rs"),
        ),
        (
            "a chart slot waiting for its data",
            include_str!("../../../panels/chart/render.rs"),
        ),
    ] {
        assert!(
            src.contains("design::empty_cover("),
            "{name} builds its own cover, so the switch and the plate can drift apart there"
        );
    }
}

/// Switching a block OFF must not forget where it goes. The two settings answer different
/// questions, and somebody who hid the minute and switched it back on must find it where they left
/// it rather than back in the corner it shipped in.
#[test]
fn hiding_a_block_does_not_forget_where_it_goes() {
    let mut layout = WindowLayout::default();
    EmptyBlock::Minute.store(&mut layout, Some(EmptySlot::BottomStart));
    layout.main_empty_minute = Some(true);

    assert!(EmptyScreen::restore(&layout).minute);
    assert_eq!(
        EmptyPlaces::restore(&layout).slot(EmptyBlock::Minute),
        EmptySlot::BottomStart
    );

    layout.main_empty_minute = Some(false);
    assert!(!EmptyScreen::restore(&layout).minute);
    assert_eq!(
        EmptyPlaces::restore(&layout).slot(EmptyBlock::Minute),
        EmptySlot::BottomStart,
        "switching a block off moved it"
    );
}

/// A dropdown's choice reaches the one write path, and that path writes both keys in ONE update.
/// The controls cannot be driven without GPUI, so the wiring is pinned as text: a dropdown that
/// confirmed into `set_switch` alone would hide but never move, and one that wrote the two keys
/// through two updates would repaint every window twice per click.
#[test]
fn a_dropdowns_answer_is_one_write_of_both_keys() {
    let seed = include_str!("arrange.rs")
        .split("pub(super) fn seed(")
        .nth(1)
        .expect("the dropdowns are seeded in arrange.rs");
    assert!(seed.contains("MoonSelectEvent::Confirm(Some(placement))"));
    assert!(seed.contains("this.set_placement(block, *placement, cx)"));

    let write = include_str!("../empty.rs")
        .split("pub(super) fn set_placement(")
        .nth(1)
        .and_then(|tail| tail.find("cx.refresh_windows();").map(|at| &tail[..at]))
        .expect("set_placement ends by refreshing every window");
    assert_eq!(
        write.matches("self.backend.update(").count(),
        1,
        "both keys must go through one update"
    );
    assert!(write.contains("block.store(&mut backend.layout, Some(slot))"));
    assert!(write.contains("store(&mut backend.layout, Some(true))"));
    assert!(write.contains("store(&mut backend.layout, Some(false))"));
    assert!(
        !write.contains("publish_crowd_rule"),
        "placing a block does not touch the rule's keys, so it must not republish the rule"
    );
}

/// Every block has its switch, and the rule has none: the dropdowns look their switch up by block,
/// and a block without one would fall back to the first row and flip the brand instead.
#[test]
fn every_block_has_exactly_one_switch_and_the_rule_has_none() {
    for block in EmptyBlock::ALL {
        let matches = SWITCHES
            .iter()
            .filter(|switch| switch.block == Some(block))
            .count();
        assert_eq!(matches, 1, "{block:?} has {matches} switches");
        assert_eq!(super::switch_of(block).block, Some(block));
    }
    assert_eq!(
        SWITCHES
            .iter()
            .filter(|switch| switch.block.is_none())
            .count(),
        1,
        "exactly one switch is not a block: the rule"
    );
}

/// A reset answers every block's question the way a fresh profile would — anchor AND switch
/// forgotten — and leaves the rule alone: a layout button may not stop a watch.
#[test]
fn a_reset_forgets_every_block_and_spares_the_rule() {
    let mut layout = WindowLayout::default();
    for block in EmptyBlock::ALL {
        block.store(&mut layout, Some(EmptySlot::TopCenter));
    }
    layout.main_empty_logo = Some(false);
    layout.main_empty_minute = Some(true);
    layout.main_empty_detect = Some(true);

    super::forget_arrangement(&mut layout);

    assert_eq!(EmptyPlaces::restore(&layout), EmptyPlaces::default());
    for block in EmptyBlock::ALL {
        assert_eq!(block.saved(&layout), None, "{block:?} kept an anchor");
    }
    assert_eq!(
        layout.main_empty_logo, None,
        "the brand's choice was not forgotten"
    );
    assert_eq!(
        layout.main_empty_minute, None,
        "the minute's choice was not forgotten"
    );
    let mut shipped = WindowLayout::default();
    shipped.main_empty_detect = Some(true);
    assert_eq!(
        EmptyScreen::restore(&layout),
        EmptyScreen::restore(&shipped),
        "a reset screen must be the shipped one, with the rule as it was"
    );
    assert_eq!(
        layout.main_empty_detect,
        Some(true),
        "the reset stopped the watch"
    );
}

/// Placing a block must not READ anything. The three tables each carry a connection to a public
/// service, and arranging a screen is not consent to open one — a profile that has only ever chosen
/// where things go must still be offline.
#[test]
fn arranging_the_screen_does_not_switch_the_service_on() {
    let mut layout = WindowLayout::default();
    for block in EmptyBlock::ALL {
        block.store(&mut layout, Some(EmptySlot::TopCenter));
    }

    let screen = EmptyScreen::restore(&layout);
    assert!(
        !screen.parts().any(),
        "a placement opened a connection nobody asked for"
    );
    assert!(!screen.detect(), "a placement started watching the market");
    assert!(screen.logo, "a placement changed what is drawn");
    assert!(screen.hint, "a placement changed what is drawn");
}

/// A valid 1.5x UI scale must keep controls inside the 520px minimum group window.
#[test]
fn settings_outer_width_fits_a_narrow_scaled_group_window() {
    use gpui::px;
    assert_eq!(
        super::popup_outer_width(px(561.0), px(520.0), px(15.0)),
        px(490.0)
    );
    assert_eq!(
        super::popup_outer_width(px(374.0), px(1200.0), px(10.0)),
        px(374.0)
    );
    assert_eq!(
        super::popup_outer_width(px(561.0), px(520.0), px(23.0)),
        px(474.0)
    );
}
