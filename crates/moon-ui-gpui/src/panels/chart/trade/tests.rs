// Do not use `super::*`: the parent re-exports GPUI's `test` attribute macro through `gpui::*`,
// which would shadow the built-in `#[test]` and make it expand recursively.
use gpui::Modifiers;
use moon_core::config::{HotkeysConfig, MouseGestureBinding};
use moon_core::session::order_lines::LineKind;

use super::hover_probe_due;
use super::{ChartPanel, TradeMouseButton};

// `Modifiers::control`/`shift`/`alt` are gpui's own single-modifier constructors; the local
// copies this file used to keep were character-identical to them.
use Modifiers as M;

/// Pins which configured gesture pair may grab each order line.
///
/// Plausible breakage: collapsing the four buckets (entry/exit × long/short) would let a TP gesture
/// grab the entry line, moving a live limit the user meant to leave alone.
#[test]
fn move_gestures_split_entry_exit_and_direction() {
    let hk = HotkeysConfig {
        buy_move_click: MouseGestureBinding::LeftShift,
        sell_move_click: MouseGestureBinding::LeftCtrl,
        short_buy_move_click: MouseGestureBinding::Middle,
        short_sell_move_click: MouseGestureBinding::MiddleShift,
        // Short lines follow their own fields only while mirroring is off.
        same_hotkeys_for_move: false,
        ..Default::default()
    };

    let first = |kind: LineKind, short| hk.move_gestures(kind == LineKind::Buy, short)[0];
    assert_eq!(
        first(LineKind::Buy, false),
        MouseGestureBinding::LeftShift,
        "long entry uses buy_move_click"
    );
    assert_eq!(
        first(LineKind::Buy, true),
        MouseGestureBinding::Middle,
        "short entry uses short_buy_move_click"
    );
    // Every exit kind shares the sell gestures, as stops are dragged like the TP line.
    for kind in [LineKind::Sell, LineKind::Stop, LineKind::TakeProfit] {
        assert_eq!(first(kind, false), MouseGestureBinding::LeftCtrl);
        assert_eq!(first(kind, true), MouseGestureBinding::MiddleShift);
    }

    // With mirroring on, short lines follow the long gestures whatever the short fields hold — a
    // shared or hand-edited hotkeys.toml can carry the flag together with stale short values.
    let mirrored = HotkeysConfig {
        same_hotkeys_for_move: true,
        ..hk
    };
    assert_eq!(
        mirrored.move_gestures(true, true)[0],
        MouseGestureBinding::LeftShift
    );
    assert_eq!(
        mirrored.move_gestures(false, true)[0],
        MouseGestureBinding::LeftCtrl
    );
}

/// Pins gesture recognition: button, modifiers and click count must all be read.
///
/// Plausible breakage: letting a Ctrl+RIGHT press satisfy a `CTRL_Click` binding — tried once as a
/// macOS workaround, before the fork stopped rewriting Control-click into a right click — makes one
/// press match both the buy-set and short-set bindings in `try_place_order_click`, which shares
/// this matcher.
#[test]
fn gesture_matching_reads_button_modifiers_and_click_count() {
    let m = |binding, button, modifiers, clicks| {
        ChartPanel::gesture_matches(binding, button, modifiers, clicks)
    };
    assert!(m(
        MouseGestureBinding::LeftShift,
        TradeMouseButton::Left,
        M::shift(),
        1
    ));
    assert!(!m(
        MouseGestureBinding::LeftShift,
        TradeMouseButton::Left,
        M::control(),
        1
    ));
    // A double-click binding requires the second press; a single one is not it.
    assert!(m(
        MouseGestureBinding::LeftDouble,
        TradeMouseButton::Left,
        Modifiers::default(),
        2
    ));
    assert!(!m(
        MouseGestureBinding::LeftDouble,
        TradeMouseButton::Left,
        Modifiers::default(),
        1
    ));
    // `None` never matches, or an unset slot would grab every press.
    assert!(!m(
        MouseGestureBinding::None,
        TradeMouseButton::Left,
        Modifiers::default(),
        1
    ));
    assert!(m(
        MouseGestureBinding::LeftCtrl,
        TradeMouseButton::Left,
        M::control(),
        1
    ));
    assert!(
        !m(
            MouseGestureBinding::LeftCtrl,
            TradeMouseButton::Right,
            M::control(),
            1
        ),
        "Ctrl+right must stay a gesture of its own on every platform"
    );
}

