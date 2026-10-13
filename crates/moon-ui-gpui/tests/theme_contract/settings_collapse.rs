//! Static contract against the collapsing-text shape in Settings: a `min_w_0` text beside a
//! `flex_none` sibling in a content-sized row stacks one glyph per line in the real window, while
//! the GPUI test harness lays it out fine, so a layout test alone cannot catch its return.
//!
//! A general textual detector of that shape is not reliable (whether a row's width is definite
//! depends on ancestors in other functions), so each cured site is pinned here instead.

use super::support::{code_only, read_src};

/// Catches the chats table's label going back to content sizing: it stacked «Чат…208» one glyph
/// per line. The label and the id take widths from `chat_row_widths` over the card width the Bot
/// page passes down; the access words get a measured floor so they wrap rather than shrink.
#[test]
fn the_chats_table_label_has_a_computed_width() {
    let access = code_only(&read_src("settings/telegram/access.rs"));
    let at = access
        .find("\"tg-chat-title-{chat}\"")
        .expect("the chat label keeps its debug selector");
    let label: String = access[at..].lines().take(5).collect::<Vec<_>>().join("\n");
    assert!(
        label.contains(".w(px(widths.title))")
            && label.contains(".flex_none()")
            && label.contains(".truncate()"),
        "the chat label must take its explicit width and truncate: {label}"
    );
    assert!(
        !label.contains("min_w_0"),
        "the chat label must not be content-sized: {label}"
    );
    let row = access
        .split("pub(super) fn chat_row(")
        .nth(1)
        .expect("chat_row exists");
    assert!(
        row.contains("let widths = chat_row_widths(card_w, chat, &title, &summary, cx);")
            && row.contains(".min_w(px(widths.role))"),
        "chat_row sizes its cells from chat_row_widths over the card width"
    );
    let page = code_only(&read_src("settings/telegram.rs"));
    assert!(
        page.contains("let card_w = card_grid_col_w(column_w, 3, cx);")
            && page.contains("self.terminal_bot_sections(card_w, cx)")
            && page.contains("self.server_bot_sections(card_w, cx)"),
        "the Bot page passes the card column's width to the chats card on both hosts"
    );
}
