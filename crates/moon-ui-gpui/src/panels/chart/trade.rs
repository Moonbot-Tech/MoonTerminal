//! Ручная торговля на чарте: постановка ордера кликом по жесту, хит-тест линий ордеров,
//! подсветка/перетаскивание линий (move_order) и нативный курсор. Вынесено из `chart.rs`.

use std::time::Duration;

use gpui::*;

use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::config::MouseGestureBinding;
use moon_core::feed::OrderLinePriceKind;
use moon_core::session::CoreId;
use moon_core::session::order_lines::LineKind;

use super::ChartPanel;

/// На сколько частей делит «Split order» (ПКМ по линии sell). MoonBot по умолчанию — на 2.
const SPLIT_PARTS: i32 = 2;

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
    /// Рынок ордера (для join/split — они per-market в moonproto).
    market: String,
    /// Сторона позиции ордера (для выбора `OrderSide` в join).
    short: bool,
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
        // ДИАГ (env MOON_ORDER_LINE_DIAG=1): вид В МОМЕНТ КЛИКА (render_center/range) и куда по
        // экрану ложится цена (rel_y). Сравнив с видом на кадре отрисовки, поймём, на сколько
        // уехал Y-масштаб (симптом «линия выше клика»).
        if std::env::var_os("MOON_ORDER_LINE_DIAG").is_some() {
            if let Some((c, r)) = self.chart.with_container(|cont| {
                cont.pane(pane)
                    .map(|p| (p.view.render_center, p.view.render_range))
            }) {
                let rel = if r > 0.0 { 0.5 - (price as f32 - c) / r } else { f32::NAN };
                let cy = pos.1;
                log::info!(
                    "order-click-diag pane={pane} click_y={cy:.1} price={price:.4} render_center={c:.4} render_range={r:.4} rel_y={rel:.3}"
                );
            }
        }
        let Some((core, market)) = self
            .chart
            .with_container(|container| container.target(pane))
        else {
            return false;
        };
        // ДИАГ (env MOON_ORDER_LINE_DIAG=1): «смотрю одно — торгую другое» под dedup. Логируем
        // ЯДРО-ЦЕЛЬ ордера, отправляемую цену и bid/ask ОТОБРАЖАЕМОГО стакана (он resolved через
        // core_provider — может быть ДРУГОЕ ядро/биржа). Если sent_price выше отображаемого ask —
        // клик был выше рынка; если отображаемый ask ≠ реальному аску ядра в логе бота — рассинхрон
        // ядра данных и ядра ордера.
        if std::env::var_os("MOON_ORDER_LINE_DIAG").is_some() {
            let book = self
                .backend
                .read(cx)
                .session
                .market_source()
                .with_orderbook_view(core, &market, |d| {
                    d.and_then(|(book, _)| book.best_bid_ask())
                });
            log::info!(
                "order-send-diag order_core={core} market={market} sent_price={price:.4} displayed_book_bid_ask={book:?}"
            );
        }

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
        let mut best: Option<(u64, LineKind, f32, bool, f32)> = None;
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
                    // Вход (Buy, в т.ч. шорт) переставляем ТОЛЬКО пока ордер не исполнен (живой
                    // входной лимит → move_order = replace). После фила (`fill_pct > 0`) линия
                    // входа — историческая отметка цены входа: реплейсить нечего, не тянем её.
                    // Управление залитой позицией идёт через выход (Sell) и стопы.
                    if kind == LineKind::Buy && order.fill_pct > 0.0 {
                        continue;
                    }
                    let Some(price) = order.lines[kind as usize]
                        .current_price()
                        .filter(|p| p.is_finite() && *p > 0.0)
                    else {
                        continue;
                    };
                    let rel_y = 0.5 - (price - center) / range;
                    let y = plot.y + rel_y * plot.h;
                    let dist = (y - pos.1).abs();
                    if dist <= threshold
                        && best.is_none_or(|(_, _, _, _, best_dist)| dist < best_dist)
                    {
                        best = Some((order.uid, kind, price, order.is_short, dist));
                    }
                }
            }
        }
        let (uid, kind, price, short, _) = best?;
        Some(OrderHit {
            core,
            uid,
            kind,
            pane,
            price,
            market,
            short,
        })
    }

    /// ПКМ по линии ордера → контекстное меню. Buy/Buy short → «Cancel» (этого ордера).
    /// Sell/Sell short → «Join all sells» + «Split order» (per-market в moonproto).
    /// Возвращает `true`, если меню открыто (вызывающий гасит дальнейшую обработку ПКМ).
    pub(super) fn try_open_order_menu(
        &mut self,
        local_pos: (f32, f32),
        menu_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Меню — ТОЛЬКО когда курсор уже в состоянии перетаскивания линии (наведён на линию
        // ордера, `order_hover` активен и курсор сменился). В прочих позициях ПКМ работает
        // как обычно (зум / возврат из фулскрина) — меню не зовём.
        if self.order_hover.is_none() {
            return false;
        }
        let Some(hit) = self.hit_order_line(local_pos, cx) else {
            return false;
        };
        let backend = self.backend.clone();
        let (core, uid, market, short) = (hit.core, hit.uid, hit.market, hit.short);
        let items: Vec<MoonMenuItem> = match hit.kind {
            LineKind::Buy => vec![
                MoonMenuItem::with_key("order-cancel", t!("chart.order_menu.cancel").to_string())
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend.update(app, |b, _| {
                            let _ = b.session.cancel_order(core, uid);
                        });
                    }),
            ],
            LineKind::Sell => {
                let backend_split = backend.clone();
                let market_split = market.clone();
                vec![
                    MoonMenuItem::with_key(
                        "order-join-sells",
                        t!("chart.order_menu.join_sells").to_string(),
                    )
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend.update(app, |b, _| {
                            let _ = b.session.join_sells(core, market.clone(), short);
                        });
                    }),
                    MoonMenuItem::with_key(
                        "order-split",
                        t!("chart.order_menu.split").to_string(),
                    )
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_split.update(app, |b, _| {
                            let _ = b.session.split_order(core, market_split.clone(), SPLIT_PARTS);
                        });
                    }),
                ]
            }
            _ => return false,
        };
        window.open_moon_context_menu(cx, "chart-order-menu", menu_pos, items, 170.0);
        cx.notify();
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