/// Pins the MouseMove hot-path thresholds enforced by `hover_probe_due`.
///
/// Plausible breakage: changing `trade.rs::hover_probe_due` to accept sub-threshold jitter or miss
/// an exact boundary would either rescan every order line on redundant moves or delay hover at the
/// specified one-pixel X and half-pixel Y thresholds.
#[test]
fn hover_probe_threshold_matches_delphi() {
    // The first visit always probes.
    assert!(hover_probe_due(None, (10.0, 10.0)));
    // Sub-threshold mouse-move jitter does not probe again.
    assert!(!hover_probe_due(Some((10.0, 10.0)), (10.0, 10.0)));
    assert!(!hover_probe_due(Some((10.0, 10.0)), (10.9, 10.4)));
    // Movement at either the one-pixel X or half-pixel Y boundary probes in both directions.
    assert!(hover_probe_due(Some((10.0, 10.0)), (11.0, 10.0)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (10.0, 10.5)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (9.0, 10.0)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (10.0, 9.5)));
}

#[test]
fn the_chart_input_channel_prefix_still_matches_this_module() {
    // moon-core cannot check this itself: the prefix names modules that live in the BINARY, so
    // over there the constant can only be compared with another copy of itself — which is how the
    // switch spent its whole life inert, pointing at `moon_ui_gpui`, a name no record ever carried.
    // `module_path!()` here is the ground truth, and it moves with a `[[bin]]` rename or a module
    // move — the two edits that would silently mute the channel again.
    let prefix = moon_core::diagnostics::CHART_INPUT_TARGET;
    assert!(
        module_path!().starts_with(prefix),
        "log.chart_input matches {prefix:?}, but this module logs as {:?} — the channel would be \
         inert and nothing else would say so",
        module_path!()
    );
}

/// The WHOLE gesture matcher, pinned as a table: which press each of the 16 bindings answers to.
///
/// This is the contract every mouse binding in the settings tab rests on — order placement, the
/// four move gestures, the figure-delete click. It has no test of its own today, so a change to
/// `gesture_matches` (a widened arm, a dropped `clear` test, a new modifier rule) is invisible
/// until a trader reports that a gesture stopped firing or started firing twice.
///
/// Plausible breakage: dropping the `clear` requirement from `Middle` would make Shift+middle
/// satisfy a plain-middle binding, so the X-scale sync and any middle-bound gesture would both
/// answer one press.
#[test]
fn every_gesture_binding_answers_exactly_the_presses_it_names() {
    use MouseGestureBinding as G;
    use TradeMouseButton as B;

    let none = Modifiers::default();
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    // (binding, button, modifiers, click count) -> matches?
    let expect = [
        // Plain buttons answer only an unmodified press.
        (G::Middle, B::Middle, none, 1, true),
        (G::Middle, B::Middle, M::control(), 1, false),
        (G::Middle, B::Middle, M::shift(), 1, false),
        (G::Middle, B::Middle, alt, 1, false),
        (G::Middle, B::Left, none, 1, false),
        (G::Middle, B::Right, none, 1, false),
        // ... and at any click count: a double middle click is still two middle presses.
        (G::Middle, B::Middle, none, 2, true),
        // Modified buttons answer their own modifier, whatever else is held.
        (G::MiddleCtrl, B::Middle, M::control(), 1, true),
        (G::MiddleCtrl, B::Middle, none, 1, false),
        (G::MiddleShift, B::Middle, M::shift(), 1, true),
        (G::MiddleAlt, B::Middle, alt, 1, true),
        (G::LeftCtrl, B::Left, M::control(), 1, true),
        (G::LeftShift, B::Left, M::shift(), 1, true),
        (G::LeftAlt, B::Left, alt, 1, true),
        (G::RightCtrl, B::Right, M::control(), 1, true),
        (G::RightShift, B::Right, M::shift(), 1, true),
        (G::RightAlt, B::Right, alt, 1, true),
        // Doubles need the second press AND a clear modifier set.
        (G::LeftDouble, B::Left, none, 2, true),
        (G::LeftDouble, B::Left, none, 1, false),
        (G::LeftDouble, B::Left, M::control(), 2, false),
        (G::RightDouble, B::Right, none, 2, true),
        (G::RightDouble, B::Right, none, 1, false),
        // Modified doubles need both halves.
        (G::LeftCtrlDouble, B::Left, M::control(), 2, true),
        (G::LeftCtrlDouble, B::Left, M::control(), 1, false),
        (G::LeftCtrlDouble, B::Left, none, 2, false),
        (G::LeftShiftDouble, B::Left, M::shift(), 2, true),
        (G::LeftAltDouble, B::Left, alt, 2, true),
        // `None` is the off switch: it answers nothing at all.
        (G::None, B::Left, none, 1, false),
        (G::None, B::Middle, none, 1, false),
        (G::None, B::Right, none, 2, false),
    ];

    for (binding, button, modifiers, clicks, matches) in expect {
        assert_eq!(
            ChartPanel::gesture_matches(binding, button, modifiers, clicks),
            matches,
            "{binding:?} vs {button:?} mods={modifiers:?} clicks={clicks}"
        );
    }
}

