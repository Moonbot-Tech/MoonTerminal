//! Static contracts for the Detects panel's narrow-dock empty state.

use super::support::*;

/// `panels/detects/mod.rs:DetectsPanel::render` must keep the empty feed in a column and give its
/// sentence a definite width. Replacing `v_flex()` with a centred flex row or hanging
/// `empty_feed_text(...)` directly on that row makes every empty-state sentence render as one line
/// clipped on both sides in the reported roughly 290-pixel side-dock screenshot.
#[test]
fn detects_empty_state_keeps_a_column_and_a_definite_text_width() {
    let source = read_src("panels/detects/mod.rs");
    let render = code_only(braced_body(
        &source,
        "fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement",
    ));
    let empty_state = render
        .split_once("let body: AnyElement = if shown == 0 {")
        .unwrap_or_else(|| {
            panic!("the Detects render must retain its shown == 0 empty-state branch")
        })
        .1
        .split_once("} else {")
        .unwrap_or_else(|| {
            panic!("the Detects empty state must remain separate from its scroll box")
        })
        .0
        .trim_start();

    assert!(
        empty_state.starts_with("v_flex()"),
        "the Detects empty state must stay a column so its sentence can wrap instead of clipping on both sides in a narrow side dock"
    );
    assert!(
        !empty_state.starts_with("div().flex().items_center()"),
        "the Detects empty state must not restore the centred flex row that produced the clipped narrow-dock screenshot"
    );

    let sentence_box = empty_state
        .split_once("div()")
        .unwrap_or_else(|| {
            panic!("the Detects empty sentence needs its own box to avoid the clipped narrow-dock screenshot")
        })
        .1
        .split_once(".child(empty_feed_text(")
        .unwrap_or_else(|| {
            panic!("the Detects empty sentence must stay inside its own box instead of clipping in a narrow side dock")
        })
        .0;
    assert!(
        sentence_box.contains(".w_full()"),
        "the Detects empty sentence box must have a definite width so every empty state wraps instead of clipping on both sides"
    );
    assert!(
        sentence_box.contains(".text_center()"),
        "wrapped Detects empty-state lines must remain centred in the narrow-dock screenshot"
    );
}
