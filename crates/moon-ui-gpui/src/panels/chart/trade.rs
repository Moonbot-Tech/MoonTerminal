//! Manual chart trading: order placement from configured mouse gestures or cursor hotkeys,
//! order-line hit testing and cancellation, hover/drag visuals, command routing, and native cursor
//! synchronization. Buy/Sell drags use `move_order`; stop and take-profit drags use
//! `move_order_stop_price`. This module was extracted from `chart.rs`.

use gpui::*;
use std::time::{Duration, Instant};

use moon_core::config::{MouseGestureBinding, MoveSide};
use moon_core::feed::OrderLinePriceKind;
use moon_core::session::CoreId;
use moon_core::session::order_lines::LineKind;

use super::ChartPanel;

mod pick;
use pick::OrderCandidate;

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
    /// Button that started the drag, so only ITS release commits the move. Every button has its
    /// own release handler, and a right or middle click during a left-button drag would otherwise
    /// send `move_order` at whatever intermediate price the line had reached.
    button: TradeMouseButton,
    start_price: f64,
    current_price: f64,
    /// Pointer Y the grab started at, and whether the drag may reprice yet.
    ///
    /// A grab on a PINNED line is the one case where the pointer starts nowhere near the price it
    /// holds: the line is drawn on the plot's edge while its order sits a long way outside the band.
    /// The drag maps the pointer absolutely — which is what the feature is for, you pull the line
    /// back onto the scale you are looking at — so without this the first pixel of jitter would
    /// rewrite the order from, say, +27% to the price at the edge of the visible window and the
    /// release would send it. Arming costs an intentional drag nothing and makes a twitch a no-op.
    /// It latches: once the drag is armed, wandering back under the threshold must not snap the
    /// price back to where it started. An ordinary grab starts armed, since there the pointer IS on
    /// the price.
    start_y: f32,
    armed: bool,
}

