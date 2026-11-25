//! Keyboard scrolling of the Screener table.
//!
//! The table runs in `controlled_row_selection` mode (#1424) so that the opened-coin mark is the
//! only tint a row ever gets. In that mode `MoonDataTable` no longer reacts to Up, Down, Home,
//! End, PageUp or PageDown: in its own mode each of them moved the table's line-number selection
//! and scrolled only to keep that line on screen, so a controlled table leaves the keys to its
//! owner. This module gives them back as pure scrolling of the table's own scroll handle — the
//! viewport moves, no row is selected, and the mark stays on the coin it names.

use gpui::{App, KeyDownEvent, Modifiers, ScrollStrategy, Window};
use moon_ui::MoonVirtualListScrollHandle;

use crate::design;

/// Slack, in rows, for a scroll offset that lands a hair off a row boundary.
///
/// Offsets are `f32` pixels and a row height is a scaled metric, so a viewport sitting exactly on
/// row 2 can read as row 1.9999. Without the slack Down from there would re-target row 2 and the
/// key would do nothing.
const ROW_EPSILON: f32 = 1e-3;

/// Where one navigation key moves the table's viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrollTarget {
    /// Put this row at the top of the viewport. The list itself clamps the offset, so a row near
    /// the end leaves the last page in view rather than empty space below it.
    TopRow(usize),
    /// Show the last row at the bottom of the viewport.
    Bottom,
}

/// The viewport a key is applied to, read off the table's scroll handle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Viewport {
    /// Distance in pixels from the top of the first row to the top of the viewport, `>= 0`.
    pub(super) scroll_top: f32,
    /// Height in pixels of the visible row area, header excluded.
    pub(super) height: f32,
    /// Height in pixels of one row.
    pub(super) row_h: f32,
    /// Rows the table lists.
    pub(super) row_count: usize,
}

/// Map one key press to the viewport position it asks for.
///
/// Up and Down move one row, PageUp and PageDown one viewport of whole rows, Home and End to the
/// first and the last row. The table's own handler moved a selected row by those steps and
/// scrolled only far enough to keep it visible; with no selected row this moves the top of the
/// viewport by them instead. A press under Ctrl, Alt or Cmd is left alone — the table's handler
/// took those too, but here they belong to whoever binds them.
/// Shift and the macOS function flag (which the platform sets on every arrow key) are ignored.
///
/// Args:
///     key: The keystroke's key name as gpui reports it.
///     modifiers: Modifiers held with it.
///     viewport: Current geometry of the table's row area.
///
/// Returns:
///     The target, or `None` when the key is not a navigation key or there is nothing to scroll.
pub(super) fn scroll_target(
    key: &str,
    modifiers: Modifiers,
    viewport: Viewport,
) -> Option<ScrollTarget> {
    if modifiers.control || modifiers.alt || modifiers.platform {
        return None;
    }
    if viewport.row_count == 0 || !(viewport.row_h > 0.0) {
        return None;
    }
    let position = viewport.scroll_top.max(0.0) / viewport.row_h;
    // The row whose top is at or just above the viewport top, and the first one at or below it.
    // They differ only while the viewport sits mid-row (a wheel scroll), and then Down goes to
    // the next boundary below and Up to the next one above, never skipping a row.
    let at_or_above = (position + ROW_EPSILON).floor() as usize;
    let at_or_below = (position - ROW_EPSILON).ceil().max(0.0) as usize;
    let page = ((viewport.height / viewport.row_h + ROW_EPSILON).floor() as usize).max(1);
    let last = viewport.row_count - 1;
    let target = match key {
        "down" => ScrollTarget::TopRow(at_or_above.saturating_add(1).min(last)),
        "up" => ScrollTarget::TopRow(at_or_below.saturating_sub(1).min(last)),
        "pagedown" | "page_down" => {
            ScrollTarget::TopRow(at_or_above.saturating_add(page).min(last))
        }
        "pageup" | "page_up" => ScrollTarget::TopRow(at_or_below.saturating_sub(page).min(last)),
        "home" => ScrollTarget::TopRow(0),
        "end" => ScrollTarget::Bottom,
        _ => return None,
    };
    Some(target)
}

/// Read the viewport geometry off the table's scroll handle.
///
/// The laid-out offset is current whenever a key arrives: the view notifies itself after a handled
/// key, and gpui's `Window::dispatch_key_event` draws a dirty window before it dispatches the next
/// key, so the list has already applied the previous press's target.
///
/// Args:
///     handle: The handle the table tracks.
///     row_h: Rendered row height in pixels.
///     row_count: Rows the table lists.
///
/// Returns:
///     The viewport as last laid out.
fn viewport(handle: &MoonVirtualListScrollHandle, row_h: f32, row_count: usize) -> Viewport {
    let state = handle.0.borrow();
    Viewport {
        scroll_top: -f32::from(state.base_handle.offset().y),
        height: f32::from(state.base_handle.bounds().size.height),
        row_h,
        row_count,
    }
}

/// Ask the table's list to move its viewport on the next layout.
///
/// Args:
///     handle: The handle the table tracks.
///     target: Where the key asked to go.
fn apply(handle: &MoonVirtualListScrollHandle, target: ScrollTarget) {
    match target {
        ScrollTarget::TopRow(row) => handle.scroll_to_item_strict(row, ScrollStrategy::Top),
        ScrollTarget::Bottom => handle.scroll_to_bottom(),
    }
}

/// Scroll the table for one key press that bubbled up from it, selecting nothing.
///
/// Called from a key listener on an element that wraps the table alone, so it only ever sees keys
/// pressed while the table has focus. A handled key stops there, as the table's own handler
/// stopped it in uncontrolled mode.
///
/// Args:
///     handle: The handle the table tracks.
///     row_count: Rows the table lists.
///     event: The key press.
///     window: Window the table lives in.
///     cx: Application context, for the row height.
///
/// Returns:
///     `true` when the key scrolled the table; the caller then notifies its view so the next
///     frame lays the list out at the new offset.
pub(super) fn scroll_by_key(
    handle: &MoonVirtualListScrollHandle,
    row_count: usize,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let viewport = viewport(handle, design::table_row_h(cx), row_count);
    let Some(target) = scroll_target(
        event.keystroke.key.as_str(),
        event.keystroke.modifiers,
        viewport,
    ) else {
        return false;
    };
    apply(handle, target);
    window.prevent_default();
    cx.stop_propagation();
    true
}

#[cfg(test)]
mod tests;
