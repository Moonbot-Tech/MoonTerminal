//! The log-line list mechanism shared by the Log panel and the Report's trade-log dialog.
//!
//! Both surfaces show the same thing — core log lines, one per row — and therefore need the same
//! four pieces: what a line's severity is, what colour that severity paints, how a whole-row
//! selection is dragged and copied, and how rows wider than the viewport scroll sideways. This
//! module owns all four so neither surface reaches into the other's internals.
//!
//! What is NOT here: the badges, coin highlighting, and search-match spans, which only the Log
//! panel draws (see `panels/log/row.rs`), and everything about sources, filters, and files.

use gpui::*;
use moon_ui::{
    MoonPalette, MoonScrollAxis, MoonScrollableElement, MoonScrollbarVisibility,
    MoonVirtualListScrollHandle, moon_scrollbar_overlay_with_palette, rgba_from,
};
use std::ops::Range;

/// Line severity, which determines a row's base text color.
///
/// Core logs have no source level — `LogLine::core` assigns `Info` — so their `Error` and `Warn`
/// severities are inferred from the message text.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Sev {
    Error,
    Warn,
    Info,
    /// Muted `Debug` and `Trace` output.
    Dim,
    /// Noise from the GPUI fork, including bursts of `window not found` errors while windows close.
    ///
    /// These are muted so they do not obscure real errors and are excluded from errors-only mode.
    Noise,
}

/// Semantic line category, independent of severity, used for the compact category badge.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Cat {
    None,
    /// Order failure or rejection, insufficient funds, and similar messages.
    Reject,
    /// Connection loss, reconnection, or timeout.
    Conn,
}

/// Severity and category produced together from a single lowercase conversion.
#[derive(Clone, Copy)]
pub(crate) struct Class {
    pub(crate) sev: Sev,
    pub(crate) cat: Cat,
}

const ERROR_KW: [&str; 8] = [
    "error",
    "panic",
    "exception",
    "critical",
    "fail",
    "reject",
    "ошиб",
    "отклон",
];
const WARN_KW: [&str; 5] = ["warn", "timeout", "таймаут", "retry", "предупре"];
const REJECT_KW: [&str; 6] = [
    "reject",
    "отклон",
    "denied",
    "insufficient",
    "недостаточно",
    "rejected",
];
const CONN_KW: [&str; 8] = [
    "disconnect",
    "reconnect",
    "connection",
    "разрыв",
    "timeout",
    "таймаут",
    "соедин",
    "подключ",
];

/// Classifies a line from a lowercase message already prepared by the caller.
///
/// Reusing `lower` keeps query, coin, severity, and category checks to one Unicode lowercase
/// allocation per row. Explicit non-`Info` levels take precedence over message text, while
/// recognized GPUI noise overrides everything; `Info` lines infer severity from their text.
pub(crate) fn classify_lower(level: log::Level, lower: &str) -> Class {
    if lower.contains("window not found") || lower.contains("недопустимый дескриптор окна")
    {
        return Class {
            sev: Sev::Noise,
            cat: Cat::None,
        };
    }
    let cat = categorize(lower);
    let sev = match level {
        log::Level::Error => Sev::Error,
        log::Level::Warn => Sev::Warn,
        log::Level::Debug | log::Level::Trace => Sev::Dim,
        log::Level::Info => {
            if ERROR_KW.iter().any(|k| lower.contains(k)) {
                Sev::Error
            } else if WARN_KW.iter().any(|k| lower.contains(k)) {
                Sev::Warn
            } else {
                Sev::Info
            }
        }
    };
    Class { sev, cat }
}

/// Categorize already-lowercased text, preferring `Reject` over `Conn` for rejection timeouts.
fn categorize(lower: &str) -> Cat {
    if REJECT_KW.iter().any(|k| lower.contains(k)) {
        Cat::Reject
    } else if CONN_KW.iter().any(|k| lower.contains(k)) {
        Cat::Conn
    } else {
        Cat::None
    }
}

/// Return whether a severity passes errors-only mode; GPUI noise is deliberately excluded.
pub(crate) fn is_error(sev: Sev) -> bool {
    matches!(sev, Sev::Error | Sev::Warn)
}

/// Return a row's base text color for its severity.
pub(crate) fn sev_color(sev: Sev, p: MoonPalette) -> u32 {
    match sev {
        Sev::Error => p.red,
        Sev::Warn => p.amber,
        Sev::Info => p.text_soft,
        Sev::Dim | Sev::Noise => p.text_muted,
    }
}

/// Background tint of a selected row.
pub(crate) fn selected_row_bg(p: MoonPalette) -> Hsla {
    rgba_from(p.accent, 0.18)
}

/// Widest row a horizontal scroll area is sized for, in characters.
///
/// The cap is what keeps ONE outlier from setting the width of a whole list: a flattened backtrace
/// or a raw exchange response is a single row tens of thousands of characters wide, and sizing to it
/// would shrink the scrollbar thumb to nothing for every ordinary line. Past the cap a row is
/// clipped and read through its copy instead.
pub(crate) const WIDEST_CHARS_CAP: usize = 2_000;

