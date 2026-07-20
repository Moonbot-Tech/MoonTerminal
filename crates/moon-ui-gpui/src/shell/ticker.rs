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
        &mut self,
        chrome_width: f32,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        // Narrowing past the threshold takes the trigger away, so the open state has to go with it:
        // returning the layers as `None` while leaving the flag set means widening the window later
        // resurrects the popup — full-window dismiss layer and all — with no user action.
        // No `notify` needed: this frame renders nothing either way, and the flag only decides
        // subsequent frames.
        if !design::ticker_visible(cx, chrome_width) {
            self.ticker_popup_open = false;
            return (None, None);
        }
        if !self.ticker_popup_open {
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

        // Anchored to the window's RIGHT edge and offset inward past everything that stands to the
        // ticker's right in the header cluster (see `terminal_chrome`): the divider, the gaps, the
        // clock, and the window controls when they are shown.
        //
        // TEMPORARY, and deliberately so: hand-summing a chain of layout terms is fragile — every
        // one of them is a place a future header tweak can silently desync the popup from its
        // trigger, and nothing here can fail at compile time. The right shape is a MoonUI
        // Root-owned anchored layer that follows the trigger, which would delete this arithmetic
        // outright. `MoonPopover` alone does not fit: it cannot preserve this popup's
        // double-click-only opening, so that migration needs a MoonUI-side API first.
        let cluster_gap = f32::from(design::ui_px(cx, 8.0));
        let controls_w = if design::show_custom_window_controls() {
            // The controls themselves, plus the cluster gap between them and the clock. That gap
            // exists only when the controls do — adding it unconditionally shifted the popup a
            // full gap left on builds that hide them.
            cluster_gap
                + f32::from(design::ui_px(
                    cx,
                    MoonWindowFrame::main("header-ticker-popup-metrics", 0.0)
                        .show_controls(true)
                        .controls_width(),
                ))
        } else {
            0.0
        };
        // The clock's rendered width floats with the selected timezone and the Font slider, so it
        // is measured rather than assumed. `vline` is a fixed `px(1.0)` and is deliberately NOT
        // font-scaled, so the divider term stays raw.
        let right = f32::from(design::ui_px(cx, design::HEADER_PAD_X))
            + controls_w
            + crate::clock::header_clock_width(&self.backend, cx)
            + cluster_gap
            + 1.0
            + cluster_gap;
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
