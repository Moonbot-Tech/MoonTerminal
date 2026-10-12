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
}
/// Reuse the retained sample and fit the phone to its Settings column.
pub(super) fn phone(content: PreviewContent, cache: PreviewCache) -> impl IntoElement {
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
    Phone { content: sample }
}

impl RenderOnce for Phone {
    /// Draw native text and table bands in the active theme's colours.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let body = match self.content.as_ref() {
            PreviewContent::Card(lines) => v_flex()
                .w_full()
                .min_w_0()
                .gap(design::ui_px(cx, 2.0))
                .children(lines.iter().map(|line| text_runs(&line.spans, window, &p)))
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
                v_flex()
                    .w_full()
                    .max_w(design::ui_px(cx, 280.0))
                    .min_w_0()
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
                    .max_w(design::ui_px(cx, 280.0))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("telegram.layout.preview_hint").to_string()),
            )
    }
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
