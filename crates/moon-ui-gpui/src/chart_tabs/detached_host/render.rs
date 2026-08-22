//! `Render` implementation for `DetachedChartHost`: a window header with market search, scale, the
//! candlestick and ⚙ layout popups, and "close all charts" above the chart-stack panel. The ⚙ popup
//! overlay and market-search plumbing are shared with the tab strip through
//! [`super::super::common`], and the header's group/divider structure mirrors that strip's.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonInput, MoonPalette,
    MoonWindowFrame, MoonWindowFrameControls, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::candle_popup;
use super::super::common;
use super::super::graphics_popup;
use super::super::labels_popup;
use super::super::{chart_pane_label, coin_search};
use super::DetachedChartHost;
use crate::design;

impl Render for DetachedChartHost {
    /// Renders the detached chart window with the dock toolbar's grouping and popup behavior.
    ///
    /// The market dropdown and dismiss layer use the measured header height so scaling cannot move
    /// either layer across the search field.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Correct a restored window's size once it is on the target display with the correct scale:
        // force the saved logical size to override `WM_DPICHANGED` shrinkage.
        if let Some(sz) = self.restore_size.take() {
            window.resize(sz);
        }
        // Shift+middle-click on THIS window's chart applies X scale to its panel and persists the spec.
        {
            let (rev, req) = {
                let b = self.backend.read(cx);
                (b.chart_x_sync_rev, b.chart_x_sync)
            };
            if rev != self.last_x_sync_rev {
                self.last_x_sync_rev = rev;
                if let Some((handle, ppm)) = req {
                    if handle == window.window_handle() {
                        self.panel
                            .update(cx, |s, c| s.set_x_ppm(Some(ppm), true, c));
                        let backend = self.backend.clone();
                        let (num, bucket) = (self.num, self.bucket.clone());
                        common::upsert_spec(
                            &backend,
                            &self.group.clone(),
                            num,
                            &bucket,
                            cx,
                            move |s| s.x_ppm = Some(ppm),
                        );
                    }
                }
            }
        }
        let p = MoonPalette::active(cx);
        // Scale belongs to this panel per tab and is edited directly on it.
        let scale = self.panel.read(cx).scale();
        let panel = self.panel.clone();
        let close_all_panel = self.panel.clone();
        let title = chart_pane_label(&self.backend, &self.group, self.num, &self.bucket, cx);
        let frame = MoonWindowFrame::detached_chart("detached-chart-window-frame", 0.0)
            .header_height(34.0)
            .controls(MoonWindowFrameControls::Close)
            .show_controls(design::show_custom_window_controls());
        let popup_open = self.layout_popup_open;
        let candle_popup_open = self.candle_popup_open;
        let graphics_popup_open = self.graphics_popup_open;
        let labels_popup_open = self.labels_popup_open;
        // Header market-search input and matches. Render the list at the `v_flex` level after the
        // body; otherwise the later-painted window body covers the header dropdown.
        let coin_search_el = div()
            .w(design::font_w_px(cx, 80.0))
            // Reopen on a click into an already-focused field; see the tab strip's twin.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, _window, cx| {
                    this.open_coin_popup(cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                MoonInput::new("detached-coin-search")
                    .state(&self.coin_input)
                    .cleanable(true)
                    .small(),
            );
        // Both layers hang off the header's ACTUAL height rather than the 34 it is built from: that
        // height is scaled, so raw constants drift under a non-default UI or font scale and leave
        // the popup overlapping the header or the dismiss layer covering the input.
        let header_h = design::fit_h_px(cx, 34.0, 13.0, 10.5);
        let coin_popup = self.coin_popup_open.then(|| {
            let results = self.coin_results(cx);
            coin_search::render_popup(
                "detached-coin",
                results,
                &std::collections::HashSet::new(),
                false,
                None,
                p,
                cx,
                common::coin_pick_handler(cx, self.coin_input.clone()),
                |_core, _market, _app| {},
                |_app| {},
            )
            .absolute()
            .right(design::ui_px(cx, 6.0))
            .top(header_h + design::ui_px(cx, 4.0))
        });
        // Catch outside clicks only below the header so the search field remains interactive.
        let coin_dismiss = self.coin_popup_open.then(|| {
            div()
                .id("detached-coin-dismiss")
                .absolute()
                .top(header_h)
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .on_mouse_down(MouseButton::Left, common::coin_dismiss_handler(cx))
        });
        // A modifier held for a MOUSE gesture is a prefix too, and releasing it must not fire a
        // lone-modifier binding. Window-level and in the capture phase: the chart consumes its own
        // presses, so a bubble listener on this root would never see them.
        {
            let view = cx.entity();
            window.on_mouse_event::<MouseDownEvent>(move |_e, phase, _window, cx| {
                if phase == DispatchPhase::Capture {
                    view.update(cx, |this, _| this.modifier_watch.interrupt());
                }
            });
        }
        // Only detached tab windows have this header; the main dock does not. Scale is on the left,
        // and "close all charts" is on the right.
        v_flex()
            .size_full()
            .relative()
            // Focusable root with window-hotkey handling through the shared dispatcher.
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_hotkey(ev, window, cx)),
            )
            // Caps Lock and a lone modifier are bindable too, and neither arrives as a key press.
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, window, cx| {
                this.on_modifier_hotkey(ev, window, cx)
            }))
            // Capture phase: a key a focused field consumes never bubbles to the listener above,
            // and a modifier held while it was typed must lose its claim to being a binding.
            .capture_key_down(cx.listener(|this, _: &KeyDownEvent, _window, _cx| {
                this.modifier_watch.interrupt();
            }))
            .child(
                h_flex()
                    .h(header_h)
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, design::CHROME_GAP))
                    .pl(design::ui_px(cx, design::titlebar_leading_inset()))
                    .pr(design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.shell_high))
                    .child(
                        frame
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    .child(design::chrome_divider(cx, p))
                    .child(design::chrome_section(cx).child(coin_search_el))
                    .child(design::chrome_divider(cx, p))
                    .child(
                        design::chrome_section(cx)
                            .child(crate::controls::scale_dropdown_for_add_stack(
                                cx,
                                scale,
                                panel.clone(),
                                p,
                            ))
                            // Both buttons ARE their popovers' triggers, so each popup opens under its
                            // own button on MoonUI's Root layer.
                            .child(candle_popup::candle_popup_host(
                                self,
                                "detached-chart-candles",
                                // Mirrors the dock strip's candle button; see `chart_tabs::strip`.
                                MoonButton::new("detached-candle-settings")
                                    .leading_icon(MoonButtonIconSlot::new(
                                        "icons/chart-candlestick.svg",
                                    ))
                                    .tooltip(t!("chart.candles.tip").to_string())
                                    .size(MoonButtonSize::Micro)
                                    .variant(if candle_popup_open {
                                        MoonButtonVariant::Blue
                                    } else {
                                        MoonButtonVariant::Ghost
                                    })
                                    .selected(candle_popup_open)
                                    .render(),
                                cx,
                            ))
                            // The palette button edits THIS window's chart-drawing settings, the
                            // way the candle button beside it edits this window's candles.
                            .child(graphics_popup::graphics_popup_host(
                                self,
                                "detached-chart-graphics",
                                MoonButton::new("detached-graphics-settings")
                                    .leading_icon(MoonButtonIconSlot::new("icons/palette.svg"))
                                    .tooltip(t!("chart.graphics.tip").to_string())
                                    .size(MoonButtonSize::Micro)
                                    .variant(if graphics_popup_open {
                                        MoonButtonVariant::Blue
                                    } else {
                                        MoonButtonVariant::Ghost
                                    })
                                    .selected(graphics_popup_open)
                                    .render(),
                                cx,
                            ))
                            // The labels button edits THIS window's chart captions.
                            .child(labels_popup::labels_popup_host(
                                self,
                                "detached-chart-labels",
                                MoonButton::new("detached-labels-settings")
                                    .leading_icon(MoonButtonIconSlot::new(
                                        "icons/a-large-small.svg",
                                    ))
                                    .tooltip(t!("chart_labels.tip").to_string())
                                    .size(MoonButtonSize::Micro)
                                    .variant(if labels_popup_open {
                                        MoonButtonVariant::Blue
                                    } else {
                                        MoonButtonVariant::Ghost
                                    })
                                    .selected(labels_popup_open)
                                    .render(),
                                cx,
                            ))
                            .child(common::layout_popup_host(
                                self,
                                "detached-chart-layout",
                                crate::panels::popup_gear_trigger(
                                    "detached-layout-settings",
                                    t!("chart.layout.tip").to_string(),
                                    popup_open,
                                ),
                                t!("chart.apply_all_tabs_windows").to_string(),
                                cx,
                            )),
                    )
                    .child(design::chrome_divider(cx, p))
                    .child(
                        design::chrome_section(cx).child(
                            // The one button in this row that keeps a glyph: MoonUI ships no bin
                            // icon (its `delete.svg` is a backspace key), and an X would read as
                            // "close the window" beside the real window controls. So it is squared
                            // the way the column selectors are — a rendered width equal to the
                            // size's own drawn height — instead of by the icon-only path.
                            MoonButton::new("detached-close-all")
                                .label("🗑")
                                .width(design::micro_control_h_value(cx))
                                .tooltip(t!("chartwin.clear").to_string())
                                .size(MoonButtonSize::Micro)
                                .variant(MoonButtonVariant::Ghost)
                                .on_click(move |_, _w, app| {
                                    close_all_panel.update(app, |p, cx| p.close_all_panes(cx));
                                })
                                .render(),
                        ),
                    )
                    .when(design::show_custom_window_controls(), |this| {
                        this.child(frame.visual_controls(cx))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    // Deliberately no `.bg()`: the chart own-pass and text layer are under-scene, so
                    // any opaque body background would cover the chart. The dark window clear from
                    // the MoonUI fork fills beneath and between charts without a white background.
                    .child(self.panel.clone()),
            )
            .children(coin_dismiss)
            .children(coin_popup)
    }
}
