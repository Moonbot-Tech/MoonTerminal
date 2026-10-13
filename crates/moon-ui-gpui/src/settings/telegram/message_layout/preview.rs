//! Telegram dark-chat imitation drawn from the production renderer's typed spans and rows.
use crate::design;
use gpui::*;
use moon_tg::{PreviewLine, PreviewRowKind, PreviewTable, Span as PreviewSpan, Tone};
use moon_ui::{MoonPalette, h_flex, rgba_from, v_flex};
use rust_i18n::t;
use std::{cell::RefCell, rc::Rc};

/// Typed content is retained only when its semantic values changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PreviewContent {
    Card(Vec<PreviewLine>),
    Report(PreviewTable),
}
/// Keep the last typed sample separate for each bot side, without an HTML parser or dependency.
#[derive(Clone, Default)]
pub(super) struct PreviewCache(Rc<RefCell<Option<Rc<PreviewContent>>>>);
/// Native phone body defers text-run styling until its window's font context is available.
#[derive(IntoElement)]
struct Phone {
    content: Rc<PreviewContent>,
    card_w: f32,
}
/// Reuse the retained sample and fit the phone to its Settings card `card_w` wide.
pub(super) fn phone(content: PreviewContent, cache: PreviewCache, card_w: f32) -> impl IntoElement {
    let mut cached = cache.0.borrow_mut();
    let sample = if let Some(previous) = cached
        .as_ref()
        .filter(|previous| previous.as_ref() == &content)
    {
        previous.clone()
    } else {
        let sample = Rc::new(content);
        *cached = Some(sample.clone());
        sample
    };
    Phone {
        content: sample,
        card_w,
    }
}

impl RenderOnce for Phone {
    /// Draw native text and table bands in the active theme's colours.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let bubble = bubble_w(self.card_w, cx);
        let text_w = (bubble - 2.0 * design::ui_value(cx, TEXT_INSET)).max(0.0);
        let body = match self.content.as_ref() {
            PreviewContent::Card(lines) => v_flex()
                .w(px(text_w))
                .gap(design::ui_px(cx, 2.0))
                .children(
                    lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| card_line(i, &line.spans, text_w, window, cx)),
                )
                .into_any_element(),
            PreviewContent::Report(table) => report_table(table, window, cx),
        };
        v_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("telegram.layout.preview").to_string()),
            )
            .child(
                // The width is computed, never content-sized: a shrink-wrapped bubble measured
                // near zero in the real window and stacked its text a few glyphs per line.
                v_flex()
                    .debug_selector(|| "tg-preview-bubble".into())
                    .w(px(bubble))
                    .flex_none()
                    .rounded(design::ui_px(cx, 14.0))
                    .p(design::ui_px(cx, 6.0))
                    .bg(rgba_from(p.shell_high, 1.0))
                    .child(
                        v_flex()
                            .w_full()
                            .min_w_0()
                            .rounded(design::ui_px(cx, 16.0))
                            .p(design::ui_px(cx, 10.0))
                            .gap(design::ui_px(cx, 8.0))
                            .bg(rgba_from(p.surface, 1.0))
                            .text_color(rgba_from(p.text, 1.0))
                            .text_size(design::t_body(cx))
                            .child(
                                div()
                                    .text_center()
                                    .text_color(rgba_from(p.text_muted, 1.0))
                                    .child(local_clock().format("%d.%m.%Y").to_string()),
                            )
                            .child(
                                v_flex()
                                    .w_full()
                                    .min_w_0()
                                    .rounded(design::ui_px(cx, 10.0))
                                    .p(design::ui_px(cx, 10.0))
                                    .bg(rgba_from(p.panel, 1.0))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgba_from(p.accent, 1.0))
                                            .child("Moon Station"),
                                    )
                                    .child(body)
                                    .child(
                                        div()
                                            .text_right()
                                            .text_color(rgba_from(p.text_muted, 1.0))
                                            .child(local_clock().format("%H:%M").to_string()),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .w(px(bubble))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("telegram.layout.preview_hint").to_string()),
            )
    }
}

/// Widest message bubble, unscaled: a Telegram message on a phone-sized chat.
const BUBBLE_MAX_W: f32 = 420.0;

/// Padding the Settings group box takes off each side of its card, unscaled: 1 px border and
/// 14 px padding (`message_layout_box`).
const CARD_INSET: f32 = 15.0;

/// Padding between the bubble's edge and its text on each side, unscaled: the bubble's 6 px, the
/// chat's 10 px and the message's 10 px.
const TEXT_INSET: f32 = 26.0;

/// The bubble's explicit width in a card `card_w` wide: `min(card - padding, 420)`, scaled.
///
/// Args:
///     card_w: The Settings card's outer width.
///     cx: Application context for scale.
pub(super) fn bubble_w(card_w: f32, cx: &App) -> f32 {
    (card_w - 2.0 * design::ui_value(cx, CARD_INSET))
        .min(design::ui_value(cx, BUBBLE_MAX_W))
        .max(0.0)
}

/// Gap between two words of a card line, unscaled: one space at the body size.
const WORD_GAP: f32 = 4.0;

