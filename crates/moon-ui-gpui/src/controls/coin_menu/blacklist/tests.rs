//! Section ordering for the combined blacklist submenu.

use super::{Section, UndoStep, submenu_plan, temp_added_caption};

/// Keep temporary presets between permanent targets and the places dialog.
#[test]
fn blacklist_submenu_order_permanent_sep_temp_sep_where() {
    use Section::*;
    assert_eq!(
        submenu_plan(true),
        [Permanent, Sep, Temporary, Sep, WhereListed]
    );
    assert_eq!(submenu_plan(false), [Permanent, Sep, WhereListed]);
    let (_, lift) = include_str!("../blacklist.rs")
        .split_once("if let Some(label) = held.lift_label()")
        .unwrap();
    let (before_lift_row, _) = lift
        .split_once("MoonMenuItem::with_key(\"tbl-clear\"")
        .unwrap();
    assert!(!before_lift_row.contains("MoonMenuItem::separator()"));
}

/// Temporary-ban feedback uses the printed ticker and uses the few form for two markets.
#[test]
fn temp_toast_names_ticker_not_symbol() {
    let _locale = crate::test_locale::force("ru");
    let symbol = "XALPHAUSDT".to_string();
    let first = UndoStep::temp_sent(
        1,
        "XALPHA",
        Some(move |_: &moon_core::session::SessionManager| Ok(!symbol.is_empty())),
    )
    .unwrap();
    let caption = temp_added_caption(std::slice::from_ref(&first), "1 ч", |_| {
        "CORE_A".to_string()
    });
    assert_eq!(caption, "XALPHA во временном ЧС ядра CORE_A на 1 ч");
    let second = UndoStep::temp_sent(
        1,
        "XBETA",
        Some(|_: &moon_core::session::SessionManager| Ok(true)),
    )
    .unwrap();
    assert_eq!(
        temp_added_caption(&[first, second], "1 ч", |_| "CORE_A".to_string()),
        "XALPHA, XBETA во временном ЧС на 1 ч · CORE_A (2 рынка)"
    );
}
