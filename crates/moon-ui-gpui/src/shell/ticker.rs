//! Header ticker-source popup hosted beside the right-aligned clock.
//!
//! The popup reuses [`crate::chart_tabs::coin_search`] and persists the selected market and stable
//! core ID through `Backend::set_header_ticker`. Its overlay and dismiss layers follow the other
//! header popups.

use gpui::*;

use moon_ui::{MoonInput, MoonPalette, MoonWindowFrame, v_flex};
use rust_i18n::t;

use crate::chart_tabs::coin_search;
use crate::design;

use super::Shell;

impl Shell {
    /// Toggle the ticker-source popup and clear its search query when opening it.
    pub(crate) fn toggle_ticker_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ticker_popup_open {
            self.ticker_popup_open = false;
        } else {
            self.ticker_popup_open = true;
            self.ticker_popup_hovered = false;
            self.ticker_input
                .update(cx, |st, c| st.set_value(String::new(), window, c));
        }
        cx.notify();
    }

    pub(super) fn close_ticker_popup(&mut self, cx: &mut Context<Self>) {
        if self.ticker_popup_open {
            self.ticker_popup_open = false;
            cx.notify();
        }
    }

    /// Build the right-anchored ticker popup and its full-window dismiss layer.
    ///
    /// Both elements are `None` when the popup is closed or `chrome_width` hides the ticker trigger.
    pub(super) fn ticker_popup_layers(
        &self,
        chrome_width: f32,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        if !self.ticker_popup_open || !design::ticker_visible(cx, chrome_width) {
            return (None, None);
        }
        let query = self.ticker_input.read(cx).value().to_string();
        let results = {
            let b = self.backend.read(cx);
            coin_search::search(&b, &self.group, None, &query)
        };
        let backend = self.backend.clone();
        let view = cx.entity();
        let list = coin_search::render_popup(
            "header-ticker-search",
            results,
            &Default::default(),
            false,
            p,
            cx,
            move |core, market, _window, app| {
                backend.update(app, |b, bcx| {
                    b.set_header_ticker(core, market);
                    bcx.notify();
                });
                view.update(app, |this, cx| this.close_ticker_popup(cx));
            },
            |_, _, _| {},
            |_| {},
        );

        // Anchored to the window's RIGHT edge, offset by the window controls: the ticker is the
        // last header element before them. Anchoring right rather than computing a left offset
        // keeps the popup under its trigger no matter how wide the clock renders — that width
        // floats with the selected timezone and its label.
        let controls_w = if design::show_custom_window_controls() {
            f32::from(design::ui_px(
                cx,
                MoonWindowFrame::main("header-ticker-popup-metrics", 0.0)
                    .show_controls(true)
                    .controls_width(),
            ))
        } else {
            0.0
        };
        // Plus the cluster gap that sits between the ticker and those controls.
        let right = f32::from(design::ui_px(cx, design::HEADER_PAD_X))
            + controls_w
            + f32::from(design::ui_px(cx, 8.0));
        let top = f32::from(design::fit_h_px(cx, design::HEADER_TOP_H, 14.0, 9.0));
        let overlay = div()
            .id("header-ticker-popup-box")
            .absolute()
            .right(px(right))
            .top(px(top))
            .on_mouse_down(MouseButton::Left, |_, _w, app| app.stop_propagation())
            .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                if *hovered {
                    this.ticker_popup_hovered = true;
                } else if this.ticker_popup_hovered && !cx.has_active_drag() {
                    this.close_ticker_popup(cx);
                }
            }))
            .child(
                // Ширина = список `render_popup` (фикс. 240) + свои паддинги (2×6, масштабируются
                // слайдером шрифта) + рамка (2×1): иначе внутренняя рамка списка вылазит за
                // границы попапа.
                v_flex()
                    .w(px(240.0 + 2.0 * f32::from(design::ui_px(cx, 6.0)) + 2.0))
                    .gap(design::ui_px(cx, 4.0))
                    .p(design::ui_px(cx, 6.0))
                    .bg(rgb(p.panel_high))
                    .border_1()
                    .border_color(rgb(p.border))
                    .rounded(design::r_button(cx))
                    .child(
                        div()
                            .text_size(design::t_caption(cx))
                            .text_color(rgb(p.text_muted))
                            .child(t!("header.ticker_pick").to_string()),
                    )
                    .child(
                        MoonInput::new("header-ticker-query")
                            .state(&self.ticker_input)
                            .small(),
                    )
                    .child(list),
            )
            .into_any_element();
        let dismiss = div()
            .id("header-ticker-popup-dismiss")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, _w, cx| {
                    this.close_ticker_popup(cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element();
        (Some(overlay), Some(dismiss))
    }
}
