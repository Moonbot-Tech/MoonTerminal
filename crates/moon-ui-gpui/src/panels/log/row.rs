//! One log row's elements: severity colors, badges, message spans, and the row's own gestures.
//!
//! Split from [`super::render`], which owns aggregation, because this half is called for every
//! visible line on every frame while that half runs once per rebuild. Severity, its colour, and the
//! selection this row participates in belong to neither: they are shared with the Report's
//! trade-log dialog and live in [`crate::panels::line_list`].

use super::render::LineView;
use super::*;
use crate::panels::line_list::{self, Cat, Sev};
use gpui::prelude::FluentBuilder;
use std::ops::Range;

/// Return the severity badge text for `Error` or `Warn`.
fn badge_tag(sev: Sev) -> Option<&'static str> {
    match sev {
        Sev::Error => Some("ERR"),
        Sev::Warn => Some("WARN"),
        _ => None,
    }
}

/// Return the severity badge in the colour the row's own text already uses for it.
fn badge(sev: Sev, p: MoonPalette) -> Option<(&'static str, u32)> {
    badge_tag(sev).map(|tag| (tag, line_list::sev_color(sev, p)))
}

/// Return the rejection or connection category badge text.
fn cat_tag(cat: Cat) -> Option<&'static str> {
    match cat {
        Cat::Reject => Some("REJ"),
        Cat::Conn => Some("NET"),
        Cat::None => None,
    }
}

/// Return the rejection or connection category badge with its color.
fn cat_badge(cat: Cat, p: MoonPalette) -> Option<(&'static str, u32)> {
    let color = match cat {
        Cat::Reject => p.orange,
        Cat::Conn => p.yellow,
        Cat::None => return None,
    };
    cat_tag(cat).map(|tag| (tag, color))
}

/// Message segment kind used for styling; a search match takes precedence over a coin.
#[derive(Clone, Copy, PartialEq)]
enum Seg {
    Plain,
    Coin,
    Match,
}

fn seg_at(idx: usize, coin: &Option<Range<usize>>, matches: &[Range<usize>]) -> Seg {
    if matches.iter().any(|r| r.contains(&idx)) {
        Seg::Match
    } else if coin.as_ref().is_some_and(|r| r.contains(&idx)) {
        Seg::Coin
    } else {
        Seg::Plain
    }
}

/// Split a message into styled spans.
///
/// Plain text uses the severity color, a clickable coin is blue, and search matches use bold
/// accent text. An empty query with no coin takes the single-span fast path.
fn message_spans(
    flat: &str,
    // Lowercase `flat`, precomputed on the row. Building it here instead cost one allocation per
    // visible row per frame for as long as a search query was active.
    lower: &str,
    base: u32,
    coin: &Option<Range<usize>>,
    query: &str,
    // Panel, coin base, and row source used by left-click filtering and right-click chart opening.
    coin_click: Option<(WeakEntity<LogPanel>, SharedString, SharedString)>,
    p: MoonPalette,
) -> Vec<AnyElement> {
    let mut matches: Vec<Range<usize>> = Vec::new();
    if !query.is_empty() {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(query) {
            let s = from + pos;
            matches.push(s..s + query.len());
            from = s + query.len();
        }
    }
    if coin.is_none() && matches.is_empty() {
        return vec![
            div()
                .flex_none()
                .text_color(rgb(base))
                .child(flat.to_string())
                .into_any_element(),
        ];
    }
    let span = |text: &str, seg: Seg| -> AnyElement {
        match seg {
            Seg::Plain => div()
                .flex_none()
                .text_color(rgb(base))
                .child(text.to_string())
                .into_any_element(),
            Seg::Match => div()
                .flex_none()
                .font_bold()
                .text_color(rgb(p.accent))
                .child(text.to_string())
                .into_any_element(),
            Seg::Coin => {
                let mut d = div()
                    .flex_none()
                    .font_bold()
                    .text_color(rgb(p.blue))
                    .child(text.to_string());
                if let Some((weak, ticker, target)) = coin_click.clone() {
                    // Left-click filters the panel to this coin base. Stop propagation so the click
                    // does not also start a row selection on the row underneath.
                    d = d.cursor_pointer().on_mouse_down(MouseButton::Left, {
                        let weak = weak.clone();
                        let ticker = ticker.clone();
                        move |ev: &MouseDownEvent, _w, app| {
                            // Shift/Ctrl belongs to the row selection; see the core-name handler.
                            if ev.modifiers.shift || ev.modifiers.secondary() {
                                return;
                            }
                            app.stop_propagation();
                            if let Some(e) = weak.upgrade() {
                                let ticker = ticker.to_string();
                                e.update(app, |t, cx| t.set_coin_filter(Some(ticker), cx));
                            }
                        }
                    });
                    // Right-click resolves the coin against the row source and opens it on Main.
                    // Stop propagation so the row-level clipboard handler does not also run.
                    d = d.on_mouse_down(MouseButton::Right, move |_ev, _w, app| {
                        if let Some(e) = weak.upgrade() {
                            let (base, target) = (ticker.to_string(), target.to_string());
                            e.update(app, |t, cx| t.open_coin_chart(base, target, cx));
                        }
                        app.stop_propagation();
                    });
                }
                d.into_any_element()
            }
        }
    };
    let mut out: Vec<AnyElement> = Vec::new();
    let mut cur: Option<Seg> = None;
    let mut buf = String::new();
    for (idx, ch) in flat.char_indices() {
        let seg = seg_at(idx, coin, &matches);
        if Some(seg) != cur {
            if let Some(prev) = cur {
                out.push(span(&buf, prev));
                buf.clear();
            }
            cur = Some(seg);
        }
        buf.push(ch);
    }
    if let Some(prev) = cur
        && !buf.is_empty()
    {
        out.push(span(&buf, prev));
    }
    out
}

