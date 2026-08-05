//! Figure-drawing tool cluster in the tab strip: a pencil button that toggles drawing mode, tools
//! shown while that mode is enabled, an Alert button for the selected figure, and a style popup
//! opened by right-clicking the pencil. The popup controls the tool, the line's colour, thickness,
//! opacity and kind (Solid, Dash, Dot, DashDot, DashDotDot), and the FILL — its colour and its own
//! opacity, where "no fill" is simply zero opacity. A tool that colours itself from a typed scale
//! keeps the fill's on/off and its opacity, but not the colour: its levels bring their own.
//! Extracted from `strip.rs`.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuSize, MoonPalette, h_flex,
    v_flex,
};

use moon_core::figures::{DEFAULT_FILL_ALPHA, DrawStyle, FigureTool, LineKind};
use rust_i18n::t;

use super::ChartTabs;
use crate::design;

/// The swatch palette and the opacity arithmetic live with the per-figure settings panel: the two
/// surfaces must offer the same colours and move by the same step, or "the same" style set here and
/// there would not be the same style.
use crate::figstyle::{SWATCHES, opacity_step};

/// One tool button, generated from the registry row: a new tool appears in the strip and in the
/// style popup by existing, with its element id and glyph coming from its own module.
fn tool_button(
    def: &'static moon_core::figures::ToolDef,
    selected: bool,
    backend: gpui::Entity<crate::Backend>,
) -> MoonButton {
    let tool = def.tool;
    MoonButton::new(ElementId::Name(SharedString::new_static(def.key)))
        .label(def.glyph)
        .tooltip(t!(def.locale_key).to_string())
        .size(MoonButtonSize::Micro)
        .variant(if selected {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Ghost
        })
        .selected(selected)
        .on_click(move |_, _w, app| {
            backend.update(app, |b, bcx| {
                b.fig_tool = tool;
                bcx.notify();
            });
        })
}

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
            for def in moon_core::figures::tools::REGISTRY {
                row = row.child(tool_button(def, tool == def.tool, self.backend.clone()).render());
            }
            row
        });

        h_flex()
            .items_center()
            .gap(px(2.0))
            .child(pencil)
            .children(tool_btns)
    }

    /// Render the pencil style popup opened by right-click: tool, line colour, thickness, opacity
    /// and kind, then the fill's colour and opacity.
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
        for def in moon_core::figures::tools::REGISTRY {
            tool_row =
                tool_row.child(tool_button(def, tool == def.tool, self.backend.clone()).render());
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

        // Fill swatches, mirroring the line's: the same palette, but writing `fill` and keeping
        // its own opacity. The leftmost cell is "no fill" — one control for the switch and the
        // colour, because a fill at zero opacity IS no fill.
        let mut fill_row = h_flex().items_center().gap(px(3.0)).flex_wrap();
        let backend_off = self.backend.clone();
        fill_row = fill_row.child(
            div()
                .id("fig-fill-off")
                .w(px(16.0))
                .h(px(16.0))
                .rounded(design::ui_px(cx, 3.0))
                .border_2()
                .border_color(if style.has_fill() {
                    rgb(p.border)
                } else {
                    rgb(p.accent)
                })
                .text_color(rgb(p.text_muted))
                .text_center()
                .child("∅")
                .cursor_pointer()
                .tooltip(|_window, cx| {
                    cx.new(|_| moon_ui::MoonTooltipView::new(t!("chart.fig.no_fill").to_string()))
                        .into()
                })
                .on_click(move |_, _w, app| {
                    backend_off.update(app, |b, bcx| {
                        b.fig_style.fill[3] = 0;
                        bcx.notify();
                    });
                }),
        );
        // A tool that colours itself from a ratio scale gets no swatches — they would change
        // nothing — but the row still has to be a two-state switch, so it gets an ON cell painted
        // in one of the scale's own hues beside the "∅". Otherwise a fill turned off could only be
        // brought back from the opacity stepper below, which is not where anyone looks for it.
        if tool.def().level_palette {
            let backend_on = self.backend.clone();
            let [sw_r, sw_g, sw_b] = moon_core::figures::levels::scale_swatch();
            fill_row = fill_row.child(
                div()
                    .id("fig-fill-scale")
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .bg(gpui::rgb(
                        ((sw_r as u32) << 16) | ((sw_g as u32) << 8) | sw_b as u32,
                    ))
                    .border_2()
                    .border_color(if style.has_fill() {
                        rgb(p.accent)
                    } else {
                        rgb(p.border)
                    })
                    .cursor_pointer()
                    .tooltip(|_window, cx| {
                        cx.new(|_| {
                            moon_ui::MoonTooltipView::new(t!("chart.fig.scale_fill").to_string())
                        })
                        .into()
                    })
                    .on_click(move |_, _w, app| {
                        backend_on.update(app, |b, bcx| {
                            if b.fig_style.fill[3] == 0 {
                                b.fig_style.fill[3] = DEFAULT_FILL_ALPHA;
                            }
                            bcx.notify();
                        });
                    }),
            );
        }
        let fill_swatches: &[[u8; 4]] = if tool.def().level_palette {
            &[]
        } else {
            &SWATCHES
        };
        for (i, sw) in fill_swatches.iter().enumerate() {
            let backend = self.backend.clone();
            let sw = *sw;
            let selected = style.has_fill() && style.fill[..3] == sw[..3];
            fill_row = fill_row.child(
                div()
                    .id(("fig-fill-swatch", i))
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
                            // Picking a colour turns the fill on: at its current strength, or at
                            // the default when it was off, since the ∅ cell zeroes the alpha. A
                            // fill that stayed invisible after a deliberate click would read as
                            // broken.
                            let a = match b.fig_style.fill[3] {
                                0 => DEFAULT_FILL_ALPHA,
                                a => a,
                            };
                            b.fig_style.fill = [sw[0], sw[1], sw[2], a];
                            bcx.notify();
                        });
                    }),
            );
        }

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

        let fill_pct = (style.fill[3] as f32 / 255.0 * 100.0).round() as i32;
        let fill_op_row = h_flex()
            .items_center()
            .gap(px(4.0))
            .child(label(&t!("chart.fig.fill_opacity")))
            .child(self.step_btn("fig-fop-dn", "−", cx, |s| {
                // Whole percentage points, like the line's opacity: raw-alpha steps make values
                // such as 15% unreachable, which is the defect `opacity_step` exists to avoid.
                s.fill[3] = if s.fill[3] == 0 {
                    0
                } else {
                    opacity_step(s.fill[3], false)
                }
            }))
            .child(
                div()
                    .w(design::font_w_px(cx, 34.0))
                    .text_center()
                    .text_color(rgb(p.text))
                    .child(format!("{fill_pct}%")),
            )
            .child(self.step_btn("fig-fop-up", "+", cx, |s| {
                // Stepping up from "no fill" turns it on at the default strength rather than at
                // the 5% floor, which would look like nothing happened.
                s.fill[3] = if s.fill[3] == 0 {
                    DEFAULT_FILL_ALPHA
                } else {
                    opacity_step(s.fill[3], true)
                }
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
                    .label(style.kind.label())
                    .trigger_caret(true)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Micro)
                    .trigger_width_scaled(120.0)
                    .menu_width_scaled(130.0)
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
            // A line has no area, so the fill controls would change nothing while one is chosen.
            .children(tool.def().fills.then_some(label(&t!("chart.fig.fill"))))
            .children(tool.def().fills.then_some(fill_row))
            .children(tool.def().fills.then_some(fill_op_row))
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
