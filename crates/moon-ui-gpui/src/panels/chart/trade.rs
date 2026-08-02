//! Manual chart trading: order placement from configured mouse gestures or cursor hotkeys,
//! order-line hit testing and cancellation, hover/drag visuals, command routing, and native cursor
//! synchronization. Buy/Sell drags use `move_order`; stop and take-profit drags use
//! `move_order_stop_price`. This module was extracted from `chart.rs`.

use gpui::*;
use std::time::{Duration, Instant};

use moon_core::config::MouseGestureBinding;
use moon_core::feed::OrderLinePriceKind;
use moon_core::session::CoreId;
use moon_core::session::order_lines::LineKind;

use super::ChartPanel;

const ORDER_DRAG_PREVIEW_HOLD: Duration = Duration::from_millis(3_000);

/// Cursor movement thresholds for repeating order-line hit testing.
///
/// The Delphi reference scans only after movement of at least one X pixel or half a Y pixel. Raw
/// mouse-move events arrive more often with subpixel jitter, for which rescanning is wasted work.
const ORDER_HOVER_MOVE_X: f32 = 1.0;
const ORDER_HOVER_MOVE_Y: f32 = 0.5;

/// Return whether the cursor moved far enough since the previous probe to recompute line hover.
///
/// A missing previous point, on first entry or return to the chart, always requires a probe.
pub(super) fn hover_probe_due(prev: Option<(f32, f32)>, pos: (f32, f32)) -> bool {
    match prev {
        Some((px, py)) => {
            (pos.0 - px).abs() >= ORDER_HOVER_MOVE_X || (pos.1 - py).abs() >= ORDER_HOVER_MOVE_Y
        }
        None => true,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TradeMouseButton {
    Left,
    Middle,
    Right,
}

pub(super) struct OrderDrag {
    core: CoreId,
    uid: u64,
    kind: LineKind,
    pane: usize,
    start_price: f64,
    current_price: f64,
}

#[derive(Clone, Copy)]
pub(super) struct PendingOrderDrag {
    core: CoreId,
    uid: u64,
    kind: LineKind,
    price: f32,
    started: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OrderHoverKey {
    core: CoreId,
    uid: u64,
    /// Whether the cursor is over the click-to-cancel start cross, selecting a pointer cursor
    /// instead of the vertical drag cursor.
    pub(super) cancel: bool,
}

struct OrderHit {
    core: CoreId,
    uid: u64,
    kind: LineKind,
    pane: usize,
    price: f32,
    /// Order market used by MoonProto's per-market join and split actions.
    market: String,
    /// Order position direction used by the join-sells action.
    short: bool,
    /// Whether the cursor is on the start cross of an unfilled Buy line.
    ///
    /// Clicking this target cancels the order, as in Moonbot, instead of starting a drag.
    on_start_cross: bool,
}

impl ChartPanel {
    fn gesture_matches(
        binding: MouseGestureBinding,
        button: TradeMouseButton,
        modifiers: Modifiers,
        click_count: usize,
    ) -> bool {
        let dbl = click_count >= 2;
        let clear = !modifiers.modified();
        match binding {
            MouseGestureBinding::None => false,
            MouseGestureBinding::LeftDouble => button == TradeMouseButton::Left && dbl && clear,
            MouseGestureBinding::LeftCtrl => button == TradeMouseButton::Left && modifiers.control,
            MouseGestureBinding::LeftShift => button == TradeMouseButton::Left && modifiers.shift,
            MouseGestureBinding::LeftAlt => button == TradeMouseButton::Left && modifiers.alt,
            MouseGestureBinding::Middle => button == TradeMouseButton::Middle && clear,
            MouseGestureBinding::MiddleCtrl => {
                button == TradeMouseButton::Middle && modifiers.control
            }
            MouseGestureBinding::MiddleShift => {
                button == TradeMouseButton::Middle && modifiers.shift
            }
            MouseGestureBinding::MiddleAlt => button == TradeMouseButton::Middle && modifiers.alt,
            MouseGestureBinding::RightDouble => button == TradeMouseButton::Right && dbl && clear,
            MouseGestureBinding::RightCtrl => {
                button == TradeMouseButton::Right && modifiers.control
            }
            MouseGestureBinding::RightShift => button == TradeMouseButton::Right && modifiers.shift,
            MouseGestureBinding::RightAlt => button == TradeMouseButton::Right && modifiers.alt,
            MouseGestureBinding::LeftCtrlDouble => {
                button == TradeMouseButton::Left && dbl && modifiers.control
            }
            MouseGestureBinding::LeftShiftDouble => {
                button == TradeMouseButton::Left && dbl && modifiers.shift
            }
            MouseGestureBinding::LeftAltDouble => {
                button == TradeMouseButton::Left && dbl && modifiers.alt
            }
        }
    }

    pub(super) fn try_place_order_click(
        &mut self,
        button: TradeMouseButton,
        modifiers: Modifiers,
        click_count: usize,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        // Suppress placement just after closing a chart. The OS can deliver the second click of a
        // close-button double-click to the newly exposed fullscreen chart, which would otherwise
        // create an accidental order while tabs are being closed quickly.
        if moon_chart::paint::now_unix_ms() - self.last_pane_close_ms < super::ORDER_SUPPRESS_MS {
            return false;
        }
        // Resolve Long/Short from the configured buy-set and short-set gestures; unrelated gestures
        // are not order placement.
        let short = {
            let b = self.backend.read(cx);
            let cfg = b.preview.as_ref().unwrap_or(&b.config);
            if Self::gesture_matches(cfg.hotkeys.buy_set_click, button, modifiers, click_count) {
                Some(false)
            } else if Self::gesture_matches(
                cfg.hotkeys.short_set_click,
                button,
                modifiers,
                click_count,
            ) {
                Some(true)
            } else {
                None
            }
        };
        let Some(short) = short else {
            return false;
        };
        self.place_order_at_pos(pos, short, cx)
    }

    /// Return the core and market under this panel's cursor for non-price hotkeys.
    ///
    /// Returns:
    ///     The hovered pane's target, or `None` after the pointer leaves the chart.
    pub(crate) fn target_at_cursor(&self) -> Option<(CoreId, String)> {
        let pane = self.input.hovered_pane?;
        self.chart
            .with_container(|container| container.target(pane))
    }

    /// Place a manual order at the price under the chart cursor for the new-long/new-short hotkey.
    ///
    /// The chart owns pane-Y-to-price conversion, so placement remains here rather than in the
    /// shared hotkey dispatcher. Returns `false` when the cursor is not over a pane.
    pub(crate) fn place_order_at_cursor(&mut self, short: bool, cx: &mut Context<Self>) -> bool {
        match self.input.cursor {
            Some(pos) => self.place_order_at_pos(pos, short, cx),
            None => false,
        }
    }

    /// Place a manual order at slot-pixel position `pos`, using `short` to select its position side.
    ///
    /// `false` selects Long and `true` selects Short. This shared mouse/hotkey path resolves pane,
    /// price, and `(core, market)`, then converts the core group's visible USD-equivalent size.
    fn place_order_at_pos(&mut self, pos: (f32, f32), short: bool, cx: &mut Context<Self>) -> bool {
        // In separate-zone mode place only from the order-book zone; otherwise accept any pane area.
        let pane = if self.separate_zones(cx) {
            self.glass_pane_at(pos)
        } else {
            self.input.pane_at(pos.0, pos.1)
        };
        let Some(pane) = pane else {
            return false;
        };
        let Some(price) = self.price_at_pane_y(pane, pos.1) else {
            return false;
        };
        let Some((core, market)) = self
            .chart
            .with_container(|container| container.target(pane))
        else {
            return false;
        };

        let placed = self.backend.update(cx, |b, _| {
            let Some(terms) = b.manual_order_terms(core, None) else {
                log::warn!(
                    "manual chart order blocked: core={core} market={market} has no complete local terms or valid base/USD rate"
                );
                return false;
            };
            let Some(usd) = terms.size_usd else {
                return false;
            };
            match b
                .session
                .place_order(
                    core,
                    market.clone(),
                    short,
                    price,
                    terms.size_base,
                    None,
                    terms.exit,
                )
            {
                Ok(()) => {
                    log::info!(
                        "manual chart order: core={core} market={market} side={} price={price:.8} size={} usd={usd}",
                        if short { "short" } else { "long" },
                        terms.size_base
                    );
                    true
                }
                Err(err) => {
                    log::warn!(
                        "manual chart order failed: core={core} market={market} price={price:.8}: {err:#}"
                    );
                    false
                }
            }
        });
        // Per-window/tab auto-pin keeps a chart that accepted an order from expiring through TTL
        // or inactivity.
        if placed
            && self.auto_pin
            && self.chart.pane_is_pinnable(pane)
            && !self.chart.pane_pinned(pane)
            && self.chart.toggle_pane_pin(pane)
        {
            self.view_dirty = true;
            self.arm_ttl_timer(cx);
        }
        placed
    }

    /// Hit-test interactive order lines under the cursor.
    ///
    /// `cross_only` applies outside the order book in separate-zone mode, where the only target is
    /// an unfilled Buy line's click-to-cancel start cross. It scans only Buy lines and gates on the
    /// cross's X range before computing unnecessary distances for all draggable kinds, as in Delphi.
    fn hit_order_line(
        &self,
        pos: (f32, f32),
        cross_only: bool,
        cx: &mut Context<Self>,
    ) -> Option<OrderHit> {
        let Some(pane) = self.input.pane_at(pos.0, pos.1) else {
            return None;
        };
        let Some((core, market)) = self
            .chart
            .with_container(|container| container.target(pane))
        else {
            return None;
        };
        let Some(plot) = self.local_plot_rect(pane) else {
            return None;
        };
        let Some((center, range, epoch_ms, left_rel, window_ms)) =
            self.chart.with_container(|container| {
                container.pane(pane).map(|pane| {
                    let (left, window) = pane.view.visible_x(plot.w);
                    (
                        pane.view.render_center,
                        pane.view.render_range,
                        pane.view.epoch_ms,
                        left,
                        window,
                    )
                })
            })
        else {
            return None;
        };
        if plot.h <= 1.0 || !(range > 0.0) || !(window_ms > 0.0) {
            return None;
        }
        // Map the line's first step to its starting X with the same transform as rendering.
        let x_of_time =
            |t_ms: f64| plot.x + ((t_ms - epoch_ms) as f32 - left_rel) / window_ms * plot.w;
        let threshold = (6.0 * self.last_ppp).max(6.0);
        let mut best: Option<(u64, LineKind, f32, bool, f32, f32, f32)> = None;
        if let Some(core_data) = self.backend.read(cx).session.store().core(core) {
            for order in core_data
                .order_lines
                .iter_market(&market)
                .filter(|order| order.closed_ms.is_none())
            {
                // Drag Buy/Sell through `move_order` and SL/Trailing/TakeProfit through absolute
                // `move_order_stop_price` updates. VStop and pending-condition lines have no price
                // level set by dragging and are therefore excluded.
                let kinds: &[LineKind] = if cross_only {
                    &[LineKind::Buy]
                } else {
                    &[
                        LineKind::Buy,
                        LineKind::Sell,
                        LineKind::Stop,
                        LineKind::Trailing,
                        LineKind::TakeProfit,
                    ]
                };
                for &kind in kinds {
                    // A Buy entry, including a short entry, is draggable only while unfilled: its
                    // live limit can be replaced through `move_order`. After any fill, the Buy line
                    // is historical; manage the position through its Sell exit and stops instead.
                    if kind == LineKind::Buy && order.fill_pct > 0.0 {
                        continue;
                    }
                    let line = &order.lines[kind as usize];
                    let Some(price) = line.current_price().filter(|p| p.is_finite() && *p > 0.0)
                    else {
                        continue;
                    };
                    // A line exists only from its first step to the right edge. Reject points left
                    // of that start so dragging cannot latch onto an unrendered extension.
                    let start_x = line.steps.first().map(|&(t, _)| x_of_time(t));
                    if let Some(start_x) = start_x {
                        if pos.0 + threshold < start_x {
                            continue;
                        }
                        // In `cross_only` mode, accept only the X band around the start cross.
                        if cross_only && (pos.0 - start_x).abs() > threshold {
                            continue;
                        }
                    } else if cross_only {
                        continue;
                    }
                    let rel_y = 0.5 - (price - center) / range;
                    let y = plot.y + rel_y * plot.h;
                    let dist = (y - pos.1).abs();
                    if dist <= threshold
                        && best.is_none_or(|(_, _, _, _, best_dist, _, _)| dist < best_dist)
                    {
                        best = Some((
                            order.uid,
                            kind,
                            price,
                            order.is_short,
                            dist,
                            start_x.unwrap_or(f32::NEG_INFINITY),
                            order.fill_pct,
                        ));
                    }
                }
            }
        }
        let (uid, kind, price, short, dist, start_x, fill_pct) = best?;
        // The unfilled entry's start cross is a cancel target using the same roughly seven-pixel
        // threshold. Only an unfilled Buy entry, including short, can be cancelled; a filled entry's
        // cross is historical.
        let on_start_cross = kind == LineKind::Buy
            && fill_pct <= 0.0
            && start_x.is_finite()
            && (pos.0 - start_x).abs() <= threshold
            && dist <= threshold;
        Some(OrderHit {
            core,
            uid,
            kind,
            pane,
            price,
            market,
            short,
            on_start_cross,
        })
    }

    /// Cancel an unfilled entry by left-clicking its start cross, matching Moonbot.
    ///
    /// The precise cross target remains active in the chart area under separate-zone mode, unlike
    /// dragging, which is restricted to the order book. Returns whether the click was consumed.
    pub(super) fn try_cancel_order_click(
        &mut self,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        // Use the hover gate in separate-zone chart space so only the start cross competes. A nearer
        // Sell line must not shadow a cross that was presented with the pointer cursor.
        let cross_only = self.separate_zones(cx) && self.glass_pane_at(pos).is_none();
        let Some(hit) = self.hit_order_line(pos, cross_only, cx) else {
            return false;
        };
        if !hit.on_start_cross {
            return false;
        }
        let (core, uid) = (hit.core, hit.uid);
        self.backend
            .update(cx, |b, _| match b.session.cancel_order(core, uid) {
                Ok(()) => log::info!("chart start-cross cancel: core={core} uid={uid}"),
                Err(error) => {
                    log::warn!("chart start-cross cancel failed: core={core} uid={uid}: {error}")
                }
            });
        true
    }

    /// Open the shared coin/order context menu for a right-clicked Buy or Sell line.
    ///
    /// The context supplies order UID, position direction, strategy, core, and market so the shared
    /// menu can expose its side-specific edit/cancel or join/split actions. Other line kinds have no
    /// coin menu. Returns whether the menu opened so the caller can suppress further right-click handling.
    pub(super) fn try_open_order_menu(
        &mut self,
        local_pos: (f32, f32),
        menu_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Open a menu only while order hover already marks an interactive line and has changed the
        // cursor. Elsewhere right-click retains its normal zoom or fullscreen-exit behavior.
        if self.order_hover.is_none() {
            return false;
        }
        let Some(hit) = self.hit_order_line(local_pos, false, cx) else {
            return false;
        };
        let (core, uid, market, short) = (hit.core, hit.uid, hit.market, hit.short);
        let side = match hit.kind {
            LineKind::Buy => crate::controls::OrderSide::Buy,
            LineKind::Sell => crate::controls::OrderSide::Sell,
            // Stops, trailing lines, and other kinds do not expose the coin/order menu.
            _ => return false,
        };
        // Read the strategy ID and the coin token from the core's open-order row; a zero strategy
        // denotes manual/join orders. The row's `coin` was resolved with this core's exchange
        // rules and is what the menu writes into the coin blacklists.
        let b = self.backend.read(cx);
        let order = b
            .session
            .store()
            .core(core)
            .and_then(|cd| cd.orders.iter().find(|o| o.uid == uid));
        let strat_id = order.map(|o| o.strat_id).filter(|id| *id != 0);
        let coin = order.map(|o| o.coin.clone());
        let strat_name = strat_id.and_then(|sid| {
            b.session
                .store()
                .core(core)
                .and_then(|cd| cd.strategies.iter().find(|s| s.id == sid))
                .map(|s| s.name.clone())
        });
        let core_name = b
            .session
            .sessions()
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        // A line whose order has already left the store asks the catalog directly: this token is
        // written into the core's coin blacklists, which it matches by exact text.
        let coin =
            coin.unwrap_or_else(|| b.session.market_source().market_label(core, &market).coin);
        let ctx = crate::controls::CoinMenuCtx {
            core,
            core_name,
            market,
            coin,
            selected_cores: vec![core],
            strat_id,
            strat_name,
            order_uid: Some(uid),
            side: Some(side),
            short,
            origin: crate::controls::CoinMenuOrigin::ChartLine,
        };
        crate::controls::open_coin_menu(ctx, self.backend.clone(), menu_pos, window, cx);
        cx.notify();
        true
    }

    /// Cancel the order under this panel's cursor for the built-in Tab/Delete route.
    ///
    /// `order_hover` identifies the hovered `(core, uid)`. Returns `false` when no order is hovered
    /// so the key can continue propagating, for example to Tab focus navigation.
    pub fn cancel_hovered_order(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(hover) = self.order_hover else {
            return false;
        };
        let (core, uid) = (hover.core, hover.uid);
        self.backend.update(cx, |b, _| {
            if let Err(error) = b.session.cancel_order(core, uid) {
                log::warn!("hotkey cancel hovered order failed: {error}");
            }
        });
        true
    }

    pub(super) fn set_order_interaction(
        &mut self,
        next: Option<OrderHoverKey>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.order_hover == next {
            return false;
        }
        self.order_hover = next;
        self.apply_order_visual(cx)
    }

    pub(super) fn apply_order_visual(&mut self, cx: &mut Context<Self>) -> bool {
        let highlight = self.order_hover.map(|hover| (hover.core, hover.uid));
        let drag_preview = self
            .order_drag
            .as_ref()
            .map(|drag| (drag.core, drag.uid, drag.kind, drag.current_price as f32))
            .or_else(|| {
                self.pending_order_drag
                    .map(|pending| (pending.core, pending.uid, pending.kind, pending.price))
            });
        if self.chart.set_order_visual(highlight, drag_preview) {
            self.sync_orders_if_visible(cx, true);
            true
        } else {
            false
        }
    }

    pub(super) fn clear_settled_order_drag_preview(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.pending_order_drag else {
            return false;
        };
        if pending.started.elapsed() >= ORDER_DRAG_PREVIEW_HOLD {
            self.pending_order_drag = None;
            return true;
        }

        let mut settled = false;
        {
            let b = self.backend.read(cx);
            if let Some(core_st) = b.session.store().core(pending.core) {
                match core_st.order_lines.order_state(pending.uid) {
                    Some(state) if state.active => {
                        if let Some(price) = core_st
                            .order_lines
                            .current_line_price(pending.uid, pending.kind)
                        {
                            let eps = pending.price.abs() * 1e-5 + 1e-8;
                            settled = (price - pending.price).abs() <= eps;
                        }
                    }
                    Some(_) | None => settled = true,
                }
            }
        }
        if settled {
            self.pending_order_drag = None;
        }
        settled
    }

    fn arm_order_drag_preview_timeout(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(ORDER_DRAG_PREVIEW_HOLD).await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    if this.clear_settled_order_drag_preview(cx) && this.apply_order_visual(cx) {
                        cx.notify();
                    }
                })
                .is_ok()
            });
        })
        .detach();
    }

    pub(super) fn sync_order_hover(&mut self, pos: (f32, f32), cx: &mut Context<Self>) -> bool {
        // Apply the Delphi threshold instead of hit-testing every raw mouse-move event.
        if !hover_probe_due(self.order_hover_probe, pos) {
            return false;
        }
        self.order_hover_probe = Some(pos);
        // In separate-zone mode, full line interaction belongs to the order book. In chart space,
        // use the reduced hit test for the click-to-cancel start cross only.
        let cross_only = self.separate_zones(cx) && self.glass_pane_at(pos).is_none();
        let next = self
            .hit_order_line(pos, cross_only, cx)
            .map(|hit| OrderHoverKey {
                core: hit.core,
                uid: hit.uid,
                cancel: hit.on_start_cross,
            });
        self.set_order_interaction(next, cx)
    }

    pub(super) fn try_start_order_drag(&mut self, pos: (f32, f32), cx: &mut Context<Self>) -> bool {
        // Separate-zone mode permits order-line dragging only inside the order book.
        if self.separate_zones(cx) && self.glass_pane_at(pos).is_none() {
            return false;
        }
        let Some(hit) = self.hit_order_line(pos, false, cx) else {
            return false;
        };
        // The start cross is handled as click-to-cancel before dragging in `mouse_down_left`. Never
        // start a drag there, or a timing miss could move the order instead of cancelling it.
        if hit.on_start_cross {
            return false;
        }
        let price = hit.price as f64;
        self.order_drag = Some(OrderDrag {
            core: hit.core,
            uid: hit.uid,
            kind: hit.kind,
            pane: hit.pane,
            start_price: price,
            current_price: price,
        });
        let visual_changed = self.set_order_interaction(
            Some(OrderHoverKey {
                core: hit.core,
                uid: hit.uid,
                cancel: false,
            }),
            cx,
        );
        if !visual_changed {
            self.apply_order_visual(cx);
        }
        true
    }

    pub(super) fn update_order_drag(&mut self, pos: (f32, f32), cx: &mut Context<Self>) -> bool {
        let Some((pane, price)) = self.order_drag.as_ref().and_then(|drag| {
            self.price_at_pane_y(drag.pane, pos.1)
                .map(|price| (drag.pane, price))
        }) else {
            return false;
        };
        let mut price_changed = false;
        if let Some(drag) = &mut self.order_drag {
            price_changed = (drag.current_price - price).abs() > 1e-9;
            drag.current_price = price;
        }
        if price_changed {
            self.apply_order_visual(cx);
        }
        self.input.cursor = Some(pos);
        self.input.hovered_pane = Some(pane);
        self.sync_native_cursor()
    }

    pub(super) fn finish_order_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.order_drag.take() else {
            return false;
        };
        let eps = drag.start_price.abs() * 1e-8 + 1e-8;
        if (drag.current_price - drag.start_price).abs() <= eps {
            self.apply_order_visual(cx);
            return true;
        }
        self.pending_order_drag = Some(PendingOrderDrag {
            core: drag.core,
            uid: drag.uid,
            kind: drag.kind,
            price: drag.current_price as f32,
            started: Instant::now(),
        });
        self.apply_order_visual(cx);
        self.arm_order_drag_preview_timeout(cx);
        // Match Moonbot drag routing:
        // - Buy/Sell replaces that order leg through `move_order`. The core performs cancel-and-new;
        //   a new crossing Sell limit in the order book executes at market. This avoids leaving the
        //   reserve-limit orphan that a separate `DoSellOrder` path would create.
        // - Stop/Trailing/TakeProfit uses `move_order_stop_price` with an absolute price.
        let sent = self.backend.update(cx, |b, _| {
            let price = drag.current_price;
            // Before moving a Sell line under panic sell, clear the panic flag for this order.
            // Otherwise the core's panic worker holds the price at the AllowedDrop floor and moves
            // it back. This matches Moonbot's manual "Stop Panic Sell" then line-drag sequence. Both
            // commands share the core queue, preserving order, while neighboring market orders stay
            // in panic mode because the flag is per-order.
            if drag.kind == LineKind::Sell
                && b.session
                    .store()
                    .core(drag.core)
                    .is_some_and(|d| d.order_lines.order_panic_sell(drag.uid))
            {
                match b.session.turn_order_panic_sell(drag.core, drag.uid, false) {
                    Ok(()) => log::info!(
                        "chart move sell line: dropping panic sell first, core={} uid={}",
                        drag.core,
                        drag.uid,
                    ),
                    Err(err) => log::warn!(
                        "chart move sell line: turn panic sell off failed, core={} uid={}: {err:#}",
                        drag.core,
                        drag.uid,
                    ),
                }
            }
            let result = match drag.kind {
                LineKind::Stop => b.session.move_order_stop_price(
                    drag.core,
                    drag.uid,
                    OrderLinePriceKind::StopLoss,
                    price,
                ),
                LineKind::Trailing => b.session.move_order_stop_price(
                    drag.core,
                    drag.uid,
                    OrderLinePriceKind::Trailing,
                    price,
                ),
                LineKind::TakeProfit => b.session.move_order_stop_price(
                    drag.core,
                    drag.uid,
                    OrderLinePriceKind::TakeProfit,
                    price,
                ),
                _ => b.session.move_order(drag.core, drag.uid, price),
            };
            match result {
                Ok(()) => {
                    log::info!(
                        "manual chart move line: core={} uid={} kind={:?} price={price:.8}",
                        drag.core,
                        drag.uid,
                        drag.kind,
                    );
                    true
                }
                Err(err) => {
                    log::warn!(
                        "manual chart move line failed: core={} uid={} kind={:?} price={price:.8}: {err:#}",
                        drag.core,
                        drag.uid,
                        drag.kind,
                    );
                    false
                }
            }
        });
        if !sent {
            self.pending_order_drag = None;
            self.apply_order_visual(cx);
        }
        sent
    }

    pub(super) fn sync_native_cursor(&mut self) -> bool {
        let cursor = self
            .input
            .cursor
            .and_then(|(x, y)| self.input.hovered_pane.map(|pane| (pane, x, y)));
        // In locked compare mode, publish the cursor price to peer charts in the tab. Each peer
        // renders a ghost horizontal line and its own volume/percentage through its Y mapping. This
        // bypasses GPUI notification because each peer schedules presentation when the price
        // changes; `None` clears the ghosts when the cursor leaves.
        if !self.ghost_peers.is_empty() {
            let price = cursor.and_then(|(pane, _x, y)| self.price_at_pane_y(pane, y));
            for peer in &self.ghost_peers {
                peer.set_price(price);
            }
        }
        self.chart.set_cursor(cursor)
    }
}

#[cfg(test)]
mod tests;