/// The line's spans cut into words that never break: a cut falls only at a space outside
/// brackets, so a parenthesised figure such as `(+4.96%)` stays whole on one line. Spaces are
/// dropped; the caller draws them as the gap between words. A word keeps each span's styling.
///
/// Args:
///     spans: The line's spans in reading order.
///
/// Returns:
///     The words, each a run of styled spans.
fn unbreakable_words(spans: &[PreviewSpan]) -> Vec<Vec<PreviewSpan>> {
    let mut words: Vec<Vec<PreviewSpan>> = Vec::new();
    let mut word: Vec<PreviewSpan> = Vec::new();
    let mut depth = 0_usize;
    for span in spans {
        let mut piece = String::new();
        for ch in span.text.chars() {
            match ch {
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.saturating_sub(1),
                _ => {}
            }
            if ch == ' ' && depth == 0 {
                if !piece.is_empty() {
                    word.push(PreviewSpan {
                        text: std::mem::take(&mut piece),
                        ..span.clone()
                    });
                }
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                piece.push(ch);
            }
        }
        if !piece.is_empty() {
            word.push(PreviewSpan {
                text: piece,
                ..span.clone()
            });
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// One card line: its unbreakable words wrapping between each other, never inside one.
///
/// Args:
///     index: The line's position, for its `tg-preview-line-{index}` selector.
///     spans: The line's spans.
///     line_w: The line's explicit width: the bubble's less its padding.
///     window: Window for the text style.
///     cx: Application context for theme and scale.
fn card_line(
    index: usize,
    spans: &[PreviewSpan],
    line_w: f32,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .debug_selector(move || format!("tg-preview-line-{index}"))
        .w(px(line_w))
        .flex_wrap()
        .items_start()
        .gap_x(design::ui_px(cx, WORD_GAP))
        .children(unbreakable_words(spans).into_iter().map(|word| {
            // Each word gets its measured width, never a content-sized one; a word wider than
            // the line (a long link) is capped to it and wraps inside rather than painting past.
            div()
                .flex_none()
                .w(px(word_w(&word, cx).min(line_w)))
                .child(text_runs(&word, window, &p))
        }))
        .into_any_element()
}

/// Measured width of a word's styled spans at the body size, with a pixel of slack so the
/// rounding of glyph advances never wraps a word that fits.
fn word_w(word: &[PreviewSpan], cx: &App) -> f32 {
    let text: f32 = word
        .iter()
        .map(|span| {
            let weight = if span.bold {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            };
            design::ui_body_text_width(cx, &span.text, weight.0)
        })
        .sum();
    text + 1.0
}

/// Convert explicit raw spans to GPUI runs; no string is treated as markup.
fn text_runs(spans: &[PreviewSpan], window: &Window, p: &MoonPalette) -> AnyElement {
    let mut text = String::new();
    let mut runs = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let mut style = window.text_style();
        style.color = rgba_from(
            match span.tone {
                Tone::Plain => p.text,
                Tone::Link => p.accent,
                Tone::Gain => p.green_text,
                Tone::Loss => p.red_text,
            },
            1.0,
        );
        style.font_weight = if span.bold {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        };
        style.font_style = if span.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        runs.push(style.to_run(span.text.len()));
        text.push_str(&span.text);
    }
    div()
        .w_full()
        .min_w_0()
        .child(StyledText::new(text).with_runs(runs))
        .into_any_element()
}

/// All rows share the same width fractions, so four columns wrap instead of scrolling.
fn table_cell(index: usize, count: usize) -> Div {
    let width = if index == 0 {
        0.40
    } else {
        0.60 / count.saturating_sub(1).max(1) as f32
    };
    div().w(relative(width)).min_w_0().px_1().py_1()
}

/// Draw typed bands, alignment and spacers directly, preserving the real row/column walk.
fn report_table(table: &PreviewTable, window: &Window, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let count = table.header.len();
    let mut body = v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 4.0))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(table.title.clone()),
        )
        .child(
            div()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(table.caption.clone()),
        )
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .border_b_1()
                .border_color(rgba_from(p.border, 1.0))
                .children(table.header.iter().enumerate().map(|(index, label)| {
                    let cell = table_cell(index, count)
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(label.clone());
                    if index == 0 { cell } else { cell.text_right() }
                })),
        );
    for row in &table.rows {
        if row.kind == PreviewRowKind::Spacer {
            body = body.child(div().h(design::ui_px(cx, 6.0)));
            continue;
        }
        let mut cells = h_flex().w_full().min_w_0();
        if row.band {
            cells = cells.bg(rgba_from(
                if row.kind == PreviewRowKind::Total {
                    p.table_selected
                } else {
                    p.row_alt
                },
                1.0,
            ));
        }
        for (index, span) in row.cells.iter().enumerate() {
            let cell =
                table_cell(index, count).child(text_runs(std::slice::from_ref(span), window, &p));
            cells = cells.child(if index > 0 || row.first_right {
                cell.text_right()
            } else {
                cell
            });
        }
        body = body.child(cells);
    }
    body.into_any_element()
}

/// Use the shared safe system offset rather than Chrono's fallible Windows Local path.
fn local_clock() -> chrono::DateTime<chrono::Utc> {
    let millis = moon_core::util::time::now_unix_ms_i64()
        .saturating_add(moon_core::util::time::local_utc_offset_ms());
    chrono::DateTime::from_timestamp_millis(millis).unwrap_or(chrono::DateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests;
