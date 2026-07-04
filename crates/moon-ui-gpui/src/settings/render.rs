//! Рендер окна настроек: полоска вкладок, тело активной вкладки (скролл), футер
//! «Сохранить» + статус и шапка окна (`settings_header`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonWindowFrame, h_flex,
    rgba_from, v_flex,
};
use rust_i18n::t;

use super::{SETTINGS_HEADER_H, SettingsView, Tab};
use crate::design;

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = f32::from(window.viewport_size().width);

        // ── Полоска вкладок ─────────────────────────────────────────────────
        let mut tabs = h_flex()
            .w_full()
            .h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
            .gap(design::ui_px(cx, 6.0))
            .px(design::ui_px(cx, 8.0))
            .bg(rgba_from(p.shell_high, 1.0))
            .border_b_1()
            .border_color(rgba_from(p.border, 1.0));
        for t in Tab::ALL {
            let on = self.active == t;
            tabs = tabs.child(
                MoonButton::new(t.id())
                    .variant(if on {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .size(MoonButtonSize::Custom {
                        height: 24.0,
                        radius: 4.0,
                        font_size: 10.5,
                        line_height: 13.0,
                        gap: 5.0,
                    })
                    .width(118.0)
                    .selected(on)
                    .label(t.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = t;
                        cx.notify();
                    }))
                    .render(),
            );
        }

        // ── Тело активной вкладки ───────────────────────────────────────────
        let content = match self.active {
            Tab::Interface => self.interface_tab(cx).into_any_element(),
            Tab::General => self.general_tab(cx).into_any_element(),
            Tab::Hotkeys => self.hotkeys_tab(cx).into_any_element(),
            Tab::Lines => self.lines_tab(cx).into_any_element(),
            Tab::Connections => self.connections_tab(cx).into_any_element(),
        };
        // Тело прокручивается (вкладки выше высоты окна): stateful div + overflow_y_scroll.
        let body = div()
            .id("settings-body")
            .flex_1()
            .w_full()
            .overflow_y_scroll()
            .bg(rgba_from(p.shell, 1.0))
            .child(
                v_flex()
                    .w_full()
                    .p(design::ui_px(cx, 18.0))
                    .gap(design::ui_px(cx, 10.0))
                    .child(content),
            );

        // ── Подвал: Сохранить + статус ──────────────────────────────────────
        let status_el = match &self.status {
            Some((msg, err)) => div()
                .text_color(rgba_from(if *err { p.red } else { p.green }, 1.0))
                .child(msg.clone()),
            None => div(),
        };
        let footer = h_flex()
            .w_full()
            .h(design::fit_h_px(cx, 42.0, 14.0, 14.0))
            .gap(design::ui_px(cx, 10.0))
            .px(design::ui_px(cx, 10.0))
            .items_center()
            .bg(rgba_from(p.shell_high, 1.0))
            .border_t_1()
            .border_color(rgba_from(p.border, 1.0))
            .child(
                MoonButton::new("save")
                    .primary()
                    .small()
                    .width(110.0)
                    .label(t!("settings.save").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    .render(),
            )
            .child(status_el);

        v_flex()
            .size_full()
            .relative()
            .bg(rgba_from(p.shell, 1.0))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .text_color(rgba_from(p.text, 1.0))
            .child(settings_header(p, cx))
            .child(tabs)
            .child(body)
            .child(footer)
            .child(
                MoonWindowFrame::tool("settings-window-frame-hit", chrome_width)
                    .header_height(SETTINGS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

fn settings_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("settings-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, SETTINGS_HEADER_H, 14.0, 8.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgba_from(p.shell_high, 1.0))
        .border_b(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("settings-titlebar-title", 0.0)
                .title_cluster(t!("settings.title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("settings-window-frame-visual", 0.0)
                    .header_height(SETTINGS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}