/// Extra width beyond the measured character budget, in unscaled pixels.
///
/// The budget counts one character per gap and nothing for a row's padding, and badges render bold,
/// so it under-measures by a few characters. A virtual list clips each row and clipped text cannot
/// be scrolled to — the slack is what keeps the tail reachable.
const WIDTH_SLACK: f32 = 48.0;

/// Width to give a row viewport whose widest row is `widest_chars` characters of monospace.
///
/// One glyph advance is measured per call rather than the whole string: the rows are monospace, so
/// the product is exact for ASCII and close enough elsewhere given the slack.
pub(crate) fn content_width(widest_chars: usize, cx: &App) -> f32 {
    crate::design::mono_body_text_width(cx, "0", 400.0) * widest_chars as f32 + WIDTH_SLACK
}

/// Wrap a vertical row list in a horizontally scrolling viewport with both scrollbars.
///
/// Two gpui facts shape this, and neither is visible from the call site:
///   - a container that scrolls only x receives a PURE VERTICAL wheel as horizontal movement unless
///     `restrict_scroll_to_axis` is set (`paint_scroll_listener`), which would drag the rows
///     sideways while the list scrolled down;
///   - every child of a scroll container is prepainted under its scroll offset, so a scrollbar
///     placed inside slides away with the content. MoonUI's own `Scrollable` keeps them siblings
///     for the same reason, but it builds the scroll area itself and leaves no way to set the axis
///     restriction — which is why this is assembled by hand.
///
/// The two bars come from different places on purpose. Horizontal is MoonUI's overlay at `Always`,
/// as the Report's table draws its own: the axis restriction above leaves the bar and Shift+wheel as
/// the only ways sideways, and a bar that appears only WHILE scrolling can never start the gesture.
/// An axis with no overflow draws none at all. Vertical stays the plain scrollbar layer — the wheel
/// already reaches it, and the overlay's track paints a lasting gutter over the rows' right edge.
///
/// Args:
///     id: Element id of the scrolling box; unique per surface, and the stem of both bars' ids.
///     list: The vertical (virtualized) row list, whose own vertical scrollbar must be hidden.
///     hscroll: Sideways offset, owned by the caller so it survives rebuilds.
///     vscroll: The list's vertical handle, which the viewport's scrollbar drives.
///     content_w: Width of the widest row, from [`content_width`].
///     window: Window the bars' drag state lives in.
///     cx: Application context, which also supplies the palette both bars paint with.
///
/// Returns:
///     The viewport element.
pub(crate) fn hscroll_viewport(
    id: &'static str,
    list: impl IntoElement,
    hscroll: &ScrollHandle,
    vscroll: &MoonVirtualListScrollHandle,
    content_w: f32,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    // The overlay is built from geometry the scroll container only writes during its own prepaint,
    // so on a surface's FIRST frame there is none and it draws no bar; `Always` schedules no
    // animation of its own, so the bar would sit out until something else repainted. One extra
    // frame fixes that — and it is taken ONCE, because a viewport that is legitimately zero-wide
    // (a collapsed dock) would otherwise ask again every frame and pin the window at 60 Hz.
    let primed = window.use_keyed_state((id, 2usize), cx, |_, _| false);
    if hscroll.bounds().size.width == px(0.0) && !*primed.read(cx) {
        primed.update(cx, |primed, _| *primed = true);
        window.request_animation_frame();
    }
    div()
        .relative()
        .size_full()
        .child(
            div()
                .id(id)
                .size_full()
                .overflow_x_scroll()
                .restrict_scroll_to_axis()
                .track_scroll(hscroll)
                .child(div().min_w(px(content_w)).h_full().child(list)),
        )
        // The bar's id must differ from the scroll container's: MoonUI names the overlay root with
        // exactly what it is given, and two `Name` ids under one parent share an element state.
        .children(moon_scrollbar_overlay_with_palette(
            format!("{id}:hbar"),
            hscroll,
            MoonScrollAxis::Horizontal,
            MoonScrollbarVisibility::Always,
            p,
            window,
            cx,
        ))
        // The vertical layer sits in a box of its own because it must be a SIBLING of the scrolling
        // container, not its child — a child would slide away with the content. The id is the
        // viewport's own, numbered, so two surfaces on screen at once stay distinct.
        .child(
            div()
                .id((id, 1usize))
                .absolute()
                .inset_0()
                .child(div().size_full().vertical_scrollbar(vscroll)),
        )
}

/// Anchor-and-head selection over a list of rows addressed by index.
///
/// Log lines are plain elements rather than an editor, so MoonUI's character-level text selection
/// (which only TextView and Input carry) does not reach them. What both surfaces offer instead is a
/// WHOLE-LINE selection with the same gestures a table has: press, drag, Shift-click to extend, and
/// copy. The anchor is where the gesture started and the head is where it currently ends; either
/// order is valid, so [`Self::range`] normalizes them. `dragging` distinguishes an in-flight drag
/// from a finished selection, so mouse movement over rows costs nothing once the button is released.
///
/// Indices address the list as it is currently filtered, so every filter change clears the selection
/// through [`Self::clear`] instead of silently re-pointing it at other lines.
#[derive(Default)]
pub(crate) struct RowSelection {
    anchor: Option<usize>,
    head: Option<usize>,
    dragging: bool,
}