/// Text a row contributes to the clipboard: time, optional source, and the flattened message.
///
/// Shared by the single-row right-click copy and the multi-row selection copy so both spell a line
/// the same way.
pub(super) fn row_copy_text(v: &LineView) -> String {
    if v.target.is_empty() {
        format!("{} {}", v.time(), v.flat)
    } else {
        format!("{} {} {}", v.time(), v.target, v.flat)
    }
}

/// Character budget of one rendered row: every visible segment plus one gap between them.
///
/// The panel multiplies the widest row by one monospace advance to size its horizontal scroll area.
/// Counting characters here — rather than measuring the string — keeps the per-frame cost at one
/// glyph measurement instead of one per row, which matters at the 5000-row view limit.
pub(super) fn row_width_chars(v: &LineView) -> usize {
    let badge_w = |tag: Option<&str>| tag.map_or(0, |tag| tag.chars().count() + 1);
    let target_w = if v.target.is_empty() {
        0
    } else {
        v.target.chars().count() + 1
    };
    v.time().chars().count()
        + 1
        + badge_w(badge_tag(v.sev))
        + badge_w(cat_tag(v.cat))
        + target_w
        + v.flat.chars().count()
}

/// What the panel knows about a row that the row cannot work out for itself.
///
/// `source_is_core` says whether the row's `target` names a configured core — true for the
/// aggregate and exchange sources, whose rows are labelled with the core that wrote them. The Local
/// source fills the same column with a Rust module path, which selects nothing and is drawn plainly.
/// The flag follows the SOURCE, so a name buffered before its core was renamed still reads as a
/// link and its click finds nothing to select.
pub(super) struct RowCtx<'a> {
    /// Index of this row in the panel's filtered list, which the selection addresses it by.
    pub(super) ix: usize,
    pub(super) selected: bool,
    pub(super) source_is_core: bool,
    /// Search query, already trimmed and lowercased, for match highlighting.
    pub(super) query: &'a str,
    pub(super) panel: &'a WeakEntity<LogPanel>,
}

