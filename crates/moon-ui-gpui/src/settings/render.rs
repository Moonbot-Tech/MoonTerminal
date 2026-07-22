//! Renders the Settings window: tab strip, scrollable active-tab body, Save/status footer, and
//! window header (`settings_header`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonScrollableElement,
    MoonWindowFrame, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::{SETTINGS_HEADER_H, SettingsView, Tab};
use crate::design;

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = f32::from(window.viewport_size().width);

        // ── Tab strip ───────────────────────────────────────────────────────
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
                        radius: design::R_BUTTON_BASE,
                        font_size: 10.5,
                        line_height: 13.0,
                        gap: 5.0,
                    })
                    // Keep all seven tabs within the default 860-pixel window width.
                    .width(110.0)
                    .selected(on)
                    .label(t.title())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = t;
                        cx.notify();
                    }))
                    .render(),
            );
        }

        // ── Active tab body ─────────────────────────────────────────────────
        let content = match self.active {
            Tab::Interface => self.interface_tab(cx).into_any_element(),
            Tab::General => self.general_tab(cx).into_any_element(),
            Tab::Hotkeys => self.hotkeys_tab(cx).into_any_element(),
            Tab::Lines => self.lines_tab(cx).into_any_element(),
            Tab::Badges => self.badges_tab(cx).into_any_element(),
            Tab::Connections => self.connections_tab(cx).into_any_element(),
            Tab::Storage => self.storage_tab(cx).into_any_element(),
        };
        // Make tall tabs scroll through a stateful div with a visible vertical scrollbar.
        // `Scrollable` inherits only `size`, not `flex_1`, and renders at `size_full`; on its own it
        // would consume the full column height and push out the footer. The outer `flex_1 +
        // min_h(0)` container takes the remaining height and permits shrinking, while the inner
        // scroll div fills that container.
        let body = div().flex_1().min_h(px(0.0)).w_full().child(
            div()
                .id("settings-body")
                .size_full()
                .bg(rgba_from(p.shell, 1.0))
                .child(
                    v_flex()
                        .w_full()
                        .p(design::ui_px(cx, 18.0))
                        .gap(design::ui_px(cx, 10.0))
                        .child(content),
                )
                .overflow_y_scrollbar(),
        );

        // ── Footer: Save and status ─────────────────────────────────────────
        // Resolve status keys against the current locale here so a language change cannot leave
        // text from the previous locale behind.
        let status_el = match &self.status {
            Some((msg, err)) => {
                let text = match msg {
                    super::StatusMsg::Key(k) => t!(*k).to_string(),
                    super::StatusMsg::Text(s) => s.clone(),
                };
                div()
                    .text_color(rgba_from(if *err { p.red } else { p.green }, 1.0))
                    .child(text)
            }
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
            .child(status_el)
            .child(div().flex_1())
            // Put MoonBot import in the footer's right-hand area, which the General tab does not
            // use for Copy/Paste, and expose its explanation through a tooltip.
            .when(self.active == Tab::General, |f| {
                f.child(
                    div()
                        .id("moonbot-import-tip")
                        .tooltip(|_window, cx| {
                            cx.new(|_| {
                                moon_ui::MoonTooltipView::new(t!("import.hint").to_string())
                                    .max_width(420.0)
                            })
                            .into()
                        })
                        .child(
                            MoonButton::new("moonbot-import")
                                .outline()
                                .small()
                                .label(format!("  {}  ", t!("import.button")))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.start_moonbot_import(cx)),
                                )
                                .render(),
                        ),
                )
            })
            // Offer Copy/Paste only on tabs with portable files: Interface, Lines, Badges, and
            // Hotkeys. See `share.rs`.
            .when(self.shareable(), |f| {
                // Use outlined buttons for a visible boundary. Label spaces work around the fork's
                // `MoonButton` `pad_x=0` bug, which otherwise places text against the outline.
                f.child(
                    MoonButton::new("share-copy")
                        .outline()
                        .small()
                        .label(format!("  {}  ", t!("settings.copy")))
                        .on_click(cx.listener(|this, _, _, cx| this.copy_tab(cx)))
                        .render(),
                )
                .child(
                    MoonButton::new("share-paste")
                        .outline()
                        .small()
                        .label(format!("  {}  ", t!("settings.paste")))
                        .on_click(cx.listener(|this, _, window, cx| this.paste_tab(window, cx)))
                        .render(),
                )
            });

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
            // Render the MoonBot import preview over the entire window body.
            .children(self.import_overlay(cx))
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
