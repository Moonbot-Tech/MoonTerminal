//! Figure-drawing tool cluster in the tab strip: a pencil button that toggles drawing mode, tools
//! shown while that mode is enabled, an Alert button for the selected figure, and a style popup
//! opened by right-clicking the pencil. The popup controls tool, color, thickness, opacity, and
//! line kind: Solid, Dash, Dot, DashDot, or DashDotDot. Extracted from `strip.rs`.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuSize, MoonPalette, h_flex,
    v_flex,
};

use moon_core::figures::{DrawStyle, FigureTool, LineKind};
use rust_i18n::t;

use super::ChartTabs;
use crate::design;

/// Popup swatch palette with the blue default and a practical set, stored as RGBA with `a=255`.
const SWATCHES: [[u8; 4]; 8] = [
    [64, 196, 255, 255],  // blue (default)
    [80, 220, 120, 255],  // green
    [240, 90, 90, 255],   // red
    [250, 200, 60, 255],  // yellow
    [245, 150, 40, 255],  // orange
    [200, 110, 240, 255], // purple
    [240, 240, 240, 255], // white
    [150, 160, 175, 255], // gray
];

/// Step opacity by 5%, snapping to whole percentage points.
///
/// The previous +/-24-of-255 arithmetic jumped from 100 to 91 to 81 and made values such as 15%
/// unreachable. Every multiple of five in `5..=100` is now reachable.
fn opacity_step(a: u8, up: bool) -> u8 {
    let pct = (a as f32 / 255.0 * 100.0).round() as i32;
    let next = if up {
        pct / 5 * 5 + 5
    } else if pct % 5 == 0 {
        pct - 5
    } else {
        pct / 5 * 5
    }
    .clamp(5, 100);
    (next as f32 / 100.0 * 255.0).round() as u8
}

const TOOLS: [(FigureTool, &str, &str); 4] = [
    (FigureTool::HLine, "fig-tool-hline", "─"),
    (FigureTool::Segment, "fig-tool-segment", "╱"),
    (FigureTool::Triangle, "fig-tool-triangle", "△"),
    (FigureTool::Channel, "fig-tool-channel", "☰"),
];

impl ChartTabs {
    pub(super) fn render_fig_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let (draw_mode, tool, style) = {
            let b = self.backend.read(cx);
            (b.fig_draw_mode, b.fig_tool, b.fig_style)
        };
        let view = cx.entity();

