//! `ChartPanel` rendering: an own-pass canvas beneath the scene, an input layer for wheel/button/
//! pointer/hover events, and GPUI overlays for the empty-slot logo, FireTest probe, control-zone
//! marker, and chart controls.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonButton, MoonButtonSize, MoonButtonVariant,
    MoonPalette, MoonRect, rgba_from,
};
use rust_i18n::t;

use moon_chart::paint::now_unix_ms;

use super::render_input;
use super::report_trades::ReportTradesStatus;
use super::{ChartPanel, chart_bootstrap_present_rate_hz};

/// Fixed clearance for the chart's top-left control strip, so the status row never competes with
/// its corner controls for the same pixels.
const OVERLAY_LEFT_PX: f32 = 92.0;

/// Right-edge reserve that protects the price scale and the visible price action from an overlong
/// status row.
const OVERLAY_RIGHT_MARGIN_PX: f32 = 120.0;

/// Micro retry-action footprint reserved while it is visible, so the badges do not claim room the
/// button will occupy.
const RETRY_BUTTON_PX: f32 = 72.0;

/// Narrowest slot, in DEVICE pixels, that still states its durable-history status.
///
/// Below it the badge and its Retry button would sit on top of the price action rather than beside
/// it. Sized against the reserves above — the left control strip plus the right-edge margin plus the
/// button — so the row only appears where it has somewhere to go.
const HISTORY_STATUS_MIN_SLOT_W: u32 =
    ((OVERLAY_LEFT_PX + OVERLAY_RIGHT_MARGIN_PX + RETRY_BUTTON_PX) * 2.0) as u32;

