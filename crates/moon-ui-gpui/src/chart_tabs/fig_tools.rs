//! Кластер инструментов рисования фигур в полоске вкладок: кнопка-карандаш
//! (тумблер режима рисования), инструменты (когда режим включён), кнопка «Alert»
//! выделенной фигуры и попап стиля (ПКМ по карандашу): инструмент, цвет, толщина,
//! непрозрачность, Solid/Dash. Вынесено из `strip.rs`.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuSize, MoonPalette, h_flex,
    v_flex,
};

use moon_core::figures::{DrawStyle, FigureTool, LineKind};
use rust_i18n::t;

use super::ChartTabs;
use crate::design;

/// Палитра свотчей попапа (голубой дефолт + практичный набор). RGBA (a=255).
const SWATCHES: [[u8; 4]; 8] = [
    [64, 196, 255, 255],  // голубой (дефолт)
    [80, 220, 120, 255],  // зелёный
    [240, 90, 90, 255],   // красный
    [250, 200, 60, 255],  // жёлтый
    [245, 150, 40, 255],  // оранжевый
    [200, 110, 240, 255], // фиолетовый
    [240, 240, 240, 255], // белый
    [150, 160, 175, 255], // серый
];

/// Шаг непрозрачности ±5% с посадкой на круглые проценты. Прежняя арифметика
/// «±24 из 255» давала прыжки 100→91→81 и недостижимые значения (15% не
/// выставлялось никак) — теперь любое кратное 5 в диапазоне 5..=100 достижимо.
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

        // Кнопка-карандаш: ЛКМ — тумблер режима рисования, ПКМ — попап стиля.
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

        // Инструменты (только в режиме рисования): подсвечен активный.
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

    /// Попап стиля карандаша (ПКМ по карандашу): инструмент, цвет, толщина, непрозрачность, Kind.
    fn render_style_popup(
        &self,
        tool: FigureTool,
        style: DrawStyle,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = |s: &str| div().text_color(rgb(p.text_muted)).child(s.to_string());

        // Ряд инструментов.
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

        // Свотчи цвета.
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
                            // Непрозрачность сохраняем из текущего стиля.
                            let a = b.fig_style.color[3];
                            b.fig_style.color = [sw[0], sw[1], sw[2], a];
                            bcx.notify();
                        });
                    }),
            );
        }

        // Степперы толщины/непрозрачности + Kind Solid/Dash.
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

        // Вид линии (Kind): выпадашка из 5 значений (Solid/Dash/Dot/DashDot/DashDotDot).
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
                    .trigger_width(120.0)
                    .menu_width(130.0)
                    .menu_size(MoonMenuSize::Compact)
                    .items(kind_items),
            );

        v_flex()
            .absolute()
            .top_full()
            .left_0()
            .mt(px(4.0))
            // Ширина попапа растёт с кеглем — иначе на +6 подписи/значения
            // переносились на вторую строку.
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

    /// Кнопка-степпер, правящая `fig_style` замыканием `edit`.
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