        // Pencil button: left-click toggles drawing mode; right-click opens the style popup.
        let backend = self.backend.clone();
        let view_popup = view.clone();
        let pencil = div()
            .relative()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _w, cx| {
                    this.fig_style_popup_open = !this.fig_style_popup_open;
                    cx.notify();
                }),
            )
            .child(
                MoonButton::new("fig-pencil")
                    .label("✏")
                    .size(MoonButtonSize::Micro)
                    .variant(if draw_mode {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(draw_mode)
                    .tooltip(t!("chart.fig.pencil_tip").to_string())
                    .on_click(move |_, _w, app| {
                        backend.update(app, |b, bcx| {
                            b.fig_draw_mode = !b.fig_draw_mode;
                            bcx.notify();
                        });
                    })
                    .render(),
            )
            .children(
                self.fig_style_popup_open
                    .then(|| self.render_style_popup(tool, style, p, cx)),
            );
        let _ = view_popup;

        // Tools shown only in drawing mode, with the active tool highlighted.
        let tool_btns = draw_mode.then(|| {
            let mut row = h_flex().items_center().gap(px(2.0));
            for (t, id, label) in TOOLS {
                let backend = self.backend.clone();
                let on = tool == t;
                row = row.child(
                    MoonButton::new(id)
                        .label(label)
                        .size(MoonButtonSize::Micro)
                        .variant(if on {
                            MoonButtonVariant::Blue
                        } else {
                            MoonButtonVariant::Ghost
                        })
                        .selected(on)
                        .on_click(move |_, _w, app| {
                            backend.update(app, |b, bcx| {
                                b.fig_tool = t;
                                bcx.notify();
                            });
                        })
                        .render(),
                );
            }
            row
        });

        h_flex()
            .items_center()
            .gap(px(2.0))
            .child(pencil)
            .children(tool_btns)
    }

    /// Render the pencil style popup opened by right-click: tool, color, thickness, opacity, and kind.
    fn render_style_popup(
        &self,
        tool: FigureTool,
        style: DrawStyle,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = |s: &str| div().text_color(rgb(p.text_muted)).child(s.to_string());

        // Tool row.
        let mut tool_row = h_flex().items_center().gap(px(3.0));
        for (t, id, glyph) in TOOLS {
            let backend = self.backend.clone();
            let on = tool == t;
            tool_row = tool_row.child(
                MoonButton::new(id)
                    .label(glyph)
                    .size(MoonButtonSize::Micro)
                    .variant(if on {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(on)
                    .on_click(move |_, _w, app| {
                        backend.update(app, |b, bcx| {
                            b.fig_tool = t;
                            bcx.notify();
                        });
                    })
                    .render(),
            );
        }

        // Color swatches followed by the arbitrary Custom color picker.
        let mut color_row = h_flex().items_center().gap(px(3.0)).flex_wrap();
        for (i, sw) in SWATCHES.iter().enumerate() {
            let backend = self.backend.clone();
            let sw = *sw;
            let selected = style.color[..3] == sw[..3];
            color_row = color_row.child(
                div()
                    .id(("fig-swatch", i))
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .bg(gpui::rgb(
                        ((sw[0] as u32) << 16) | ((sw[1] as u32) << 8) | sw[2] as u32,
                    ))
                    .border_2()
                    .border_color(if selected {
                        rgb(p.accent)
                    } else {
                        rgb(p.border)
                    })
                    .cursor_pointer()
                    .on_click(move |_, _w, app| {
                        backend.update(app, |b, bcx| {
                            // Preserve opacity from the current style.
                            let a = b.fig_style.color[3];
                            b.fig_style.color = [sw[0], sw[1], sw[2], a];
                            bcx.notify();
                        });
                    }),
            );
        }
        // Choose any palette color, not only a fixed swatch, through MoonUI's `MoonColorPicker`.
        // The subscription in `ChartTabs::new` writes the selection to `fig_style`.
        color_row = color_row.child(
            div()
                .id("fig-custom-color")
                .tooltip(|_window, cx| {
                    cx.new(|_| {
                        moon_ui::MoonTooltipView::new(t!("chart.fig.custom_color").to_string())
                    })
                    .into()
                })
                .child(
                    moon_ui::MoonColorPicker::new(&self.fig_color_picker)
                        .colors(design::picker_palette()),
                ),
        );

        // Thickness and opacity steppers plus solid or dashed kind.
        let thickness_row = h_flex()
            .items_center()
            .gap(px(4.0))
            .child(label(&t!("chart.fig.thickness")))
            .child(self.step_btn("fig-th-dn", "−", cx, |s| {
                s.thickness = (s.thickness - 0.5).max(0.5)
            }))
            .child(
                div()
                    .w(design::font_w_px(cx, 28.0))
                    .text_center()
                    .text_color(rgb(p.text))
                    .child(format!("{:.1}", style.thickness)),
            )
            .child(self.step_btn("fig-th-up", "+", cx, |s| {
                s.thickness = (s.thickness + 0.5).min(6.0)
            }));

        let opacity_pct = (style.color[3] as f32 / 255.0 * 100.0).round() as i32;
        let opacity_row = h_flex()
            .items_center()
            .gap(px(4.0))
            .child(label(&t!("chart.fig.opacity")))
            .child(self.step_btn("fig-op-dn", "−", cx, |s| {
                s.color[3] = opacity_step(s.color[3], false)
            }))
            .child(
                div()
                    .w(design::font_w_px(cx, 34.0))
                    .text_center()
                    .text_color(rgb(p.text))
                    .child(format!("{opacity_pct}%")),
            )
            .child(self.step_btn("fig-op-up", "+", cx, |s| {
                s.color[3] = opacity_step(s.color[3], true)
            }));

        // Line kind dropdown over every `LineKind::ALL` value: Solid, Dash, Dot, DashDot, DashDotDot.
        let backend_kind = self.backend.clone();
        let kind_items = crate::panels::radio_items(
            LineKind::ALL.iter().map(|k| {
                (
                    *k,
                    SharedString::from(k.label()),
                    SharedString::from(k.label()),
                )
            }),
            style.kind,
            crate::panels::RadioMark::Check,
            move |app, k: LineKind| {
                backend_kind.update(app, |b, bcx| {
                    b.fig_style.kind = k;
                    bcx.notify();
                });
            },
        );
        let kind_row = h_flex()
            .items_center()
            .gap(px(4.0))
            .child(label(&t!("chart.fig.kind")))
            .child(
                MoonDropdown::new("fig-kind")
                    .label(format!("{} ▾", style.kind.label()))
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Micro)
                    .trigger_width(design::font_w(cx, 120.0))
                    .menu_width(design::font_w(cx, 130.0))
                    .menu_size(MoonMenuSize::Compact)
                    .items(kind_items),
            );

        v_flex()
            .absolute()
            .top_full()
            .left_0()
            .mt(px(4.0))
            // Grow popup width with font size; otherwise labels and values wrapped at +6.
            .w(design::font_w_px(cx, 210.0))
            .p(px(8.0))
            .gap(px(6.0))
            .bg(rgb(p.surface))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(design::r_container(cx))
            .shadow_lg()
            .text_size(design::t_body(cx))
            .child(tool_row)
            .child(color_row)
            .child(thickness_row)
            .child(opacity_row)
            .child(kind_row)
    }

    /// Build a stepper button that edits `fig_style` through the `edit` closure.
    fn step_btn(
        &self,
        id: &'static str,
        label: &'static str,
        _cx: &mut Context<Self>,
        edit: fn(&mut DrawStyle),
    ) -> impl IntoElement {
        let backend = self.backend.clone();
        MoonButton::new(id)
            .label(label)
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .on_click(move |_, _w, app| {
                backend.update(app, |b, bcx| {
                    edit(&mut b.fig_style);
                    bcx.notify();
                });
            })
            .render()
    }
}