impl Render for ChartPanel {
    /// Render the live chart surface and its explicitly unscoped in-chart figure settings.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::CHART_RENDER);
        let _render_us = crate::diag::scope(&crate::diag::CHART_RENDER_US);
        let became_visible = !self.scene_visible;
        self.scene_visible = true;
        self.chart.set_scene_visible(true);
        self.chart
            .set_market_source(Some(self.backend.read(cx).session.market_source()));
        let ppp = window.scale_factor();
        // Cache DPI for the data-prepare path, which has no Window. DPI changes infrequently.
        self.last_ppp = ppp;
        self.chart.set_last_ppp(ppp);
        let palette = MoonPalette::active(cx);
        self.chart.set_ui_palette(palette);
        // Bootstrap only: chartdx refines this from real `gpu_canvas.frame()` cadence,
        // so macOS/Linux do not depend on this fallback staying exact forever.
        let monitor_rate_hz = chart_bootstrap_present_rate_hz();
        let fast_divisor = (monitor_rate_hz / 60.0).round().max(1.0) as u32;
        let effective_present_rate_hz = if self.fast {
            monitor_rate_hz / fast_divisor as f32
        } else {
            60.0
        };
        self.chart.set_present_rate_hz(effective_present_rate_hz);
        // IMPORTANT: there is no request_animation_frame or continuous present. `gpu_canvas.frame()`
        // decides whether to present on the platform tick without dirtying the GPUI tree, and
        // `draw()` renders during that same tick.
        let (
            theme,
            orders_style,
            follow,
            prospective_usd,
            candle_view,
            chart_graphics,
            chart_labels,
            arb_view,
            report_axis,
        ) = {
            let b = self.backend.read(cx);
            let eff = b.preview.as_ref().unwrap_or(&b.config);
            // Prospective F1-F6 order size in dollars for the active coin's crosshair label.
            let prospective = self
                .chart
                .active_target()
                .and_then(|(core, _)| b.prospective_order_usd(core));
            // Select chart-theme and line-style sets from the active light/dark theme. The light
            // set is complete in theme.toml `[light]`, with no runtime palette overrides, so its
            // colors are editable in the same way as the dark set.
            let orders = eff.orders.get(palette.is_light()).clone();
            let theme = eff.theme.get(palette.is_light()).clone();
            // Candles use the panel's per-tab override, or the default of the KIND of tab this
            // panel sits on — the main chart, a torn-off window, or a comparison. Resolving the
            // base field here instead would draw Main's default on every chart in the application
            // and make the split invisible.
            let candle_view = self
                .candle_view
                .unwrap_or_else(|| b.layout.candle_view_for(self.default_kind));
            // Chart graphics follow the same per-tab override as candles: the palette popup writes
            // the tab's own value, and only a tab without one falls back to its kind's default.
            let chart_graphics = self
                .chart_graphics
                .unwrap_or_else(|| b.layout.chart_graphics_for(self.default_kind));
            // Captions come from the settings SIGNATURE rather than being rebuilt here: it already
            // holds this panel's effective value, sanitized, and is restamped by the same backend
            // observation and the same setters that can change it. Rebuilding it per render meant
            // sanitizing sixteen rows and deep-copying their names on every frame.
            let chart_labels = self.settings_sig.chart_labels.clone();
            // The roster is GLOBAL: the same handle for every chart, taken straight off the
            // backend rather than through the per-tab settings signature beside it.
            let arb_view = b.arb_view.clone();
            // Same reasoning as `chart_labels` just above: read off the settings signature, which
            // the observer already restamped before this render ran, instead of rebuilding it here
            // every frame.
            let report_axis = self.settings_sig.report_axis.clone();
            (
                theme,
                orders,
                b.follow,
                prospective,
                candle_view,
                chart_graphics,
                chart_labels,
                arb_view,
                report_axis,
            )
        };
        // The cursor's mode badge is published by `sync_fig_visual` off the backend observer, which
        // runs on every backend notification: it appears on the keypress rather than on the next
        // repaint, and it costs no userdata rebuild — so nothing about it belongs here.
        // Scale is PER TAB: use self.scale, updated through set_scale by the active-tab toolbar or
        // detached-window header, rather than the global backend.price_scale.
        let mut settings_changed = self.chart.set_theme(theme)
            | self.chart.set_orders(orders_style)
            | self.chart.set_scale(self.scale)
            | self.chart.set_orderbook_enabled(self.orderbook_enabled)
            | self
                .chart
                .set_liquidations_enabled(self.liquidations_enabled)
            | self.chart.set_orderbook_only(self.orderbook_only)
            | self.chart.set_candle_view(candle_view)
            | self.chart.set_chart_graphics(chart_graphics)
            | self.chart.set_report_axis(report_axis)
            | self.chart.set_chart_labels(chart_labels)
            | self.chart.set_arb_view(arb_view)
            | self.chart.set_price_axis_pos(self.price_axis_pos)
            | self.chart.set_time_axis_visible(self.time_axis_visible)
            | self.chart.set_line_labels(self.line_labels)
            | self.chart.set_cursor_labels(self.cursor_labels)
            | self.chart.set_prospective_usd(prospective_usd)
            | self.chart.set_follow(follow, now_unix_ms());
        // While compare lock is active, preserve the anchor's Y window, overriding scale each
        // frame. The engine's set_locked_y is idempotent and returns false when unchanged; the
        // panel's set_locked_y handles clearing the lock and restoring configured/automatic scale.
        if let Some((center, range)) = self.locked_y {
            settings_changed |= self.chart.set_locked_y(center, range);
        }
        if settings_changed {
            self.view_dirty = true;
        }
        self.sync_chart_text(cx);

        // Render path only publishes layout/settings dirtiness. Market data is pulled
        // by gpu_canvas.frame(); account/order overlays have their own narrow sync.
        let view_changed = self.view_dirty;
        if became_visible || view_changed {
            self.view_dirty = false;
            self.sync_orders_if_visible(cx, true);
        }