/// WHICH presses answer two bindings at once — the overlap the settings page cannot show today.
///
/// A modified DOUBLE click also satisfies the single-click binding of the same modifier, because
/// `Ctrl+Left` is defined as "left button with Control held", at any count. So `CTRL_Click` and
/// `CTRL_Dbl` on two different slots both accept the user's second press, and which one acts is
/// decided by the order of branches in `mouse_down_left` — nothing in Settings says so.
///
/// Pinned as a fact, not as a defect: this is what the conflict indicator (docs-internal/
/// HOTKEYS_UNIFIED_PLAN.md) has to report, and it is what a change to `gesture_matches` must not
/// widen. Everything NOT listed here stays unambiguous.
#[test]
fn only_the_modified_doubles_overlap_their_single_click_twins() {
    use MouseGestureBinding as G;
    use TradeMouseButton as B;

    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    // The three known overlaps, each on the second press of a modified left click.
    let known = [
        (
            B::Left,
            M::control(),
            2usize,
            vec![G::LeftCtrl, G::LeftCtrlDouble],
        ),
        (
            B::Left,
            M::shift(),
            2,
            vec![G::LeftShift, G::LeftShiftDouble],
        ),
        (B::Left, alt, 2, vec![G::LeftAlt, G::LeftAltDouble]),
    ];

    for button in [B::Left, B::Middle, B::Right] {
        for modifiers in [Modifiers::default(), M::control(), M::shift(), alt] {
            for clicks in [1usize, 2] {
                let hit: Vec<G> = G::ALL
                    .into_iter()
                    .filter(|g| ChartPanel::gesture_matches(*g, button, modifiers, clicks))
                    .collect();
                let expected = known
                    .iter()
                    .find(|(b, m, c, _)| *b == button && *m == modifiers && *c == clicks)
                    .map(|(_, _, _, both)| both.clone());
                match expected {
                    Some(both) => assert_eq!(
                        hit, both,
                        "{button:?} mods={modifiers:?} clicks={clicks}: known overlap changed"
                    ),
                    None => assert!(
                        hit.len() <= 1,
                        "{button:?} mods={modifiers:?} clicks={clicks} now answers {hit:?} — a new \
                         overlap, so one press means two gestures"
                    ),
                }
            }
        }
    }
}

/// Pins which of the four placement gestures a press satisfies, and that a pending is a SEPARATE
/// intent from an immediate order rather than a variant of one.
///
/// Plausible breakage: giving the pending rows the immediate branch — or the reverse — sends the
/// clicked price as an ENTRY where the wire expects a trigger condition, so an order opens at once
/// at a price the trader meant the market to have to reach first.
#[test]
fn placement_gestures_split_side_and_pending() {
    let hk = HotkeysConfig {
        buy_set_click: MouseGestureBinding::LeftDouble,
        short_set_click: MouseGestureBinding::MiddleShift,
        pending_long_click: MouseGestureBinding::LeftAlt,
        pending_short_click: MouseGestureBinding::MiddleAlt,
        ..Default::default()
    };
    let intent =
        |button, modifiers, clicks| super::placement_intent(&hk, button, modifiers, clicks);

    assert_eq!(
        intent(TradeMouseButton::Left, Modifiers::default(), 2),
        Some(moon_core::config::Placement {
            short: false,
            pending: false
        }),
    );
    assert_eq!(
        intent(TradeMouseButton::Middle, M::shift(), 1),
        Some(moon_core::config::Placement {
            short: true,
            pending: false
        }),
    );
    assert_eq!(
        intent(TradeMouseButton::Left, M::alt(), 1),
        Some(moon_core::config::Placement {
            short: false,
            pending: true
        }),
    );
    assert_eq!(
        intent(TradeMouseButton::Middle, M::alt(), 1),
        Some(moon_core::config::Placement {
            short: true,
            pending: true
        }),
    );
    // A press no placement row holds is not placement, whatever else it may be.
    assert_eq!(intent(TradeMouseButton::Left, M::control(), 1), None);
    assert_eq!(
        intent(TradeMouseButton::Right, Modifiers::default(), 1),
        None
    );
}

/// A press held by two placement rows goes to the EARLIER row, and that order is the one the
/// settings page's clash caption reports.
///
/// Plausible breakage: trying the pending rows first would make every duplicate fire the pending
/// while the page keeps naming the immediate row as the winner.
#[test]
fn a_shared_placement_gesture_goes_to_the_immediate_row() {
    let hk = HotkeysConfig {
        buy_set_click: MouseGestureBinding::LeftAlt,
        pending_long_click: MouseGestureBinding::LeftAlt,
        short_set_click: MouseGestureBinding::MiddleAlt,
        pending_short_click: MouseGestureBinding::MiddleAlt,
        ..Default::default()
    };

    assert_eq!(
        super::placement_intent(&hk, TradeMouseButton::Left, M::alt(), 1),
        Some(moon_core::config::Placement {
            short: false,
            pending: false
        }),
    );
    assert_eq!(
        super::placement_intent(&hk, TradeMouseButton::Middle, M::alt(), 1),
        Some(moon_core::config::Placement {
            short: true,
            pending: false
        }),
    );
}
