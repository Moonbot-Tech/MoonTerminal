//! Chart-tab strip rendering for `ChartTabs`: the Main/AddToChart strip, gather-windows and layout
//! controls, and the active panel below. Tab and synchronization logic lives in [`super`], while
//! detached windows live in [`super::windows`].

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonInput, MoonPalette,
    MoonRect, MoonTabItem, MoonTabStrip, h_flex, v_flex,
};
use rust_i18n::t;

use super::candle_popup;
use super::common;
use super::graphics_popup;
use super::labels_popup;
use super::{ChartTabs, Tab, chart_tab_strip_h, coin_search};
use crate::design;

impl Render for ChartTabs {
    /// Renders the chart tabs and the grouped toolbar shared by the docked chart surface.
    ///
    /// Popup hosts remain outside the strip's clipping layer so search results and anchored
    /// settings are not cut off when the tab row overflows.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Snapshot tabs so callbacks do not retain a borrow of `self.add`: identity, label, width
        // count, unread badge count, and detachability.
        let mut tabs: Vec<(Tab, String, usize, usize, bool)> =
            vec![(Tab::Main, "Main".to_string(), 0, 0, false)];
        tabs.extend(self.add.iter().map(|(n, bucket, panel)| {
            let count = panel.read(cx).pane_count(cx);
            let seen = self.seen.get(&(*n, bucket.clone())).copied().unwrap_or(0);
            (
                Tab::Add(*n, bucket.clone()),
                self.add_label(*n, bucket, cx),
                count,
                count.saturating_sub(seen),
                true,
            )
        }));
        // Custom multi-market tabs use their own names, have no badge, and are closable. The shared
        // click handler below detaches them on double-click just like Add tabs.
        tabs.extend(self.custom.iter().map(|(n, bucket, _)| {
            (
                Tab::Custom(*n, bucket.clone()),
                self.custom_label(*n),
                0,
                0,
                true,
            )
        }));
        let tab_keys = Rc::new(
            tabs.iter()
                .map(|(tab, _, _, _, _)| tab.clone())
                .collect::<Vec<_>>(),
        );
        let items = tabs
            .iter()
            .map(|(tab, label, _count, unread, detachable)| {
                let width = design::tab_width(cx, label.chars().count(), *unread > 0, *detachable);
                let mut item = MoonTabItem::new(label.clone())
                    .width(width)
                    .selected(self.active == *tab)
                    .closable(*detachable);
                if *unread > 0 {
                    item = item.badge(unread.to_string());
                }
                item
            })
            .collect::<Vec<_>>();
        let view = cx.entity();
        // `MoonTabStrip` absolutely positions every tab and clips to its own bounds. Explicit
        // bounds prevent its root from collapsing to zero size and leaving the flexible chart to
        // consume the full height. The lower container clips window width to the panel width.
        let strip_w = f32::from(window.viewport_size().width).max(1.0);
        // Match the strip height to `fit_height` so UI or font scaling keeps its underline aligned.
        let strip_h = chart_tab_strip_h(cx);
        let strip = MoonTabStrip::new("chart-tabs-strip")
            .padding_left(8.0)
            .gap(4.0)
            .bounds(MoonRect::new(0.0, 0.0, strip_w, strip_h))
            .items(items)
            .on_click({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, event, window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    // Read before entering the view update: the detached window must open on THIS
                    // window's display, and `detach` can no longer ask the owner for it from inside
                    // the owner's own update. Gated on the detaching gesture because the read walks
                    // every monitor, and a plain tab switch is the common case.
                    let detaching = matches!(tab_id, Tab::Add(..) | Tab::Custom(..))
                        && event.click_count() >= 2;
                    let owner_display = detaching
                        .then(|| crate::window::windowing::window_display_id(window, app))
                        .flatten();
                    view.update(app, |this, cx| {
                        // Double-click detaches Add and Custom tabs into OS windows, but never Main.
                        if detaching {
                            this.detach(tab_id, owner_display, cx);
                            return;
                        }
                        let exists = matches!(tab_id, Tab::Main)
                            || this
                                .add
                                .iter()
                                .any(|(n, c, _)| Tab::Add(*n, c.clone()) == tab_id)
                            || this
                                .custom
                                .iter()
                                .any(|(n, c, _)| Tab::Custom(*n, c.clone()) == tab_id);
                        if exists && this.active != tab_id {
                            this.active = tab_id;
                            this.sync_seen_for_active(cx);
                            this.sync_active_scale(cx);
                            this.sync_inactive_chart_visibility(cx);
                            this.refresh_orderbook_gates(cx);
                            // A locked comparison tab uses its anchor as the trading target, like Main fullscreen.
                            this.sync_main_chart_target(cx);
                            this.persist_scales(cx);
                            cx.notify();
                        }
                    });
                }
            })
            .on_close({
                let tab_keys = tab_keys.clone();
                let view = view.clone();
                move |ix, _event, _window, app| {
                    let Some(tab_id) = tab_keys.get(ix).cloned() else {
                        return;
                    };
                    if matches!(tab_id, Tab::Main) {
                        return;
                    }
                    view.update(app, |this, cx| {
                        this.add
                            .retain(|(n, c, _)| Tab::Add(*n, c.clone()) != tab_id);
                        // Fully close a custom tab by removing its stack, label, and persisted spec.
                        if let Tab::Custom(n, _) = &tab_id {
                            let n = *n;
                            this.custom
                                .retain(|(num, c, _)| Tab::Custom(*num, c.clone()) != tab_id);
                            this.custom_labels.remove(&n);
                            this.remove_custom_spec(n, cx);
                        }
                        if this.active == tab_id {
                            this.active = Tab::Main;
                        }
                        this.sync_seen_for_active(cx);
                        this.sync_active_scale(cx);
                        this.sync_inactive_chart_visibility(cx);
                        this.refresh_orderbook_gates(cx);
                        this.sync_main_chart_target(cx);
                        this.persist_scales(cx);
                        cx.notify();
                    });
                }
            });

        // Show Gather Windows on the strip's right only when this group has detached windows. It
        // activates every window; on Windows it also restores and cascades them onto the primary display.
        let detached_count = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == self.group)
            .count();
        let gather_btn = (detached_count > 0).then(|| {
            let entity = cx.entity();
            MoonButton::new("chart-gather-windows")
                // Two stacked windows, which is literally what the action does. Deliberately the
                // one FILLED icon in the strip: MoonUI's is a solid path rather than a Lucide
                // outline, and `render_alpha_mask` keeps only the raster's alpha, so its own black
                // fill is discarded and the silhouette takes the button's variant colour. It reads
                // heavier than its neighbours by choice, not by mistake.
                .leading_icon(MoonButtonIconSlot::new("icons/window-restore.svg"))
                // The glyph it replaces carried the button's whole meaning, so dropping it without
                // a tooltip would leave the action unnamed.
                .tooltip(t!("chart.gather_windows.tip").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .on_click(move |_, _w, app| {
                    entity.update(app, |this, cx| this.gather_windows(cx));
                })
                .render()
        });

        // The active tab's layout control and adjacent scale dropdown are both per-tab.
        let popup_open = self.layout_popup_open;
        let p_strip = MoonPalette::active(cx);
        let scale_dropdown = crate::controls::scale_dropdown_for_tabs(
            cx,
            self.active_scale_value(cx),
            cx.entity(),
            p_strip,
        );
        let apply_all_label = if matches!(self.active, Tab::Main) {
            t!("chart.layout.apply_all_windows").to_string()
        } else {
            t!("chart.layout.apply_all_charts").to_string()
        };
        // Every button in this cluster is icon-only, and that is load-bearing rather than a style
        // preference: MoonUI's `Button` takes its SQUARE path (`size_5`, one value for width and
        // height) only when there is no label and no child. Give one a text label and it switches
        // to `h_5().px_1()`, where the width follows that glyph's advance — which is how the row
        // came to hold three buttons of three different widths. A new button here needs an icon
        // from `moon-ui-components-assets/assets/icons`, not a glyph.
        //
        // The gear IS the popover trigger, so the popup opens under its own button and rides
        // MoonUI's Root layer instead of an in-scene overlay the strip's clipping could cut.
        let settings_btn = common::layout_popup_host(
            self,
            "chart-layout",
            crate::panels::popup_gear_trigger(
                "chart-layout-settings",
                t!("chart.layout.tip").to_string(),
                popup_open,
            ),
            apply_all_label,
            cx,
        );
        // The candle/trade display control beside layout edits the global setting set.
        let candle_popup_open = self.candle_popup_open;
        let candle_btn = candle_popup::candle_popup_host(
            self,
            "chart-candles",
            // An icon, not a glyph: the former "❚" label drew as a thin vertical bar that read as a
            // separator between toolbar groups rather than as a button. No explicit icon colour —
            // the icon inherits the button's variant foreground, so the selected (Blue) state stays
            // visible.
            MoonButton::new("chart-candle-settings")
                .leading_icon(MoonButtonIconSlot::new("icons/chart-candlestick.svg"))
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
        );
        // The palette button beside the candles one edits the ACTIVE TAB's chart-drawing settings.
        let graphics_popup_open = self.graphics_popup_open;
        let graphics_btn = graphics_popup::graphics_popup_host(
            self,
            "chart-graphics",
            MoonButton::new("chart-graphics-settings")
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
        );
        // The labels button beside the palette one edits the ACTIVE TAB's chart captions.
        let labels_popup_open = self.labels_popup_open;
        let labels_btn = labels_popup::labels_popup_host(
            self,
            "chart-labels",
            MoonButton::new("chart-labels-settings")
                .leading_icon(MoonButtonIconSlot::new("icons/a-large-small.svg"))
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
        );
        // The per-window market search sits left of scale and queries the active tab's cores.
        // Absolutely position matches below its wrapper and place the cluster outside the strip's
        // clipping layer so `overflow_hidden` cannot cut off the dropdown.
        let coin_popup = self.coin_popup_open.then(|| {
            let server_context = {
                let b = self.backend.read(cx);
                let auto_core = super::auto_workspace_chart_core(b, &self.group);
                let bucket = super::coin_search_bucket(&self.active, auto_core);
                auto_core
                    .and_then(|_| {
                        crate::controls::coin_search::single_server_context(
                            b,
                            &self.group,
                            bucket.as_ref(),
                        )
                    })
                    .map(|name| crate::display_text::flatten_lines(&name))
            };
            let results = self.coin_results(cx);
            let view_toggle = cx.entity();
            let view_open = cx.entity();
            coin_search::render_popup(
                "tabs-coin",
                results,
                &self.coin_selected,
                true,
                server_context,
                p_strip,
                cx,
                common::coin_pick_handler(cx, self.coin_input.clone()),
                move |core, market, app| {
                    view_toggle.update(app, |this, cx| this.toggle_coin_selected(core, market, cx));
                },
                move |app| {
                    view_open.update(app, |this, cx| this.open_selected_in_new_tab(cx));
                },
            )
            .absolute()
            .top_full()
            .right_0()
            .mt(px(2.0))
        });
        let coin_search_el = div()
            .relative()
            .child(
                div()
                    .w(design::font_w_px(cx, 80.0))
                    // `Focus` fires only on GAINING focus, so clicking a field that already has it
                    // emits nothing and would leave a dismissed list closed. This reopens it, and
                    // stops the event before the dismiss layer underneath closes it again.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev, _window, cx| {
                            this.open_coin_popup(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        MoonInput::new("tabs-coin-search")
                            .state(&self.coin_input)
                            .cleanable(true)
                            .small(),
                    ),
            )
            .children(coin_popup);
        // The tool-settings panel closes the same way the market list does: a layer below the
        // cluster catches every click that missed it. Without one the panel stays parked over the
        // chart until its own button is pressed again.
        let fig_dismiss = self.fig_style_popup_open.then(|| {
            div()
                .id("tabs-fig-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _w, cx| {
                        this.fig_style_popup_open = false;
                        cx.notify();
                    }),
                )
        });
        // Catch clicks outside the market list on a layer below the cluster to dismiss it.
        let coin_dismiss = self.coin_popup_open.then(|| {
            div()
                .id("tabs-coin-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, common::coin_dismiss_handler(cx))
        });

        // Erased to `AnyElement` immediately: `render_fig_tools` returns `impl IntoElement`, which
        // captures the `&mut cx` borrow for as long as the element lives, and every scaled metric
        // below reads `cx` again.
        let fig_tools = self.render_fig_tools(cx).into_any_element();

        // Right cluster, read left to right as three groups of one job each: what you DRAW on the
        // chart, what you PUT on it, and how you VIEW it. The groups are `chrome_section`s with a
        // `chrome_divider` standing between them, so the boundary comes from the rule rather than
        // from wider spacing — the same block idiom the terminal header and the trading toolbar use.
        // Both settings buttons carry their own anchored popovers, so nothing here positions a popup.
        let right_cluster = div()
            .absolute()
            .right(design::ui_px(cx, 6.0))
            .top(design::ui_px(cx, 4.0))
            .child(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, design::CHROME_GAP))
                    .child(design::chrome_section(cx).child(fig_tools))
                    .child(design::chrome_divider(cx, p_strip))
                    .child(design::chrome_section(cx).child(coin_search_el))
                    .child(design::chrome_divider(cx, p_strip))
                    .child(
                        design::chrome_section(cx)
                            .child(scale_dropdown)
                            .children(gather_btn)
                            .child(candle_btn)
                            .child(graphics_btn)
                            .child(labels_btn)
                            .child(settings_btn),
                    ),
            );
        v_flex()
            .size_full()
            .relative()
            .child(
                // Clip only the tab strip to hide excess tabs. Keep the right cluster and dropdowns
                // as separate children below so the strip height does not clip them.
                div()
                    .h(px(strip_h))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(strip),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.active_element()),
            )
            // Keep `coin_dismiss` below the cluster: list rows handle their own clicks, while this
            // layer catches clicks elsewhere and closes the list.
            .children(coin_dismiss)
            .children(fig_dismiss)
            .child(right_cluster)
    }
}
