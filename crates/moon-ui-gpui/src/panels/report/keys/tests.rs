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

/// Run the actual Windows physical-key normalization against a Russian layout.
#[cfg(target_os = "windows")]
fn with_russian_layout(test: impl FnOnce()) {
    use std::sync::Mutex;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        ACTIVATE_KEYBOARD_LAYOUT_FLAGS, ActivateKeyboardLayout, GetKeyboardLayout, HKL,
        LoadKeyboardLayoutW,
    };
    use windows::core::w;

    struct Restore(HKL);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = unsafe { ActivateKeyboardLayout(self.0, ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0)) };
        }
    }

    static LAYOUT: Mutex<()> = Mutex::new(());
    let _guard = LAYOUT.lock().unwrap_or_else(|err| err.into_inner());
    let _restore = Restore(unsafe { GetKeyboardLayout(0) });
    let russian = unsafe { LoadKeyboardLayoutW(w!("00000419"), ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0)) }
        .expect("Russian keyboard layout");
    unsafe { ActivateKeyboardLayout(russian, ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0)) }
        .expect("activate Russian keyboard layout");
    test();
}

/// A configured Cyrillic spelling occupies the Report candidate on its physical key.
#[cfg(target_os = "windows")]
#[test]
fn cyrillic_configured_g_occupies_g() {
    with_russian_layout(|| {
        let mut hotkeys = HotkeysConfig::default();
        hotkeys.set_key(KeySlot::SwitchCharts, "п".into());
        let map = report_keymap(&hotkeys);
        assert_eq!(map.matches(&stroke("п")), None);
        assert_eq!(map.matches(&stroke("g")), None);
        assert_eq!(
            map.matches(&stroke("shift-п")),
            Some(ReportKey::ShowOnChart)
        );
        assert_eq!(
            map.hint(ReportKey::ShowOnChart),
            Some(Kbd::format(&stroke("shift-g")))
        );
    });
}

/// The raw path reaches Copy when the root cannot match its Latin binding.
#[cfg(target_os = "windows")]
#[test]
fn ctrl_c_cyrillic_layout_copies_ticker() {
    with_russian_layout(|| {
        let event = stroke("ctrl-с");
        let map = report_keymap(&HotkeysConfig::default());
        assert!(is_row_key_candidate(&event));
        assert_eq!(
            map.matches(&event)
                .and_then(|action| for_selection(action, 1)),
            Some(ReportKey::CopyTicker)
        );
        assert_eq!(
            map.hint(ReportKey::CopyTicker),
            Some(Kbd::format(&stroke("ctrl-c")))
        );
    });
}
