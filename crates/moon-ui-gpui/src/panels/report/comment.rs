//! Full-width comment pane under the Report table.
//!
//! Report comments are the one free-text field in the schema: they routinely outgrow their table
//! cell, which caps at 360 px and truncates the rest. The pane shows the comment of the row the
//! user last clicked across the panel's whole width, wrapping instead of truncating, so a long
//! comment is readable without widening the column or exporting the row.

use super::*;

/// Table rows the comment pane may take before it scrolls instead of growing.
///
/// The pane pushes into the table, which is the only shrinkable sibling, so an uncapped pane would
/// leave a long comment with a table of zero rows. Counted in ROWS, not pixels: the pane's whole
/// point is to consume the table's height in whole row steps (see [`ReportPanel::comment_pane`]).
const COMMENT_PANE_MAX_ROWS: f32 = 6.0;

/// Resolve the comment shown for the current row selection.
///
/// The row is the last one clicked (`ReportSelection::current`, backed by `last_clicked` rather
/// than the Shift `anchor`), and only while it is still selected: after a Ctrl-click removes it,
/// showing its comment would describe a row that is no longer highlighted anywhere.
///
/// Args:
///     data: Current report snapshot with stable row identities.
///     cols: Runtime report schema in source order.
///     selection: Controlled multi-selection.
///
/// Returns:
///     The comment text, or `None` when no row is current, the schema has no comment column, or
///     the value is empty. An all-whitespace comment counts as empty.
pub(super) fn current_comment(
    data: &ReportData,
    cols: &[String],
    selection: &ReportSelection,
) -> Option<String> {
    let current = selection.current()?;
    let column = cols.iter().position(|name| name == "comment")?;
    let row_index = data.row_keys.iter().position(|key| *key == Some(current))?;
    let value = data.rows.get(row_index)?.get(column)?;
    let text = super::export::field_text("comment", value, chrono_tz::UTC);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // GPUI breaks lines on `\n` alone. Core comments carry CRLF and lone CR too — see
    // `display_text::flatten_lines`, which exists for the same fact on single-line surfaces —
    // and an unnormalized CR would paint as a stray glyph or swallow the break entirely.
    Some(if trimmed.contains('\r') {
        trimmed.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        trimmed.to_string()
    })
}

impl ReportPanel {
    /// Build the full-width comment pane placed between the table and the totals line.
    ///
    /// The pane has no fixed height: the text is one element, GPUI splits it on hard line breaks
    /// and wraps each of those at the pane's width, so the block is exactly as tall as the comment
    /// needs at the current width. It is `flex_none`, so that height comes out of the table above
    /// rather than the pane being squeezed to nothing.
    ///
    /// The text is set tight — the caption step with its own line box — so the block stays as small
    /// as the comment allows. It does NOT align to the table's row grid: matching that grid means a
    /// line box as tall as a table row, which reads as a stretched-out block for text this size.
    /// Its own background and top rule are what separate it from the table instead, so the table's
    /// unused tail below the last row stays visibly part of the table.
    ///
    /// Height is capped at [`COMMENT_PANE_MAX_ROWS`] table rows, with anything past the cap
    /// reachable by wheel scrolling. Every realistic comment is far below that; without a cap a
    /// pathological one would grow until the table it belongs to had no rows left.
    ///
    /// Args:
    ///     p: Active Moon palette.
    ///     cx: Panel context used for text sizing.
    ///
    /// Returns:
    ///     The pane: the current row's comment, or a muted marker when that row has no comment.
    ///     With no current row the panel omits the pane entirely rather than showing a blank strip.
    pub(super) fn comment_pane(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let text = self
            .data
            .data()
            .and_then(|data| current_comment(data, &self.cols, &self.selection));
        // With no current row the caller renders no pane at all, so the only muted case left here
        // is a current row whose comment is empty.
        let (body, color) = match text {
            Some(text) => (text, p.text),
            None => (t!("report.comment.empty").to_string(), p.text_muted),
        };
        // No caption: the text gets the panel's whole width, which is the point of the pane.
        // `items_start` is explicit — `h_flex` centres its children, which would push the first
        // lines of a capped comment above scroll offset 0, out of reach.
        let row_h = design::table_row_h(cx);
        h_flex()
            .id("rep-comment")
            .w_full()
            .flex_none()
            .items_start()
            .px_2()
            .py_0p5()
            // Its own surface, not the table's: without a lift the comment reads as one more
            // (unaligned) table row, and the table's unused tail below the last row reads as part
            // of the comment. The top rule closes the table above it.
            .bg(rgba_from(p.panel_high, 0.35))
            .border_t_1()
            .border_color(rgb(p.border))
            .max_h(px(row_h * COMMENT_PANE_MAX_ROWS))
            // The element scrolls itself. `overflow_y_scrollbar()` would look nicer but wraps the
            // pane in its own `size_full()` div that copies only `size` from this style — the cap
            // and `flex_none` would stay behind on the inner node and the full-height wrapper
            // would take the table's whole height (verified in moon-ui-components
            // `scroll/scrollable.rs`). A wheel-scrollable cap is worth more than the chrome.
            .overflow_y_scroll()
            // An unbroken token longer than the pane has no wrap opportunity; clip it instead of
            // letting it paint outside the panel.
            .overflow_x_hidden()
            .child(
                // `min_w_0` lets the text shrink below its own measured width: GPUI measures text
                // unwrapped for the automatic minimum size, so without this ANY comment longer
                // than the pane widens the row instead of wrapping inside it.
                div()
                    .flex_1()
                    .min_w_0()
                    // Caption step with the font's own compact line box: the comment is a dense
                    // block under the table, not a continuation of its rows.
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(color))
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
