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
use crate::persistence::chart_persist::ChartBtnPos;

/// Market-action button type for the chart overlay.
#[derive(Clone, Copy)]
enum ActKind {
    CancelBuy,
    PanicSell,
}

/// Builds a Cancel Buy or Panic Sell button shared by per-pane and fullscreen overlays.
///
/// Labels, variants, and click handling are identical; callers supply the target plus its live
/// workspace permission and may add `.full_width()`. Moonbot product terms remain untranslated.
///
/// Args:
///     kind: Market action represented by the button.
///     id: Stable GPUI element identifier.
///     armed: Whether panic sell is currently armed for the target.
///     backend: Live command and workspace authority.
///     workspace_group: Owning chart group, or `None` for an unscoped diagnostic chart.
///     allowed: Render-time permission used to disable stale Auto targets visibly.
///     core: Core targeted by the command.
///     market: Canonical market targeted by the command.
///
/// Returns:
///     A button whose callback revalidates the workspace before dispatch.
fn action_button(
    kind: ActKind,
    id: SharedString,
    armed: bool,
    backend: Entity<crate::Backend>,
    workspace_group: Option<String>,
    allowed: bool,
    core: moon_core::session::CoreId,
    market: String,
) -> MoonButton {
    let (label, variant, selected) = match kind {
        ActKind::CancelBuy => ("Cancel Buy", MoonButtonVariant::Soft, false),
        // An armed panic becomes "Stop Panic", matching MoonBot's second-click
        // `TTurnPanicSell off` behavior. This exposes the ability to disarm panic, which is
        // required before moving sell below the AllowedDrop floor.
        ActKind::PanicSell if armed => ("Stop Panic", MoonButtonVariant::Danger, true),
        ActKind::PanicSell => ("Panic Sell", MoonButtonVariant::Danger, false),
    };
    MoonButton::new(id)
        .label(label)
        .size(MoonButtonSize::Micro)
        .variant(variant)
        .selected(selected)
        .disabled(!allowed)
        .on_click(move |_, _w, app| {
            backend.update(app, |b, cx| {
                if !b.workspace_action_allows_core(workspace_group.as_deref(), core) {
                    return;
                }
                match kind {
                    ActKind::CancelBuy => {
                        if let Err(error) = b.session.cancel_market_buys(core, market.clone()) {
                            log::warn!("cancel market buys failed: {error:#}");
                        }
                    }
                    ActKind::PanicSell => {
                        b.toggle_panic_sell(core, market.clone());
                        cx.notify();
                    }
                }
            });
        })
}

