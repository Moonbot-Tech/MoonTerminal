//! Structured station readings and the bounded action log, with independent scrolling chrome.

use gpui::*;
use moon_ui::{
    MoonPalette, MoonScrollAxis, MoonScrollbarVisibility, MoonTooltipView, h_flex,
    moon_scrollbar_overlay_with_palette, rgba_from, v_flex,
};

use crate::design;

/// Owned display data lets MoonUI construct the scrollbar with the render window and app.
#[derive(IntoElement)]
pub(super) struct StationProgress {
    pub(super) status: Option<moon_tg::StatusFacts>,
    pub(super) lines: Vec<String>,
    pub(super) scroll: ScrollHandle,
}

impl RenderOnce for StationProgress {
    /// Render uncapped facts and a capped log with one clipped row per entry and full-text tooltips.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let mut content = v_flex()
            .w_full()
            .min_w(px(0.0))
            .gap(design::ui_px(cx, 12.0));
        if let Some(facts) = self.status {
            let mut status = v_flex()
                .w_full()
                .min_w(px(0.0))
                .flex_shrink_0()
                .font_family(design::ui_font())
                .text_color(rgba_from(p.text, 1.0))
                .gap(design::ui_px(cx, 12.0))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(facts.title));
            for (section_index, section) in facts.sections.into_iter().enumerate() {
                let mut rows = v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(section.title));
                for (row_index, (label, value)) in section.rows.into_iter().enumerate() {
                    // Proportional columns fit a narrow dock. Ellipsis bounds unbroken paths;
                    // each cell exposes its complete text through MoonUI's tooltip.
                    let cell = |id: String, text: String, mono: bool| {
                        let full = text.clone();
                        div()
                            .id(id)
                            .min_w(px(0.0))
                            .truncate()
                            .font_family(if mono {
                                design::mono()
                            } else {
                                design::ui_font()
                            })
                            .text_color(rgba_from(if mono { p.text } else { p.text_muted }, 1.0))
                            .tooltip(move |_, cx| {
                                cx.new(|_| MoonTooltipView::new(full.clone())).into()
                            })
                            .child(text)
                    };
                    rows = rows.child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .gap(design::ui_px(cx, 8.0))
                            .child(
                                cell(
                                    format!("station-label-{section_index}-{row_index}"),
                                    label,
                                    false,
                                )
                                .w(relative(0.4))
                                .flex_shrink_0(),
                            )
                            .child(
                                cell(
                                    format!("station-value-{section_index}-{row_index}"),
                                    value,
                                    true,
                                )
                                .flex_1(),
                            ),
                    );
                }
                status = status.child(rows);
            }
            for note in facts.notes {
                status = status.child(div().text_color(rgba_from(p.text_muted, 1.0)).child(note));
            }
            content = content.child(status);
        }
        if !self.lines.is_empty() {
            // The overlay reads the previous frame's scroll extent. Repaint after new content
            // is laid out so even a single completed batch exposes its overflow immediately.
            let seen = window.use_keyed_state("station-progress-line-count", cx, |_, _| 0usize);
            if *seen.read(cx) != self.lines.len() {
                seen.update(cx, |count, _| *count = self.lines.len());
                window.request_animation_frame();
            }
            let mut lines = v_flex()
                .id("server-bot-lines")
                .w_full()
                .max_h(design::ui_px(cx, 260.0))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .font_family(design::mono())
                .text_color(rgba_from(p.text, 1.0))
                .p(design::ui_px(cx, 10.0))
                .pr(design::ui_px(cx, 10.0 + design::MOON_SCROLLBAR_OVERLAY_W));
            for (index, line) in self.lines.into_iter().enumerate() {
                let full = line.clone();
                lines = lines.child(
                    div()
                        .id(("station-progress-line", index))
                        .w_full()
                        .min_w(px(0.0))
                        .flex_shrink_0()
                        .truncate()
                        .tooltip(move |_, cx| cx.new(|_| MoonTooltipView::new(full.clone())).into())
                        .child(line),
                );
            }
            content = content.child(
                div()
                    .relative()
                    .w_full()
                    .border_1()
                    .border_color(rgba_from(p.border, 1.0))
                    .rounded(design::r_container(cx))
                    .overflow_hidden()
                    .bg(rgba_from(p.table_body, 1.0))
                    .child(lines)
                    .children(moon_scrollbar_overlay_with_palette(
                        "server-bot-lines-scrollbar",
                        &self.scroll,
                        MoonScrollAxis::Vertical,
                        MoonScrollbarVisibility::Always,
                        p,
                        window,
                        cx,
                    )),
            );
        }
        content
    }
}