/// Pointer travel that arms a grab on a pinned line, in the hit test's own pixel units.
const ORDER_PIN_DRAG_ARM_PX: f32 = 4.0;

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
    /// Whether the grabbed line was actually MOVED to the plot's edge by the clamp, which decides
    /// whether the drag has to be armed before it may reprice anything. An exit still on screen is
    /// grabbed exactly as before — arming it would put a dead zone on an ordinary drag.
    pinned: bool,
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
        // Ctrl+Left needs no macOS special case HERE, but it did need one in the fork: Zed's mac
        // backend rewrote a Control+left press into a right click and erased the Control flag, so
        // the default Ctrl+Left move gesture could not fire on a Mac at all and the press fell
        // through to the fullscreen toggle instead. MoonUI now passes it through unless an
        // application opts into that convention (`gpui::set_macos_control_click_as_secondary`,
        // which `startup::run` explicitly leaves off), and the press arrives as Left carrying
        // Control exactly as on Windows.
        //
        // Matching a Ctrl+RIGHT press against a Ctrl+Left binding was tried as a workaround and
        // removed: this same matcher decides order PLACEMENT, where it would let one press satisfy
        // both the buy-set and short-set bindings and open the wrong side.
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

    /// Place an order when the press matches a configured buy-set or short-set gesture.
    ///
    /// `click_count` must be the count from this panel's own [`super::ClickSeries`], never the
    /// window's native one: the native count pairs presses by time and distance across the whole
    /// window, so a press arriving here right after a chart closed elsewhere carries a two.
    pub(super) fn try_place_order_click(
        &mut self,
        button: TradeMouseButton,
        modifiers: Modifiers,
        click_count: usize,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        // A HISTORICAL VIEWER places no orders, and this is the FIRST of the nine order entry
        // points that says so. The trade-detail window draws a market that already happened, so a
        // gesture here would send a LIVE command at a price read off an old chart.
        //
        // Refused at the entry point rather than deeper down, and by returning "I did not claim
        // this press" rather than by swallowing it: pan, zoom and the crosshair go on working
        // normally in that window, which is what the user wants there. The other eight guards
        // carry a one-line reference to this explanation instead of a ninth copy of it.
        if self.historical {
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

    /// Send Moonbot's Move Open / Move TP for a press that matches one of the four move gestures.
    ///
    /// In Moonbot these gestures are a CLICK, not a drag: the press names a destination price and
    /// the core moves the addressed orders onto it, laid out by the slot's "Move kind" — parallel
    /// to the nearest line, all onto one price, top volume first, and so on. Nothing is computed
    /// here and no line has to be under the pointer, which is the whole difference from the plain
    /// left drag that grabs one line and keeps working exactly as before.
    ///
    /// The price and the market come from the pane under the pointer, exactly as a placement click
    /// takes them, so separate-zone mode restricts this to the order book on the same terms.
    ///
    /// Args:
    ///     button: Physical button of the press.
    ///     modifiers: Modifier state of the press.
    ///     click_count: This panel's own click count, never the window's.
    ///     pos: Chart-local pointer position.
    ///     cx: Panel context, used to read the bindings and to send the command.
    ///
    /// Returns:
    ///     Whether a gesture claimed the press. A claimed press sends nothing further, including
    ///     when the market or the price could not be resolved: the user asked for a bulk move, and
    ///     falling through to pan or zoom would answer a trading gesture with a chart movement.
    pub(super) fn try_move_orders_click(
        &mut self,
        button: TradeMouseButton,
        modifiers: Modifiers,
        click_count: usize,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        // Historical viewer: no orders. Rationale at `try_place_order_click`.
        if self.historical {
            return false;
        }
        let command = {
            let b = self.backend.read(cx);
            let cfg = b.preview.as_ref().unwrap_or(&b.config);
            cfg.hotkeys.resolve_move_gesture(|binding| {
                Self::gesture_matches(binding, button, modifiers, click_count)
            })
        };
        let Some(command) = command else {
            return false;
        };
        // In separate-zone mode the book is the trading surface, as it is for placement.
        let pane = if self.separate_zones(cx) {
            self.glass_pane_at(pos)
        } else {
            self.input.pane_at(pos.0, pos.1)
        };
        let Some(pane) = pane else {
            return false;
        };
        let Some(price) = self.price_at_pane_y(pane, pos.1) else {
            return true;
        };
        let Some((core, market)) = self
            .chart
            .with_container(|container| container.target(pane))
        else {
            return true;
        };
        let workspace_group = self.workspace_group.clone();
        self.backend.update(cx, |b, _| {
            // Logged rather than dropped, like every other refusal on a path that moves live money:
            // a window that may not trade this core looks exactly like a gesture that missed.
            if !b.workspace_action_allows_core(workspace_group.as_deref(), core) {
                log::warn!(
                    "move orders to price: core={} market={market} is outside this window's \
                     workspace, nothing sent",
                    moon_core::feed::core_label(core)
                );
                return;
            }
            // A press that claimed both sides says nothing about which one was meant — the shipped
            // `same_hotkeys_for_move` puts one gesture on both. Narrow it to the side actually open
            // on this market, exactly as the sells-to-zone command does, or a hedged market would
            // have the other position's orders repriced by a click aimed at this one.
            let side = match command.side {
                MoveSide::Both if b.market_position_short(core, &market) => MoveSide::Short,
                MoveSide::Both => MoveSide::Long,
                side => side,
            };
            match b.session.move_orders_to_price(
                core,
                market.clone(),
                command.sell,
                command.kind,
                price,
                side,
            ) {
                Ok(()) => log::info!(
                    "move orders to price: core={} market={market} {} -> {price:.8} kind={:?} side={side:?}",
                    moon_core::feed::core_label(core),
                    if command.sell { "sells" } else { "buys" },
                    command.kind,
                ),
                Err(error) => log::warn!(
                    "move orders to price failed: core={} market={market} {error}",
                    moon_core::feed::core_label(core)
                ),
            }
        });
        true
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
        // Historical viewer: no orders. Rationale at `try_place_order_click`.
        if self.historical {
            log::debug!("manual order refused: historical viewer places no orders");
            return false;
        }
        match self.input.cursor {
            Some(pos) => self.place_order_at_pos(pos, short, cx),
            None => {
                log::debug!(
                    "manual order refused: this chart holds no cursor position, so the hotkey has \
                     no price to place at"
                );
                false
            }
        }
    }

    /// Place a manual order at slot-pixel position `pos`, using `short` to select its position side.
    ///
    /// `false` selects Long and `true` selects Short. This shared mouse/hotkey path resolves pane,
    /// price, and `(core, market)`, then converts the core group's visible USD-equivalent size.
    fn place_order_at_pos(&mut self, pos: (f32, f32), short: bool, cx: &mut Context<Self>) -> bool {
        // In separate-zone mode place only from the order-book zone; otherwise accept any pane area.
        let separate = self.separate_zones(cx);
        let pane = if separate {
            self.glass_pane_at(pos)
        } else {
            self.input.pane_at(pos.0, pos.1)
        };
        let Some(pane) = pane else {
            // The likeliest refusal of the lot, and the one a user cannot see: with separate zones
            // the pointer has to sit inside the order-book strip on the right, which a HOTKEY gives
            // no reason to expect — nothing was clicked. The zone's own bounds go in the line, or
            // the reader is left holding a coordinate and no idea how far off it was.
            let zone = self
                .input
                .pane_at(pos.0, pos.1)
                .and_then(|pane| self.control_zone_rect(pane));
            log::debug!(
                "manual order refused: ({:.1}, {:.1}) is outside {}",
                pos.0,
                pos.1,
                match (separate, zone) {
                    (true, Some(z)) => format!(
                        "the order-book zone x={:.0}..{:.0}, y={:.0}..{:.0} (separate control \
                         zones are on)",
                        z.x,
                        z.x + z.w,
                        z.y,
                        z.y + z.h
                    ),
                    // Either no pane holds the pointer, or the one that does reports no control
                    // zone at all — a book enabled but measured to zero width. Stated as the one
                    // fact both share: there was no zone to be inside of.
                    (true, None) =>
                        "the order-book zone, which resolved to nothing here".to_string(),
                    (false, _) => "every pane".to_string(),
                }
            );
            return false;
        };
        let Some(price) = self.price_at_pane_y(pane, pos.1) else {
            log::debug!(
                "manual order refused: pane {pane} maps y={:.1} to no finite positive price",
                pos.1
            );
            return false;
        };
        let Some((core, market)) = self
            .chart
            .with_container(|container| container.target(pane))
        else {
            log::debug!("manual order refused: pane {pane} carries no core and market");
            return false;
        };

        let workspace_group = self.workspace_group.clone();
        let placed = self.backend.update(cx, |b, _| {
            // Warn, not debug, and for the reason the identical guard on `move_orders_to_price`
            // states: a window that may not trade this core looks exactly like a gesture that
            // missed. The other refusals here really are "you aimed wrong" and stay on the channel.
            if !b.workspace_action_allows_core(workspace_group.as_deref(), core) {
                log::warn!(
                    "manual order refused: core={} market={market} is outside this window's \
                     workspace, nothing sent",
                    moon_core::feed::core_label(core)
                );
                return false;
            }
            let Some(terms) = b.manual_order_terms(core, &market, price, None) else {
                // The reason itself is stated by whichever guard refused, on the line immediately
                // above this one. Listing every possible cause here as well only competes with it.
                log::warn!(
                    "manual chart order blocked: core={} market={market} — see the refusal above",
                    moon_core::feed::core_label(core)
                );
                return false;
            };
            let Some(usd) = terms.size_usd else {
                log::debug!(
                    "manual order refused: core={} market={market} has no visible USD-equivalent \
                     order size",
                    moon_core::feed::core_label(core)
                );
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
                    // The pointer position is in the SUCCESS line too, so a log holding both
                    // outcomes shows where the accepting zone actually starts — a refusal alone
                    // gives one side of the boundary and leaves the other to guesswork.
                    log::info!(
                        "manual chart order: core={} market={market} side={} price={price:.8} size={} usd={usd} at ({:.1}, {:.1})",
                        moon_core::feed::core_label(core),
                        if short { "short" } else { "long" },
                        terms.size_base,
                        pos.0,
                        pos.1
                    );
                    true
                }
                Err(err) => {
                    log::warn!(
                        "manual chart order failed: core={} market={market} price={price:.8}: {err:#}",
                        moon_core::feed::core_label(core)
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
        let mut best: Option<OrderCandidate> = None;
        if let Some(core_data) = self.backend.read(cx).session.store().core(core) {
            // Iterated, not collected: this runs on every mouse-move past the probe threshold, and
            // the ranking below ends on a unique `seq`, so the winner does not depend on the order
            // the store hands them over in. What the arbitrary `HashMap` order DOES decide is which
            // pinned line is painted on top, and that is settled where the painting happens, in
            // `build_order_geometry`.
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
                    let mut y = plot.y + rel_y * plot.h;
                    // A pinned exit is drawn on the plot's nearer edge, so that is where the pointer
                    // must find it. Eligibility is `line_is_pinned`'s to say — the geometry that drew
                    // it and the labels that caption it ask the same function about the same order —
                    // while whether it actually MOVED is the pin's own answer: an exit whose price is
                    // on screen is an ordinary line and keeps every ordinary behaviour.
                    let pin = moon_chart::order_geometry::line_is_pinned(order, kind)
                        .then(|| moon_chart::order_geometry::pin_line_y(y, plot.y, plot.h))
                        .flatten();
                    if let Some(pin) = pin {
                        y = pin.y;
                        // The tolerance below reaches PAST the plot, and under its bottom edge lies
                        // the time axis. Without this a press meant for the axis would grab an exit
                        // whose price is nowhere near the pointer — and a pinned grab reprices to
                        // wherever it is dropped, so a stray one is not a cosmetic mistake.
                        if pos.1 < plot.y || pos.1 > plot.y + plot.h {
                            continue;
                        }
                    }
                    let dist = (y - pos.1).abs();
                    if dist > threshold {
                        continue;
                    }
                    let candidate = OrderCandidate {
                        uid: order.uid,
                        kind,
                        price,
                        short: order.is_short,
                        dist,
                        start_x: start_x.unwrap_or(f32::NEG_INFINITY),
                        fill_pct: order.fill_pct,
                        // How far past the plot the line's own Y sat — zero while it is on screen.
                        // The same measure the label column ranks pinned captions by, taken from the
                        // same call, rather than a second derivation in price units to keep in sync.
                        overshoot: pin.map_or(0.0, |pin| pin.overshoot),
                        size: order.exit_size(),
                        seq: order.seq,
                        pinned: pin.is_some(),
                    };
                    if best.as_ref().is_none_or(|best| candidate.beats(best)) {
                        best = Some(candidate);
                    }
                }
            }
        }
        let OrderCandidate {
            uid,
            kind,
            price,
            short,
            dist,
            start_x,
            fill_pct,
            pinned,
            ..
        } = best?;
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
            pinned,
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
        // Historical viewer: no orders. Rationale at `try_place_order_click`.
        if self.historical {
            return false;
        }
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
        let workspace_group = self.workspace_group.clone();
        self.backend.update(cx, |b, _| {
            if !b.workspace_action_allows_core(workspace_group.as_deref(), core) {
                return;
            }
            match b.session.cancel_order(core, uid) {
                Ok(()) => log::info!(
                    "chart start-cross cancel: core={} uid={uid}",
                    moon_core::feed::core_label(core)
                ),
                Err(error) => {
                    log::warn!(
                        "chart start-cross cancel failed: core={} uid={uid}: {error}",
                        moon_core::feed::core_label(core)
                    )
                }
            }
        });
        true
    }

    /// Open the shared coin/order context menu for a right-clicked Buy or Sell line.
    ///
    /// The context supplies order UID, position direction, strategy, core, and market so the shared
    /// menu can expose its side-specific edit/cancel or join/split actions. Other line kinds have no
    /// coin menu.
    ///
    /// Args:
    ///     local_pos: Chart-local hit-test position.
    ///     menu_pos: Window-coordinate popup position.
    ///     window: Chart window that owns the context menu.
    ///     cx: Panel context used to resolve the live order row.
    ///
    /// Returns:
    ///     Whether a menu opened, allowing the caller to suppress further right-click handling.
    pub(super) fn try_open_order_menu(
        &mut self,
        local_pos: (f32, f32),
        menu_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Historical viewer: no orders. Rationale at `try_place_order_click`.
        if self.historical {
            return false;
        }
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
        if !self.workspace_action_allowed(&b, core) {
            return false;
        }
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
            workspace_group: self.workspace_group.clone(),
            side: Some(side),
            short,
            origin: crate::controls::CoinMenuOrigin::ChartLine,
            history: None,
            trailing: Vec::new(),
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
        // Historical viewer: no orders. Rationale at `try_place_order_click`.
        if self.historical {
            return false;
        }
        let Some(hover) = self.order_hover else {
            return false;
        };
        let (core, uid) = (hover.core, hover.uid);
        let workspace_group = self.workspace_group.clone();
        self.backend.update(cx, |b, _| {
            if !b.workspace_action_allows_core(workspace_group.as_deref(), core) {
                return;
            }
            if let Err(error) = b.session.cancel_order(core, uid) {
                log::warn!("hotkey cancel hovered order failed: {error}");
            }
        });
        true
    }

    /// Spread this chart's sells across a band named on it, for both ways of naming one: the
    /// band drawn in Ctrl+S mode and the right-click entry on a Zone or Rect.
    ///
    /// `a` and `z` are the band's two prices. The authority check is the one its trading siblings
    /// make — this panel may be showing a core the group's Auto rail no longer trades, and this is a
    /// live bulk move. Every way this can end without a command is logged, because by the time it
    /// runs the band the user drew is already gone from the screen.
    pub(super) fn send_sells_to_zone(
        &mut self,
        core: CoreId,
        market: &str,
        a: f64,
        z: f64,
        cx: &mut Context<Self>,
    ) {
        // Historical viewer: no orders. Rationale at `try_place_order_click`.
        if self.historical {
            return;
        }
        if !self.workspace_action_allowed(self.backend.read(cx), core) {
            log::warn!(
                "sells to zone: core={} market={market} is not authorized for this workspace group, nothing sent",
                moon_core::feed::core_label(core)
            );
            return;
        }
        self.backend.update(cx, |b, _| {
            crate::hotkeys::sells_to_zone(b, core, market, a, z)
        });
    }

    /// Split the order under this panel's cursor into `parts` for the Split Order hotkeys.
    ///
    /// The hotkey has no click of its own, so it addresses what the pointer addresses, exactly as
    /// the built-in Tab/Delete cancellation does. Only an order that HAS a live sell leg qualifies:
    /// the core splits the sell, and the pointer can equally rest on an entry, stop or trailing line
    /// of an order that has none. Anything else returns `false`, leaving the caller its market-level
    /// fallback rather than sending a command the core would discard.
    pub fn split_hovered_order(&mut self, parts: i32, cx: &mut Context<Self>) -> bool {
        // Historical viewer: no orders. Rationale at `try_place_order_click`.
        if self.historical {
            return false;
        }
        let Some(hover) = self.order_hover else {
            return false;
        };
        let (core, uid) = (hover.core, hover.uid);
        {
            let b = self.backend.read(cx);
            let Some(lines) = b.session.store().core(core).map(|data| &data.order_lines) else {
                return false;
            };
            let active = lines.order_state(uid).is_some_and(|state| state.active);
            let has_sell = lines
                .current_line_price(uid, LineKind::Sell)
                .is_some_and(|price| price.is_finite() && price > 0.0);
            if !(active && has_sell) {
                return false;
            }
        }
        let workspace_group = self.workspace_group.clone();
        self.backend.update(cx, |b, _| {
            if b.workspace_action_allows_core(workspace_group.as_deref(), core)
                && let Err(error) = b.session.split_order(core, uid, parts)
            {
                log::warn!("hotkey split hovered order failed: {error}");
            }
        });
        // Handled either way once a target was found. A refusal or a failed send must NOT report
        // "nothing hovered": the caller's fallback would then split a different order — the one on
        // its Main chart — which is the opposite of doing nothing.
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

    /// Start dragging an order line when the press is allowed to grab it.
    ///
    /// Two ways in, and the configured gestures only ADD to what already worked:
    /// - the built-in single LEFT press, whatever modifiers it carries, exactly as before this
    ///   became configurable — a config with no usable move gesture never leaves a line immovable;
    /// - a configured Moonbot move gesture for that line's own side and direction, which is the
    ///   only route for the middle and right buttons and for the double-click bindings.
    ///
    /// `native_single` is the window's own "this is not the second press of a pair" answer, kept
    /// separate from `click_count`: the built-in grab requires both, while a double-click gesture
    /// deliberately wants the pair-second press this panel counted.
    pub(super) fn try_start_order_drag(
        &mut self,
        button: TradeMouseButton,
        click_count: usize,
        native_single: bool,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        // Historical viewer: no orders. Rationale at `try_place_order_click`.
        if self.historical {
            return false;
        }
        // A live drag owns the line until ITS button is released. A second button pressed mid-drag
        // would otherwise replace the drag, and the first button's release would then find a drag
        // it does not own, drop the move on the floor and spring the line back.
        if self.order_drag.is_some() {
            return false;
        }
        // Dragging is the plain single left press and nothing else. The configured gestures are
        // Moonbot's bulk move-to-price (`try_move_orders_click`) and are answered before this: one
        // press that grabbed a line when it landed on one and moved the whole side when it did not
        // would be two different trades behind the same hand movement.
        if button != TradeMouseButton::Left || click_count > 1 || !native_single {
            return false;
        }
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
        if !self.workspace_action_allowed(&self.backend.read(cx), hit.core) {
            return false;
        }
        let price = hit.price as f64;
        self.order_drag = Some(OrderDrag {
            core: hit.core,
            uid: hit.uid,
            kind: hit.kind,
            pane: hit.pane,
            button,
            start_price: price,
            current_price: price,
            start_y: pos.1,
            armed: !hit.pinned,
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
        let arm_px = (ORDER_PIN_DRAG_ARM_PX * self.last_ppp).max(ORDER_PIN_DRAG_ARM_PX);
        let mut price_changed = false;
        if let Some(drag) = &mut self.order_drag {
            if !drag.armed && (pos.1 - drag.start_y).abs() >= arm_px {
                drag.armed = true;
            }
            // Until it arms, a grab on a pinned line holds the order's real price: the preview must
            // not show a move the release would not send either.
            let next = if drag.armed { price } else { drag.start_price };
            price_changed = (drag.current_price - next).abs() > 1e-9;
            drag.current_price = next;
        }
        if price_changed {
            self.apply_order_visual(cx);
        }
        self.input.cursor = Some(pos);
        self.input.hovered_pane = Some(pane);
        self.sync_native_cursor(cx)
    }

    /// Finish a drag on the release of the button that started it.
    ///
    /// A release from any OTHER button leaves the drag live and returns `false`, so its handler
    /// keeps its normal behavior instead of committing someone else's gesture.
    pub(super) fn finish_order_drag(
        &mut self,
        button: TradeMouseButton,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.order_drag.as_ref().is_some_and(|d| d.button != button) {
            return false;
        }
        let Some(drag) = self.order_drag.take() else {
            return false;
        };
        let eps = drag.start_price.abs() * 1e-8 + 1e-8;
        if (drag.current_price - drag.start_price).abs() <= eps {
            self.apply_order_visual(cx);
            return true;
        }
        // Match Moonbot drag routing:
        // - Buy/Sell replaces that order leg through `move_order`. The core performs cancel-and-new;
        //   a new crossing Sell limit in the order book executes at market. This avoids leaving the
        //   reserve-limit orphan that a separate `DoSellOrder` path would create.
        // - Stop/Trailing/TakeProfit uses `move_order_stop_price` with an absolute price.
        let workspace_group = self.workspace_group.clone();
        let sent = self.backend.update(cx, |b, _| {
            if !b.workspace_action_allows_core(workspace_group.as_deref(), drag.core) {
                return false;
            }
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
        if sent {
            self.pending_order_drag = Some(PendingOrderDrag {
                core: drag.core,
                uid: drag.uid,
                kind: drag.kind,
                price: drag.current_price as f32,
                started: Instant::now(),
            });
            self.apply_order_visual(cx);
            self.arm_order_drag_preview_timeout(cx);
        } else {
            self.pending_order_drag = None;
            self.apply_order_visual(cx);
        }
        sent
    }

    pub(super) fn sync_native_cursor(&mut self, cx: &App) -> bool {
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
        let moved = self.chart.set_cursor(cursor);
        // A measuring caption reads the POINTER, not the market, so this is the only path that can
        // refresh it — a still market would otherwise leave it stating the moment the mouse left.
        // Costs one comparison on a chart that carries no such caption.
        let source = self.backend.read(cx).session.market_source();
        let measured = self.chart.sync_cursor_volumes(&source);
        moved || measured
    }
}

#[cfg(test)]
mod tests;