/// Render one log row as time, optional severity and category badges, source, and message.
///
/// `query` is already trimmed and lowercased for match highlighting. Pressing a row selects it and
/// dragging extends the selection; right-clicking copies the selection when the row is inside one,
/// and that row alone otherwise. Clicking a coin filters to its base and right-clicking it opens the
/// resolved market on Main; clicking the source name selects that core in the source list.
pub(super) fn log_row(v: &LineView, ctx: &RowCtx, p: MoonPalette, cx: &App) -> AnyElement {
    let RowCtx {
        ix,
        selected,
        source_is_core,
        query,
        panel: weak,
    } = *ctx;
    let base = line_list::sev_color(v.sev, p);
    let mut row = h_flex()
        .w_full()
        .gap_1()
        .items_baseline()
        .text_size(crate::design::t_body(cx))
        .px_1()
        .when(selected, |row| row.bg(line_list::selected_row_bg(p)));
    row = row.child(
        div()
            .flex_none()
            .text_color(rgb(p.text_muted))
            .child(v.time().to_string()),
    );
    if let Some((tag, col)) = badge(v.sev, p) {
        row = row.child(
            div()
                .flex_none()
                .font_bold()
                .text_color(rgb(col))
                .child(tag),
        );
    }
    if let Some((tag, col)) = cat_badge(v.cat, p) {
        row = row.child(
            div()
                .flex_none()
                .font_bold()
                .text_color(rgb(col))
                .child(tag),
        );
    }
    if !v.target.is_empty() {
        // Green, and only green: the row already spends blue on the coin, amber on WARN, orange and
        // yellow on the badges, and in the dark theme `accent` IS `amber` — a source name painted
        // with it read as one more warning.
        // Bold as well as green: on a selected row the accent tint sits under it, where a regular
        // weight in this green loses too much contrast.
        let mut source = div()
            .flex_none()
            .font_bold()
            .text_color(rgb(p.green_text))
            .child(v.target.clone());
        if source_is_core {
            // Clicking the name selects that core in the panel's source list, the way clicking a
            // core cell in the Report narrows its core selector. Stop propagation so the click does
            // not also start a row selection underneath.
            let target = SharedString::from(v.target.clone());
            let weak_core = weak.clone();
            source = source.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                move |ev: &MouseDownEvent, _w, app| {
                    // A Shift or Ctrl press is a selection gesture wherever it lands, so it bubbles
                    // to the row — the same rule the Report's coin cell follows.
                    if ev.modifiers.shift || ev.modifiers.secondary() {
                        return;
                    }
                    app.stop_propagation();
                    if let Some(panel) = weak_core.upgrade() {
                        let target = target.to_string();
                        panel.update(app, |t, cx| t.select_source_by_name(&target, cx));
                    }
                },
            );
        }
        row = row.child(source);
    }
    // Left-click filters by the coin base (`USDT-SPK` becomes `SPK`); right-click resolves the
    // market from the panel source and row target before opening its chart.
    let coin_range = v.coin.as_ref().map(|(r, _)| r.clone());
    let coin_click = v.coin.as_ref().map(|(_, base)| {
        (
            weak.clone(),
            SharedString::from(base.clone()),
            SharedString::from(v.target.clone()),
        )
    });
    let weak_press = weak.clone();
    let weak_drag = weak.clone();
    let weak_copy = weak.clone();
    row.child(
        // No `flex_1`/`min_w_0` here: the message keeps its intrinsic width so the row overflows
        // its viewport instead of being clipped, which is what the panel's horizontal scroll area
        // scrolls over.
        h_flex().flex_none().children(message_spans(
            &v.flat,
            &v.lower,
            base,
            &coin_range,
            query,
            coin_click,
            p,
        )),
    )
    // Press starts a row selection; Shift extends the existing one.
    .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _w, app| {
        if let Some(panel) = weak_press.upgrade() {
            let shift = ev.modifiers.shift;
            panel.update(app, |t, cx| t.on_row_press(ix, shift, cx));
        }
    })
    // Extends the selection while the button stays down. The button check is what keeps ordinary
    // hovering free: without it every mouse move over the list would reach the panel.
    .on_mouse_move(move |ev: &MouseMoveEvent, _w, app| {
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some(panel) = weak_drag.upgrade() {
            panel.update(app, |t, cx| t.on_row_drag(ix, cx));
        }
    })
    // Right-click copies the selection when this row belongs to one, and this row alone otherwise.
    .on_mouse_down(MouseButton::Right, move |_ev, _w, app| {
        if let Some(panel) = weak_copy.upgrade() {
            panel.update(app, |t, cx| t.copy_row_or_selection(ix, cx));
        }
    })
    .into_any_element()
}
