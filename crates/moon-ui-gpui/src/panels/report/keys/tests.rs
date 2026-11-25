//! Report key arbitration and hint regressions.

use super::*;
use moon_core::config::KeySlot;

/// Parse a key written in GPUI's binding notation.
fn stroke(binding: &str) -> Keystroke {
    Keystroke::parse(binding).unwrap()
}

/// Every default hint represents the binding actually routed by the same map.
#[test]
fn hint_equals_bound_key_for_defaults() {
    let map = report_keymap(&HotkeysConfig::default());
    let copy = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    };
    for (action, binding) in [
        (ReportKey::OpenWindow, "enter"),
        (ReportKey::ShowOnChart, "g"),
        (ReportKey::TradeLog, "l"),
        (ReportKey::FilterCoin, "f"),
        (ReportKey::CopyTicker, copy),
    ] {
        let event = stroke(binding);
        assert_eq!(map.matches(&event), Some(action));
        assert_eq!(map.hint(action), Some(Kbd::format(&event)));
    }
    assert_eq!(map.matches(&stroke("ctrl-a")), None);
}

/// A global G binding moves both Report dispatch and its menu caption to Shift+G.
#[test]
fn taken_key_falls_back_and_hint_follows() {
    let mut hotkeys = HotkeysConfig::default();
    hotkeys.set_key(KeySlot::SwitchCharts, "g".into());
    let map = report_keymap(&hotkeys);

    assert_eq!(map.matches(&stroke("g")), None);
    assert_eq!(
        map.matches(&stroke("shift-g")),
        Some(ReportKey::ShowOnChart)
    );
    assert_eq!(
        map.hint(ReportKey::ShowOnChart),
        Some(Kbd::format(&stroke("shift-g"))),
    );
}

/// Claiming all three chart candidates hides its hint and leaves every key to the shell.
#[test]
fn exhausted_candidates_show_no_hint() {
    let mut hotkeys = HotkeysConfig::default();
    hotkeys.set_key(KeySlot::OrderSize(0), "g".into());
    hotkeys.set_key(KeySlot::OrderSize(1), "shift-g".into());
    hotkeys.set_key(KeySlot::OrderSize(2), "ctrl-shift-g".into());
    let map = report_keymap(&hotkeys);

    assert_eq!(map.hint(ReportKey::ShowOnChart), None);
    for binding in ["g", "shift-g", "ctrl-shift-g"] {
        assert_eq!(map.matches(&stroke(binding)), None);
    }
}

/// L routes to the trade log when there is exactly one trade to scan.
#[test]
fn l_with_one_selected_row_maps_to_trade_log() {
    let map = report_keymap(&HotkeysConfig::default());
    assert_eq!(
        map.matches(&stroke("l"))
            .and_then(|action| for_selection(action, 1)),
        Some(ReportKey::TradeLog),
    );
}

/// A chart or log shortcut in a multi-selection must propagate to other handlers.
#[test]
fn g_with_many_rows_not_consumed() {
    let map = report_keymap(&HotkeysConfig::default());
    for binding in ["g", "l"] {
        let action = map.matches(&stroke(binding)).unwrap();
        assert_eq!(for_selection(action, 5), None);
    }
    assert_eq!(
        for_selection(ReportKey::FilterCoin, 5),
        Some(ReportKey::FilterCoin)
    );
    assert_eq!(
        for_selection(ReportKey::OpenWindow, 5),
        Some(ReportKey::OpenWindow)
    );
}