impl Render for ChartPanel {
    /// Render the live chart surface and its explicitly unscoped in-chart figure settings.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::CHART_RENDER);
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
        let (theme, orders_style, follow, prospective_usd, candle_view) = {
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
            // Candles use the panel's per-tab override or the global layout default.
            let candle_view = self.candle_view.unwrap_or(b.layout.candle_view);
            (theme, orders, b.follow, prospective, candle_view)
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
        // Input hit testing needs the axis side to account for plot inset/width. Broom mode hides it.
        self.input.price_axis_pos = if self.orderbook_only {
            crate::persistence::chart_persist::PriceAxisPos::Hide
        } else {
            self.price_axis_pos
        };
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
        // A visible book already marks this area, so do not duplicate it. Tuple fields are
        // (idx, logical left, logical top, logical width, logical height), converted from axis_panes
        // device pixels by dividing by ppp, like the close buttons.
        let show_zone_marker = self.show_zone && self.separate_zones(cx) && !self.orderbook_enabled;
        let zone_markers: Vec<(usize, f32, f32, f32, f32)> = if show_zone_marker {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| {
                    let zone_w = moon_chart::GLASS_ZONE_PX.min(rect.w * 0.5);
                    // A hidden time axis reserves no label gutter, so the zone reaches the slot bottom.
                    let time_axis_h = if self.time_axis_visible {
                        moon_chart::TIME_AXIS_H * ppp
                    } else {
                        0.0
                    };
                    let plot_h = (rect.h - time_axis_h).max(1.0);
                    (
                        *idx,
                        (rect.x + rect.w - zone_w) / ppp,
                        rect.y / ppp,
                        zone_w / ppp,
                        plot_h / ppp,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        // Render Cancel Buy / Panic Sell as a GPUI overlay at the bottom of the graph body, ABOVE
        // the time-axis row. OverScene axis text renders above GPUI, so avoid its area. Each tab's
        // setting chooses Hide/Left/Center/Right. Keep buttons strictly inside the chart zone:
        // exclude the price-axis gutter on its configured side and the book/control zone on the
        // right even when the book is disabled. Shrink buttons when needed; overflow_hidden clips
        // text on the right.
        // Tuple fields: (kind, x, top, width, height, core, market, armed).
        const ACT_BTN_W: f32 = 92.0;
        const ACT_GAP: f32 = 8.0;
        const ACT_MIN_W: f32 = 30.0;
        // These use MoonButton Micro, like the chart's close/pin/lock overlays. The height comes
        // from the shared helper so it follows the font slider just like the button does, and so
        // the mirrored MoonUI metrics live in exactly one place. This layout controls button
        // width from the available chart zone.
        let act_btn_h = crate::design::micro_control_h_value(cx);
        let cancel_pos = self.cancel_buy_pos;
        let panic_pos = self.panic_sell_pos;
        // For the container's populated pane, place buttons through the GPUI `action_overlay` below
        // so they resize synchronously with the slot. Positions from axis_panes use own-pass
        // geometry (`data.w/h`), which updates on the present tick and can lag a fullscreen toggle
        // by several frames, making buttons jump. Stack/compare layouts compose separate ChartPanels;
        // each chart container itself holds at most one pane.
        let single_pane =
            !self.orderbook_only && axis_panes.len() == 1 && self.chart.pane_target(0).is_some();
        let mut action_btns: Vec<(
            ActKind,
            f32,
            f32,
            f32,
            f32,
            moon_core::session::CoreId,
            String,
            bool,
        )> = Vec::new();
        if !single_pane && !self.orderbook_only {
            for (idx, rect, _) in axis_panes.iter() {
                let Some((core, market)) = self.chart.pane_target(*idx) else {
                    continue;
                };
                let pane_left = rect.x / ppp;
                let pane_w = rect.w / ppp;
                // Chart zone spans from the price axis to the start of the book/control zone.
                // Always reserve the book zone, even when disabled. Remove the axis gutter from its
                // actual side: left via axis_off, or right beyond the book via an extra reserve.
                let glass_reserve = moon_chart::GLASS_ZONE_PX.min(pane_w * 0.5);
                let right_axis_reserve = if matches!(
                    self.price_axis_pos,
                    crate::persistence::chart_persist::PriceAxisPos::Right
                ) {
                    moon_chart::PRICE_AXIS_W
                } else {
                    0.0
                };
                let zone_left = pane_left + axis_off;
                let zone_right = pane_left + pane_w - glass_reserve - right_axis_reserve;
                let zone_w = zone_right - zone_left;
                if zone_w < ACT_MIN_W {
                    continue;
                }
                let time_axis_reserve = if self.time_axis_visible {
                    moon_chart::TIME_AXIS_H
                } else {
                    0.0
                };
                let top = (rect.y + rect.h) / ppp - time_axis_reserve - act_btn_h - 10.0;
                let armed = self.backend.read(cx).is_panic_armed(core, &market);
                // Visible buttons as (kind, anchor), in stable order.
                let mut vis: Vec<(ActKind, ChartBtnPos)> = Vec::new();
                if cancel_pos != ChartBtnPos::Hide {
                    vis.push((ActKind::CancelBuy, cancel_pos));
                }
                if panic_pos != ChartBtnPos::Hide {
                    vis.push((ActKind::PanicSell, panic_pos));
                }
                if vis.is_empty() {
                    continue;
                }
                // Shrink globally so ALL buttons fit in one row; otherwise different left/right
                // anchors overlap on a narrow chart.
                let n = vis.len() as f32;
                let bw = if n * ACT_BTN_W + (n - 1.0) * ACT_GAP > zone_w {
                    ((zone_w - (n - 1.0) * ACT_GAP) / n).max(ACT_MIN_W)
                } else {
                    ACT_BTN_W
                };
                let hi = (zone_right - bw).max(zone_left);
                let anchor_x = |a: ChartBtnPos| -> f32 {
                    let x = match a {
                        ChartBtnPos::Left => zone_left,
                        ChartBtnPos::Center => zone_left + (zone_w - bw) * 0.5,
                        ChartBtnPos::Right => zone_right - bw,
                        ChartBtnPos::Hide => zone_left,
                    };
                    x.clamp(zone_left, hi)
                };
                let order = |a: ChartBtnPos| -> u8 {
                    match a {
                        ChartBtnPos::Left => 0,
                        ChartBtnPos::Center => 1,
                        ChartBtnPos::Right => 2,
                        ChartBtnPos::Hide => 0,
                    }
                };
                let mut placed: Vec<(ActKind, f32)> = Vec::new();
                if vis.len() == 1 {
                    placed.push((vis[0].0, anchor_x(vis[0].1)));
                } else if vis[0].1 == vis[1].1 {
                    // Same anchor: place both buttons in a row at that anchor.
                    let total = 2.0 * bw + ACT_GAP;
                    let start = match vis[0].1 {
                        ChartBtnPos::Center => zone_left + (zone_w - total) * 0.5,
                        ChartBtnPos::Right => zone_right - total,
                        _ => zone_left,
                    }
                    .clamp(zone_left, (zone_right - total).max(zone_left));
                    placed.push((vis[0].0, start));
                    placed.push((vis[1].0, start + bw + ACT_GAP));
                } else {
                    // Different anchors: place left-to-right by anchor order without overlap.
                    let (mut a, mut b) = (vis[0], vis[1]);
                    if order(a.1) > order(b.1) {
                        std::mem::swap(&mut a, &mut b);
                    }
                    let xa = anchor_x(a.1);
                    let xb = anchor_x(b.1).max(xa + bw + ACT_GAP).clamp(zone_left, hi);
                    placed.push((a.0, xa));
                    placed.push((b.0, xb));
                }
                for (kind, x) in placed {
                    action_btns.push((kind, x, top, bw, act_btn_h, core, market.clone(), armed));
                }
            }
        }
        // The SINGLE-pane action overlay uses pure GPUI layout (insets + flex), without own-pass
        // geometry, keeping positions synchronized with the slot and preventing fullscreen jumps.
        // The chart zone is the slot minus the configured-side price-axis gutter, right
        // book/control zone, and bottom time axis. Its regions correspond to Left/Center/Right
        // anchors.
        let action_overlay = if single_pane {
            self.chart.pane_target(0).and_then(|(core, market)| {
                let backend = self.backend.read(cx);
                let armed = backend.is_panic_armed(core, &market);
                let allowed = self.workspace_action_allowed(&backend, core);
                let backend0 = self.backend.clone();
                let workspace_group = self.workspace_group.clone();
                let mk = |kind: ActKind| -> AnyElement {
                    let id = match kind {
                        ActKind::CancelBuy => "chart-cancelbuy-fs",
                        ActKind::PanicSell => "chart-panic-fs",
                    };
                    action_button(
                        kind,
                        SharedString::from(id),
                        armed,
                        backend0.clone(),
                        workspace_group.clone(),
                        allowed,
                        core,
                        market.clone(),
                    )
                    .render()
                    .into_any_element()
                };
                let mut left: Vec<AnyElement> = Vec::new();
                let mut center: Vec<AnyElement> = Vec::new();
                let mut right: Vec<AnyElement> = Vec::new();
                for (kind, pos) in [
                    (ActKind::CancelBuy, cancel_pos),
                    (ActKind::PanicSell, panic_pos),
                ] {
                    match pos {
                        ChartBtnPos::Left => left.push(mk(kind)),
                        ChartBtnPos::Center => center.push(mk(kind)),
                        ChartBtnPos::Right => right.push(mk(kind)),
                        ChartBtnPos::Hide => {}
                    }
                }
                if left.is_empty() && center.is_empty() && right.is_empty() {
                    return None;
                }
                // Size regions by CONTENT rather than flex_1 so buttons retain their width when two
                // share an anchor (both default to Right). Two flex spacers establish L/C/R anchoring.
                // Left padding is the price axis only when it is on the left; right padding is the
                // book zone plus the axis gutter when the axis is on the right beyond the book.
                let region = |btns: Vec<AnyElement>| {
                    div().flex().items_center().gap(px(ACT_GAP)).children(btns)
                };
                let left_pad = if matches!(
                    self.price_axis_pos,
                    crate::persistence::chart_persist::PriceAxisPos::Left
                ) {
                    moon_chart::PRICE_AXIS_W
                } else {
                    0.0
                };
                let right_pad = moon_chart::GLASS_ZONE_PX
                    + if matches!(
                        self.price_axis_pos,
                        crate::persistence::chart_persist::PriceAxisPos::Right
                    ) {
                        moon_chart::PRICE_AXIS_W
                    } else {
                        0.0
                    };
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .pl(px(left_pad))
                        .pr(px(right_pad))
                        .pb(px(if self.time_axis_visible {
                            moon_chart::TIME_AXIS_H + 10.0
                        } else {
                            10.0
                        }))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .child(
                            div()
                                .w_full()
                                .h(px(act_btn_h))
                                .flex()
                                .items_center()
                                .child(region(left))
                                .child(div().flex_1())
                                .child(region(center))
                                .child(div().flex_1())
                                .child(region(right)),
                        )
                        .into_any_element(),
                )
            })
        } else {
            None
        };
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
        let trade_status = match self.report_trades.status {
            ReportTradesStatus::Idle => None,
            ReportTradesStatus::Loading => Some(t!("chart.trade_history.loading").to_string()),
            ReportTradesStatus::Ready { count, truncated } => Some(
                t!(
                    if truncated {
                        "chart.trade_history.ready_truncated"
                    } else {
                        "chart.trade_history.ready"
                    },
                    count = count
                )
                .to_string(),
            ),
            ReportTradesStatus::Empty => Some(t!("chart.trade_history.empty").to_string()),
            ReportTradesStatus::NotReady => Some(t!("chart.trade_history.not_ready").to_string()),
            ReportTradesStatus::Failed => Some(t!("chart.trade_history.failed").to_string()),
        };
        let trade_retry = matches!(
            self.report_trades.status,
            ReportTradesStatus::NotReady | ReportTradesStatus::Failed
        );
        let (slot_w, _) = self.chart.slot_dev_size();
        let logo_w = ((slot_w as f32 / ppp) * 0.28).clamp(180.0, 280.0);
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
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(render_input::mouse_up_middle),
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
            .children(trade_status.map(|status| {
                let entity = cx.entity();
                div()
                    .absolute()
                    .top(px(6.0))
                    .left(px(92.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        MoonBadge::new(status)
                            .variant(MoonBadgeVariant::Soft)
                            .size(MoonBadgeSize::Tiny)
                            .render(),
                    )
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
                // Cover the own pass with an opaque chart background in an empty slot so its logo
                // does not reveal a stale graph rendered beneath the GPUI scene.
                this.child(
                    div()
                        .absolute()
                        .size_full()
                        .bg(rgb(palette.chart_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::design::logo_glow_sized(cx, logo_w)),
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
            .children(action_btns.into_iter().enumerate().map(
                |(i, (kind, x, y, w, h, core, market, armed))| {
                    let id = match kind {
                        ActKind::CancelBuy => format!("chart-cancelbuy-{i}"),
                        ActKind::PanicSell => format!("chart-panic-{i}"),
                    };
                    let allowed = self.workspace_action_allowed(&self.backend.read(cx), core);
                    let btn = action_button(
                        kind,
                        SharedString::from(id),
                        armed,
                        self.backend.clone(),
                        self.workspace_group.clone(),
                        allowed,
                        core,
                        market,
                    )
                    .full_width()
                    .render();
                    // The container sets text-clipping width; overflow clips to bounds on BOTH axes.
                    // Add 4 px of height so the top-aligned button remains fully inside and its
                    // bottom is not cut off.
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(h + 4.0))
                        .overflow_x_hidden()
                        .child(btn)
                },
            ))
            .children(action_overlay)
            // LAST of the overlays: the settings panel is opened deliberately and edits what is
            // under it, so a close/pin/broom button painting over its top rows — and taking the
            // clicks meant for them — would make it unusable exactly where a figure is easiest to
            // right-click, near the top of a pane.
            .children(fig_settings)
    }
}
