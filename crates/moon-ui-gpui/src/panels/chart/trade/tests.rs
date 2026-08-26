// Do not use `super::*`: the parent re-exports GPUI's `test` attribute macro through `gpui::*`,
// which would shadow the built-in `#[test]` and make it expand recursively.
use gpui::Modifiers;
use moon_core::config::{HotkeysConfig, MouseGestureBinding};
use moon_core::session::order_lines::LineKind;

use super::hover_probe_due;
use super::{ChartPanel, TradeMouseButton};

fn ctrl() -> Modifiers {
    Modifiers {
        control: true,
        ..Default::default()
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Default::default()
    }
}

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
        shift(),
        1
    ));
    assert!(!m(
        MouseGestureBinding::LeftShift,
        TradeMouseButton::Left,
        ctrl(),
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
        ctrl(),
        1
    ));
    assert!(
        !m(
            MouseGestureBinding::LeftCtrl,
            TradeMouseButton::Right,
            ctrl(),
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