        // Snapshot visible pane layout once and reuse its rectangles for pane-positioned GPUI
        // overlays. The single-pane action overlay uses GPUI layout, while FireTest uses its canvas
        // bounds. Input hit testing receives the engine's current pane rectangles separately.
        let axis_panes = self.chart.axis_panes();
        self.input.pane_rects = self.chart.pane_rects();
        // Input hit testing takes the inputs `chartdx::pane_layout` needs and lets it derive the
        // effective axis position — including the hiding broom mode applies — rather than being
        // handed a pre-resolved one that only half the arithmetic knew about.
        self.input.price_axis_pos = self.price_axis_pos;
        self.input.orderbook_only = self.orderbook_only;
        self.input.orderbook_enabled = self.orderbook_enabled;
        self.input.time_axis_visible = self.time_axis_visible;
        // Place each corner close button on its graph pane in Main and AddToChart. Closing Main's
        // coin returns it to the logo. Convert pane-layout device pixels to slot logical pixels,
        // and collect these positions once for the overlay list.
        let close_btns: Vec<(usize, f32, f32)> = axis_panes
            .iter()
            .map(|(idx, rect, _)| (*idx, (rect.x + rect.w) / ppp, rect.y / ppp))
            .collect();
        // Cursor-only motion is handled by the chart-slot hitbox below. It updates retained
        // gpu_canvas cursor/readout directly and does not notify the GPUI tree.
        // Put the pin at the graph area's top-left plot edge, clear of the configured price-axis
        // gutter, only on TTL-enabled AddToChart panes. Pinning cancels automatic closure. Fields are
        // (idx, pinned, left_px, top_px). PRICE_AXIS_W is logical while pane rects use device px.
        // Pin/lock/broom buttons sit at the plot's LEFT edge, so offset only for a left-side axis.
        // With a right/hidden axis or broom mode, the plot begins at the slot edge with no offset.
        let axis_off = if matches!(
            self.price_axis_pos,
            crate::persistence::chart_persist::PriceAxisPos::Left
        ) && !self.orderbook_only
        {
            moon_chart::PRICE_AXIS_W
        } else {
            0.0
        };
        let pin_btns: Vec<(usize, bool, f32, f32)> = axis_panes
            .iter()
            .filter(|(idx, _, _)| self.chart.pane_is_pinnable(*idx))
            .map(|(idx, rect, _)| {
                (
                    *idx,
                    self.chart.pane_pinned(*idx),
                    rect.x / ppp + axis_off,
                    rect.y / ppp,
                )
            })
            .collect();
        // Show the compare-lock button beside the pin only for horizontal, compare-eligible tabs.
        // It is selected on the anchor. Clicking the active anchor requests comparison shutdown;
        // clicking another chart requests that the stack move it left and make it the price leader.
        let compare_anchor = self.is_compare_anchor;
        let compare_broom_on = self.compare_broom_on;
        let lock_btns: Vec<(usize, f32, f32)> = if self.compare_eligible {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| (*idx, rect.x / ppp + axis_off, rect.y / ppp))
                .collect()
        } else {
            Vec::new()
        };
        // Show the broom only on the anchor beside its selected lock; it toggles book-only mode for
        // the anchor's neighbors.
        let broom_btns: Vec<(usize, f32, f32)> = if self.compare_eligible && compare_anchor {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| (*idx, rect.x / ppp + axis_off, rect.y / ppp))
                .collect()
        } else {
            Vec::new()
        };
        // With separate zones and a hidden order book, shade the right-side order control zone so
        // users can distinguish order-placement clicks from chart double-clicks that open Main.
        // A visible book already marks this area, so do not duplicate it. Neither does a book-only
        // broom pane, whose book covers the whole slot: there is no boundary left to draw, and a
        // strip on the right would name one where the whole pane trades. Tuple fields are
        // (idx, logical left, logical top, logical width, logical height), converted from axis_panes
        // device pixels by dividing by ppp, like the close buttons.
        // `!self.historical` for the reason the strip exists at all: it marks where an
        // order-placement click lands, and a historical viewer places no orders. Shading a strip
        // for a gesture that was just removed would leave the window still saying "trading here".
        let show_zone_marker = self.show_zone
            && !self.historical
            && self.separate_zones(cx)
            && !self.orderbook_drawn();
        let zone_markers: Vec<(usize, f32, f32, f32, f32)> = if show_zone_marker {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| {
                    // The rectangle the CLICKS use, converted to logical pixels — shading anything
                    // else would promise a boundary the hit test does not honour.
                    let zone = self.control_zone_of(*rect);
                    (*idx, zone.x / ppp, zone.y / ppp, zone.w / ppp, zone.h / ppp)
                })
                .collect()
        } else {
            Vec::new()
        };
        // The market buttons — `Cancel Buy`, `Panic Sell`, the temporary-ban lock — are CAPTIONS
        // now, drawn by the chart's own text pass wherever the label configuration puts them.
        // This hands their state to that pass; the press on one is routed by the chart's input,
        // like every other gesture over the plot. See `market_actions`.
        self.sync_market_actions(cx);
        // News card: the live modifier state beats the cached flag, because GPUI delivers modifier
        // changes only along the focus path. Re-validate the hover first — the chart scrolls between
        // pointer events, so a mark can slide out from under a resting cursor.
        let news_ctrl = window.modifiers().secondary();
        self.revalidate_news_hover(cx);
        let news_card = self.news_card(ppp, news_ctrl, palette, cx);
        // Warning badges: same re-validate (the chart scrolls between events); the card is Ctrl-gated
        // like the news card, reusing the same live modifier state.
        self.revalidate_warn_hover(cx);
        let warn_card = self.warn_card(ppp, news_ctrl, palette, cx);
        // Trade arrows: same re-validate, and for a stronger reason — an arrow is anchored to a
        // PRICE, so it slides out from under a resting cursor on a Y auto-fit as well as on a
        // scroll. This card needs no modifier (see `trade_history_hover`).
        self.revalidate_trade_hover(cx);
        let trade_card = self.trade_hover_card(ppp, palette, cx);
        // Per-figure settings panel, opened from a right-click on a figure. Rendered here beside
        // the other overlays rather than as a context menu: it holds swatches and switches, which
        // a menu of text items cannot draw.
        let fig_settings = self.fig_settings.clone().and_then(|(target, at)| {
            let backend = self.backend.clone();
            crate::figstyle::render(
                &backend,
                &target,
                crate::figstyle::WorkspaceAuthority::Unscoped,
                at,
                cx,
            )
        });

        let show_empty_logo = axis_panes.is_empty();
        // The brand follows one switch wherever it is drawn; the cover under it does not. See
        // `design::empty_cover`.
        let empty_logo =
            show_empty_logo && crate::chart_tabs::empty_logo(&self.backend.read(cx).layout);
        // Only the states the user can ACT on are stated. A settled read — trades drawn, or none to
        // draw — is already visible as the arrows themselves, so a badge counting them spends the
        // row's width on a fact the chart is showing anyway, ahead of the live order figures that
        // nothing else states.
        // A chart wide enough to carry the badge states its history status. Gating on Main itself
        // would silence it on a detached or Custom full-window chart, where the chart IS the window
        // and a failed history read would otherwise be undiscoverable; gating on width silences it
        // exactly where it does not fit — a stack slot a few centimetres wide, showing a dozen
        // markets, whose badge would sit on top of the price.
        let status_fits = self.chart.slot_dev_size().0 >= HISTORY_STATUS_MIN_SLOT_W;
        let trade_status = match self.report_trades.status {
            _ if !status_fits => None,
            ReportTradesStatus::Idle | ReportTradesStatus::Ready | ReportTradesStatus::Empty => {
                None
            }
            ReportTradesStatus::Loading => Some(t!("chart.trade_history.loading").to_string()),
            ReportTradesStatus::NotReady => Some(t!("chart.trade_history.not_ready").to_string()),
            ReportTradesStatus::Failed => Some(t!("chart.trade_history.failed").to_string()),
        };
        let trade_retry = status_fits
            && matches!(
                self.report_trades.status,
                ReportTradesStatus::NotReady | ReportTradesStatus::Failed
            );
        let (slot_w, _) = self.chart.slot_dev_size();
        // Live open-order figures for the chart's active pane, assembled beside the closed-trade
        // badge above. The two share a row but nothing else: this reads the live session store,
        // that one a durable report snapshot.

        // Placeholder lockup width for this pane. The share and its bounds are brand geometry, so
        // they live with the artwork in `design`, not as three literals in a panel.
        let logo_w = ((slot_w as f32 / ppp) * crate::design::CHART_LOGO_SLOT_SHARE).clamp(
            crate::design::CHART_LOGO_MIN_W,
            crate::design::CHART_LOGO_MAX_W,
        );
        div()
            .id("chart-slot")
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .relative()
            .track_focus(&self.focus)
            // Over a draggable order line, whether hovered or actively dragged, use the vertical
            // resize cursor because the line moves only along price (Y). Avoid separate grab/grabbing
            // cursors; ns-resize communicates the one-dimensional motion more precisely. Over the
            // START CROSS of an unfilled entry, use a pointer because clicking cancels the order.
            .map(|this| {
                if self.order_drag.is_some() {
                    this.cursor_ns_resize()
                } else if let Some(hover) = self.order_hover {
                    if hover.cancel {
                        this.cursor_pointer()
                    } else {
                        this.cursor_ns_resize()
                    }
                } else {
                    this
                }
            })
            .on_scroll_wheel(cx.listener(render_input::scroll_wheel))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(render_input::mouse_down_left),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(render_input::mouse_up_left))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(render_input::mouse_down_right),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(render_input::mouse_up_right),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(render_input::mouse_down_middle),
            )
            .on_mouse_move(cx.listener(render_input::mouse_move))
            .on_hover(cx.listener(render_input::hover))
            // Ctrl gates the news-mark card. Without this the card would only appear after the
            // pointer moved AGAIN, so pressing Ctrl while already parked on a ring did nothing.
            .on_modifiers_changed(cx.listener(
                |this: &mut ChartPanel, e: &ModifiersChangedEvent, _w, cx| {
                    this.note_news_modifiers(e.modifiers, cx);
                    this.note_warn_modifiers(e.modifiers, cx);
                },
            ))
            // The own-pass engine synchronously obtains slot geometry from `GpuFrameInfo.bounds`
            // in `frame()`; see data_state::apply_slot_geometry. The first present therefore uses
            // the real slot, without expanding to a default size or lagging during reflow.
            .child(self.chart.canvas().text_under().absolute().size_full())
            // Transparent zones over the arbitrage venue NAMES, for one reason: a native cursor can
            // only be asked for during PAINT, so a hover handler cannot set it — a styled element
            // over the name can. They carry no click handler; the press is still routed by the
            // chart's own input, which is where every other chart gesture is decided.
            //
            // Rectangles come from what the LAST frame drew, which is a frame behind during a
            // resize. That is invisible for a cursor and would be wrong for a click, which is the
            // other reason the click is not handled here.
            .children(self.arb_cursor_zones())
            // The market buttons: real controls, placed where the caption layout reserved room for
            // them. See `market_actions`.
            .children(self.action_buttons(cx))
            // Top-left status row. The container is hoisted out of the trade-history status so the
            // live order figures still appear while that durable read is Idle; it is rendered only
            // when at least one of the two sources produced something.
            .children(trade_status.is_some().then(|| {
                let entity = cx.entity();
                div()
                    .absolute()
                    .top(px(6.0))
                    .left(px(OVERLAY_LEFT_PX))
                    .flex()
                    .items_center()
                    .gap_1()
                    // Defensive only: the width budget already dropped anything that would not
                    // fit, so clipping here means the badge measurement under-estimated. It is
                    // the backstop for that estimate being an estimate, not a layout mode this
                    // row is meant to reach.
                    .overflow_hidden()
                    .children(trade_status.map(|status| {
                        MoonBadge::new(status)
                            .variant(MoonBadgeVariant::Soft)
                            .size(MoonBadgeSize::Tiny)
                            .render()
                    }))
                    .when(trade_retry, |this| {
                        this.child(
                            MoonButton::new("chart-trade-history-retry")
                                .label(t!("chart.trade_history.retry").to_string())
                                .size(MoonButtonSize::Micro)
                                .variant(MoonButtonVariant::Ghost)
                                .on_click(move |_, _window, app| {
                                    entity.update(app, |this, cx| this.retry_trade_history(cx));
                                })
                                .render(),
                        )
                    })
            }))
            .when(show_empty_logo, |this| {
                // Cover the own pass with an opaque chart background in an empty slot so a stale
                // graph rendered beneath the GPUI scene does not show through. The COVER is not
                // optional — it is hiding something — and only the mark on it follows the switch;
                // both come from one builder so that cannot be got wrong here.
                this.child(
                    crate::design::empty_cover(cx, palette.chart_bg, empty_logo.then_some(logo_w))
                        .absolute(),
                )
            })
            // FireTest probe only. Do not source chart geometry from this GPUI probe;
            // `GpuFrameInfo.bounds` is the sole source of truth for input and own-pass rendering.
            .child({
                let is_main = self.num.is_none();
                let backend = self.backend.clone();
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        let sf = window.scale_factor();
                        let firetest_probe = crate::firetest::ChartProbe::new(
                            crate::window::windowing::window_hwnd(window),
                            f32::from(window.window_bounds().get_bounds().origin.x),
                            f32::from(window.window_bounds().get_bounds().origin.y),
                            f32::from(bounds.origin.x),
                            f32::from(bounds.origin.y),
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                            sf,
                        );
                        if is_main {
                            if let Some(probe) = firetest_probe {
                                backend.update(cx, |b, _| {
                                    crate::firetest::observe_chart_probe(b, probe);
                                });
                            }
                        }
                    },
                )
                .absolute()
                .size_full()
            })
            .children(zone_markers.into_iter().map(|(_idx, left, top, w, h)| {
                // Faintly shade the control zone while the order book is hidden, without a border line.
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(w))
                    .h(px(h))
                    .bg(rgba_from(palette.blue, 0.03))
            }))
            // News-mark card: above the chart, below every chart control (close/pin/lock/broom and
            // the action buttons), so it can never swallow a trading click's target.
            .children(news_card)
            .children(warn_card)
            .children(trade_card)
            .children(close_btns.into_iter().map(|(idx, right, top)| {
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-close-{idx}")))
                    // Make the close glyph brighter and heavier than the former muted
                    // text_muted@0.78 Ghost foreground. `text_segment` supplies full `text` color
                    // and weight 700; backdrop and hover still follow Ghost (transparent by
                    // default, with a light hover background).
                    .text_segment("×", palette.text, 700.0)
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    // Use a 22×22 hit area to avoid missing into the book when closing charts quickly.
                    .bounds(MoonRect::new(right - 26.0, top + 3.0, 22.0, 22.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.remove_pane(idx, cx));
                    })
                    .render()
            }))
            .children(pin_btns.into_iter().map(|(idx, pinned, left, top)| {
                // Top-left pin button: filled circle means pinned; outline means unpinned.
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-pin-{idx}")))
                    .label(if pinned { "●" } else { "○" })
                    .size(MoonButtonSize::Micro)
                    .variant(if pinned {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(pinned)
                    .bounds(MoonRect::new(left + 3.0, top + 3.0, 15.0, 15.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.toggle_pin(idx, cx));
                    })
                    .render()
            }))
            .children(lock_btns.into_iter().map(|(idx, left, top)| {
                // Lock to the right of the pin: click the anchor to disable comparison, or click a
                // non-anchor chart to move it first and make it the price leader.
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-lock-{idx}")))
                    .label("🔒")
                    .size(MoonButtonSize::Micro)
                    .variant(if compare_anchor {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(compare_anchor)
                    .bounds(MoonRect::new(left + 21.0, top + 3.0, 15.0, 15.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.request_compare_lock(cx));
                    })
                    .render()
            }))
            .children(broom_btns.into_iter().map(|(idx, left, top)| {
                // Broom to the right of the anchor's lock: toggle book-only neighbors.
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-broom-{idx}")))
                    .label("🧹")
                    .size(MoonButtonSize::Micro)
                    .variant(if compare_broom_on {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Ghost
                    })
                    .selected(compare_broom_on)
                    .bounds(MoonRect::new(left + 39.0, top + 3.0, 15.0, 15.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.request_compare_broom(cx));
                    })
                    .render()
            }))
            // LAST of the overlays: the settings panel is opened deliberately and edits what is
            // under it, so a close/pin/broom button painting over its top rows — and taking the
            // clicks meant for them — would make it unusable exactly where a figure is easiest to
            // right-click, near the top of a pane.
            .children(fig_settings)
    }
}
