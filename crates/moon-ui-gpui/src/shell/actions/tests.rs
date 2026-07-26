//! Group ownership contracts for shared inline-edit requests.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{select_hotkey_target, take_group_edit};

/// Regression target: replacing the group check with an unconditional `request.take()` lets the
/// first repainting window steal another group's editor, so the double-click appears to do nothing.
#[test]
fn another_group_cannot_consume_an_inline_edit_request() {
    let mut request = Some(("desk-a".to_string(), 3));

    assert_eq!(take_group_edit(&mut request, "desk-b"), None);
    assert_eq!(request, Some(("desk-a".to_string(), 3)));
    assert_eq!(
        take_group_edit(&mut request, "desk-a"),
        Some(("desk-a".to_string(), 3))
    );
    assert_eq!(request, None);
}

/// Regression target: removing the hovered preference routes a docked AddToChart hotkey to hidden
/// Main, so Cancel Buy or Panic Sell can execute on another core and market than the cursor shows.
#[test]
fn hovered_group_chart_wins_over_hidden_main_for_market_hotkeys() {
    let hovered = (7, "ETHUSDT".to_string());
    let main = (9, "BTCUSDT".to_string());

    assert_eq!(
        select_hotkey_target(Some(hovered.clone()), true, Some(main.clone())),
        Some(hovered)
    );
    assert_eq!(
        select_hotkey_target(Some((11, "SOLUSDT".to_string())), false, Some(main.clone())),
        Some(main)
    );
}
