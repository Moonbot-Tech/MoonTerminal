//! Ручная торговля на чарте: постановка ордера кликом по жесту, хит-тест линий ордеров,
//! подсветка/перетаскивание линий (move_order) и нативный курсор. Вынесено из `chart.rs`.

use std::time::Duration;

use gpui::*;

use moon_core::config::MouseGestureBinding;
use moon_core::feed::OrderLinePriceKind;
use moon_core::session::CoreId;
use moon_core::session::order_lines::LineKind;

use super::ChartPanel;

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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OrderHoverKey {
    core: CoreId,
    uid: u64,
}

struct OrderHit {
    core: CoreId,
    uid: u64,
    kind: LineKind,
    pane: usize,
    price: f32,
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
        // Дебаунс после закрытия графика (×): не ставим ордер ~600мс, иначе быстрый второй
        // клик «закрыть» попадает на стакан уехавшего графика и засчитывается как дабл-клик.
        if self
            .last_pane_close
            .is_some_and(|t| t.elapsed() < Duration::from_millis(600))
        {
            return false;
        }
        // Раздельные зоны: ордер ставим только в стакане; иначе — по любой pane-области графика.
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
            let cfg = b.preview.as_ref().unwrap_or(&b.config);
            let short = if Self::gesture_matches(
                cfg.hotkeys.buy_set_click,
                button,
                modifiers,
                click_count,
            ) {
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
            };
            let Some(short) = short else {
                return false;
            };
            let size = b.manual_order_size(core);
            match b
                .session
                .place_order(core, market.clone(), short, price, size, None)
            {
                Ok(()) => {
                    log::info!(
                        "manual chart order: core={core} market={market} side={} price={price:.8} size={size}",
                        if short { "short" } else { "long" }
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
        // Авто-пин при выставлении ордера (per-окно/вкладка): успешный ордер закрепляет этот
        // график, чтобы он не закрылся по TTL/неактивности, пока пользователь в позиции.
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

    fn hit_order_line(&self, pos: (f32, f32), cx: &mut Context<Self>) -> Option<OrderHit> {
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
        let Some((center, range)) = self.chart.with_container(|container| {
            container
                .pane(pane)
                .map(|pane| (pane.view.render_center, pane.view.render_range))
        }) else {
            return None;
        };
        if plot.h <= 1.0 || !(range > 0.0) {
            return None;
        }
        let threshold = (6.0 * self.last_ppp).max(6.0);
        let mut best: Option<(u64, LineKind, f32, f32)> = None;
        if let Some(core_data) = self.backend.read(cx).session.store().core(core) {
            for order in core_data
                .order_lines
                .market_draw_orders(&market, 0)
                .into_iter()
                .filter(|order| order.closed_ms.is_none())
            {
                // Перетаскиваемые виды линий: вход/выход (move_order) + SL/Trailing/TakeProfit
                // (update_stops по абсолютной цене). VStop (объёмный) и pending-условие НЕ тянем
                // — у них нет ценового уровня, который ставится перетаскиванием.
                for kind in [
                    LineKind::Buy,
                    LineKind::Sell,
                    LineKind::Stop,
                    LineKind::Trailing,
                    LineKind::TakeProfit,
                ] {
                    let Some(price) = order.lines[kind as usize]
                        .current_price()
                        .filter(|p| p.is_finite() && *p > 0.0)
                    else {
                        continue;
                    };
                    let rel_y = 0.5 - (price - center) / range;
                    let y = plot.y + rel_y * plot.h;
                    let dist = (y - pos.1).abs();
                    if dist <= threshold && best.is_none_or(|(_, _, _, best_dist)| dist < best_dist)
                    {
                        best = Some((order.uid, kind, price, dist));
                    }
                }
            }
        }
        let (uid, kind, price, _) = best?;
        Some(OrderHit {
            core,
            uid,
            kind,
            pane,
            price,
        })
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
            .map(|drag| (drag.core, drag.uid, drag.kind, drag.current_price as f32));
        if self.chart.set_order_visual(highlight, drag_preview) {
            self.sync_orders_if_visible(cx, true);
            true
        } else {
            false
        }
    }

    pub(super) fn sync_order_hover(&mut self, pos: (f32, f32), cx: &mut Context<Self>) -> bool {
        // Раздельные зоны: за линии цепляемся только в стакане → и подсветку даём только там.
        if self.separate_zones(cx) && self.glass_pane_at(pos).is_none() {
            return self.set_order_interaction(None, cx);
        }
        let next = self.hit_order_line(pos, cx).map(|hit| OrderHoverKey {
            core: hit.core,
            uid: hit.uid,
        });
        self.set_order_interaction(next, cx)
    }

    pub(super) fn try_start_order_drag(&mut self, pos: (f32, f32), cx: &mut Context<Self>) -> bool {
        // Раздельные зоны: тянуть линию ордера можно только в зоне стакана.
        if self.separate_zones(cx) && self.glass_pane_at(pos).is_none() {
            return false;
        }
        let Some(hit) = self.hit_order_line(pos, cx) else {
            return false;
        };
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
        self.apply_order_visual(cx);
        let eps = drag.start_price.abs() * 1e-8 + 1e-8;
        if (drag.current_price - drag.start_price).abs() <= eps {
            return true;
        }
        // Вход/выход (Buy/Sell) переставляются как ордер (move_order). Стоп-линии
        // (Stop/Trailing/TakeProfit) — задают цену стопа/тейка через update_stops
        // (SL/Trailing → ФИКСИРОВАННЫЙ стоп по цене). Другие виды до drag не доходят
        // (хит-тест их не ловит), но на всякий случай трактуем как move_order.
        let stop_kind = match drag.kind {
            LineKind::Stop => Some(OrderLinePriceKind::StopLoss),
            LineKind::Trailing => Some(OrderLinePriceKind::Trailing),
            LineKind::TakeProfit => Some(OrderLinePriceKind::TakeProfit),
            _ => None,
        };
        self.backend.update(cx, |b, _| {
            let result = match stop_kind {
                Some(kind) => {
                    b.session
                        .move_order_stop_price(drag.core, drag.uid, kind, drag.current_price)
                }
                None => b.session.move_order(drag.core, drag.uid, drag.current_price),
            };
            match result {
                Ok(()) => {
                    log::info!(
                        "manual chart move line: core={} uid={} kind={:?} price={:.8}",
                        drag.core,
                        drag.uid,
                        stop_kind,
                        drag.current_price
                    );
                    true
                }
                Err(err) => {
                    log::warn!(
                        "manual chart move line failed: core={} uid={} kind={:?} price={:.8}: {err:#}",
                        drag.core,
                        drag.uid,
                        stop_kind,
                        drag.current_price
                    );
                    false
                }
            }
        })
    }

    pub(super) fn sync_native_cursor(&mut self) -> bool {
        let cursor = self
            .input
            .cursor
            .and_then(|(x, y)| self.input.hovered_pane.map(|pane| (pane, x, y)));
        self.chart.set_cursor(cursor)
    }
}