impl RowSelection {
    /// Starts a new selection at `ix` and begins a drag.
    pub(crate) fn press(&mut self, ix: usize) {
        self.anchor = Some(ix);
        self.head = Some(ix);
        self.dragging = true;
    }

    /// Extends an existing selection to `ix`, or starts one when there is no anchor yet.
    pub(crate) fn shift_press(&mut self, ix: usize) {
        if self.anchor.is_none() {
            self.press(ix);
            return;
        }
        self.head = Some(ix);
        self.dragging = true;
    }

    /// Selects every row of a list of `len` rows, leaving no drag in flight.
    pub(crate) fn select_all(&mut self, len: usize) {
        if len == 0 {
            self.clear();
            return;
        }
        self.press(0);
        self.shift_press(len - 1);
        self.release();
    }

    /// Moves the head to `ix` while dragging.
    ///
    /// Returns whether anything changed, so a mouse move over an unchanged row does not repaint:
    /// this runs on every `MouseMove` the list receives.
    pub(crate) fn drag_to(&mut self, ix: usize) -> bool {
        if !self.dragging || self.head == Some(ix) {
            return false;
        }
        self.head = Some(ix);
        true
    }

    /// Ends the drag, keeping the selected range.
    pub(crate) fn release(&mut self) {
        self.dragging = false;
    }

    /// Drops the selection entirely.
    pub(crate) fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.dragging = false;
    }

    /// Whether no rows are selected.
    pub(crate) fn is_empty(&self) -> bool {
        self.range().is_none()
    }

    /// Selected rows as a half-open range, ordered regardless of drag direction.
    pub(crate) fn range(&self) -> Option<Range<usize>> {
        let (anchor, head) = (self.anchor?, self.head?);
        Some(anchor.min(head)..anchor.max(head) + 1)
    }

    /// The rows a copy command should take when it acts on row `ix`: the selection when `ix`
    /// belongs to it, and that row alone otherwise.
    pub(crate) fn range_for(&self, ix: usize) -> Range<usize> {
        self.range()
            .filter(|range| range.contains(&ix))
            .unwrap_or(ix..ix + 1)
    }

    /// Drops a selection that no longer addresses existing rows.
    ///
    /// A rebuild that shrinks the list — a narrower filter, a smaller reload — would otherwise leave
    /// endpoints pointing past the end, and a later `contains` would paint an unrelated row as
    /// selected. Dropping is the honest outcome: the rows the user picked are no longer on screen.
    pub(crate) fn clamp_to(&mut self, len: usize) {
        let out_of_range = |ix: Option<usize>| ix.is_some_and(|ix| ix >= len);
        if out_of_range(self.anchor) || out_of_range(self.head) {
            self.clear();
        }
    }

    /// Whether row `ix` is selected.
    pub(crate) fn contains(&self, ix: usize) -> bool {
        self.range().is_some_and(|range| range.contains(&ix))
    }
}

/// Copies `rows` of `lines` to the clipboard as CRLF-separated text, or nothing when the range is
/// empty or out of date.
///
/// CRLF because the destination is a Windows text field as often as it is a terminal.
pub(crate) fn copy_rows<T>(
    lines: &[T],
    rows: Range<usize>,
    text_of: impl Fn(&T) -> String,
    cx: &mut App,
) {
    let text = lines
        .get(rows)
        .unwrap_or_default()
        .iter()
        .map(text_of)
        .collect::<Vec<_>>()
        .join("\r\n");
    if text.is_empty() {
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

/// What a copy-and-select keystroke asks a line list to do.
pub(crate) enum ListKey {
    /// Ctrl/Cmd+C: copy the given rows.
    Copy(Range<usize>),
    /// Ctrl/Cmd+A: the whole list is now selected.
    SelectedAll,
}

/// Applies the shared copy and select-all keys to `selection`, returning what the caller must do.
///
/// Modifier flags and the key are compared rather than a whole keystroke: an equality check against
/// a constructed `Keystroke` also compares fields the platform fills differently. Escape is NOT
/// handled here — in a panel it clears the selection, while in a dialog it belongs to the dialog.
pub(crate) fn handle_list_key(
    selection: &mut RowSelection,
    ev: &KeyDownEvent,
    len: usize,
) -> Option<ListKey> {
    let modifiers = &ev.keystroke.modifiers;
    if !modifiers.secondary() {
        return None;
    }
    match ev.keystroke.key.as_str() {
        "c" => selection.range().map(ListKey::Copy),
        "a" if len > 0 => {
            selection.select_all(len);
            Some(ListKey::SelectedAll)
        }
        _ => None,
    }
}

#[cfg(test)]
/// Tests for row-range selection gestures.
mod tests;
