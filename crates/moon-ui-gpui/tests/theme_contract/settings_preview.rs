//! Static contract for the Telegram preview bubble in Settings > Messages.

use super::support::{code_only, read_src};

/// Catches the bubble going back to content sizing: a shrink-wrapped bubble (`min_w_0` without a
/// width) measured near zero in the real window and stacked its text a few glyphs per line,
/// while the test harness laid it out fine. Its width must come from `bubble_w`, computed from
/// the card width the page passes down.
#[test]
fn the_telegram_preview_bubble_has_a_computed_width() {
    let preview = code_only(&read_src("settings/telegram/message_layout/preview.rs"));
    let at = preview
        .find("\"tg-preview-bubble\"")
        .expect("the preview bubble keeps its debug selector");
    let bubble: String = preview[at..].lines().take(4).collect::<Vec<_>>().join("\n");
    assert!(
        bubble.contains(".w(px(bubble))") && bubble.contains(".flex_none()"),
        "the preview bubble must take its explicit width: {bubble}"
    );
    assert!(
        !bubble.contains("min_w_0") && !bubble.contains("max_w("),
        "the preview bubble must not be content-sized: {bubble}"
    );
    assert!(
        preview.contains("let bubble = bubble_w(self.card_w, cx);"),
        "the bubble width comes from bubble_w over the card width"
    );
    let page = code_only(&read_src("settings/telegram.rs"));
    assert!(
        page.contains("self.message_layout_box(side, card_w, cx)")
            && page.contains("let card_w = card_grid_col_w(column_w, 2, cx);"),
        "the Messages page passes the card column's width to the preview"
    );
}
