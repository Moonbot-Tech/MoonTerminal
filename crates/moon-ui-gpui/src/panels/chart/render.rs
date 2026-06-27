//! `impl Render for ChartPanel` — own-pass canvas под сценой + слой ввода (колесо/кнопки/
//! движение мыши/ховер) + GPUI-оверлеи (логотип пустого слота, FireTest-probe, риска зоны
//! управления, кнопки ✕/пин). Вынесено из `chart.rs` без изменения поведения.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonRect, rgba_from};

use moon_chart::paint::now_unix_ms;

use crate::{axes, input};

use super::trade::TradeMouseButton;
use super::{ChartPanel, chart_bootstrap_present_rate_hz};

fn rgb3_from_hex(hex: u32) -> [u8; 3] {
    [
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    ]
}

impl Render for ChartPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::CHART_RENDER);
        let became_visible = !self.scene_visible;
        self.scene_visible = true;
        self.chart.set_scene_visible(true);
        self.chart
            .set_market_source(Some(self.backend.read(cx).session.market_source()));
        let ppp = window.scale_factor();
        // Запоминаем DPI для data prepare path (у него нет window). DPI меняется редко.
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
        // ВАЖНО: НЕТ request_animation_frame/continuous-present. `gpu_canvas.frame()` решает
        // present на platform tick без dirty GPUI tree; `draw()` рисует в тот же tick.
        let (mut theme, orders_style, follow) = {
            let b = self.backend.read(cx);
            let eff = b.preview.as_ref().unwrap_or(&b.config);
            (eff.theme.clone(), eff.orders.clone(), b.follow)
        };
        if palette.is_light() {
            theme.bg = rgb3_from_hex(palette.chart_bg);
            theme.grid = rgb3_from_hex(palette.row_line);
            theme.grid_alpha = theme.grid_alpha.clamp(0.0, 1.0);
            theme.book_bg = rgb3_from_hex(palette.chart_bg);
        }
        // Масштаб — ПО-ВКЛАДОЧНЫЙ: берём self.scale (его правят set_scale из тулбара активной
        // вкладки / шапки выносного окна), а не глобальный backend.price_scale.
        let mut settings_changed = self.chart.set_theme(theme)
            | self.chart.set_orders(orders_style)
            | self.chart.set_scale(self.scale)
            | self.chart.set_orderbook_enabled(self.orderbook_enabled)
            | self.chart.set_orderbook_only(self.orderbook_only)
            | self.chart.set_follow(follow, now_unix_ms());
        // Режим сравнения: пока активен lock, держим Y-окно якоря (перебивает scale каждый кадр —
        // set_locked_y идемпотентен, без изменений вернёт false). Снятие lock — в set_locked_y.
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

        // axis_panes (раскладка панелей + снимок) считаем ОДИН раз за кадр и переиспользуем
        // и для hit-теста ввода (pane_rects), и для отрисовки осей — раньше layout панелей
        // гонялся дважды (внутри гейта prepare ради pane_rects + здесь ради отрисовки).
        let axis_panes = self.chart.axis_panes(axes::local_offset_sec());
        self.input.pane_rects = self.chart.pane_rects();
        // Угловой ✕ закрытия монеты — на панели графика (и Main, и AddToChart):
        // закрыл монету на Main → вернулись к лого. Позиция из раскладки панелей (девайс-px →
        // лог.px слота); собираем ДО canvas, который забирает axis_panes по move.
        let close_btns: Vec<(usize, f32, f32)> = axis_panes
            .iter()
            .map(|(idx, rect, _)| (*idx, (rect.x + rect.w) / ppp, rect.y / ppp))
            .collect();
        // Cursor-only motion is handled by the chart-slot hitbox below. It updates retained
        // gpu_canvas cursor/readout directly and does not notify the GPUI tree.
        // П.2: кнопка «пин» в левом верхнем углу ВНУТРИ области графика (правее ценовой оси,
        // не на самой оси) — ТОЛЬКО на AddToChart-панелях (с TTL). Пин отменяет авто-закрытие.
        // (idx, pinned, left_px, top_px). PRICE_AXIS_W — логическая ширина оси (rect в девайс-px).
        // В режиме «только стакан» оси цен нет → кнопки у левого края слота (без сдвига на ось).
        let axis_off = if self.orderbook_only {
            0.0
        } else {
            moon_chart::PRICE_AXIS_W
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
        // Кнопка-замок режима сравнения — ТОЛЬКО когда вкладка горизонтальная (`compare_eligible`),
        // рядом с пином. Горит на якоре (`is_compare_anchor`). Клик переносит чарт влево и делает
        // его ведущим по цене (обрабатывает стек по `take_compare_lock_request`).
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
        // Кнопка-метла — ТОЛЬКО на якоре (рядом с горящим замком). Переключает «только стакан»
        // у соседей якоря.
        let broom_btns: Vec<(usize, f32, f32)> = if self.compare_eligible && compare_anchor {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| (*idx, rect.x / ppp + axis_off, rect.y / ppp))
                .collect()
        } else {
            Vec::new()
        };
        // Риска зоны управления: при раздельных зонах И СКРЫТОМ стакане рисуем границу зоны
        // ордеров (справа поверх чарта), чтобы было видно, где клики ставят ордера, а где
        // дабл-клик уходит на Main. Стакан виден → его видно и так, риску не дублируем.
        // (idx, left_лог, top_лог, w_лог, h_лог) — device-px из axis_panes делим на ppp, как ✕.
        let show_zone_marker = self.show_zone && self.separate_zones(cx) && !self.orderbook_enabled;
        let zone_markers: Vec<(usize, f32, f32, f32, f32)> = if show_zone_marker {
            axis_panes
                .iter()
                .map(|(idx, rect, _)| {
                    let zone_w = moon_chart::GLASS_ZONE_PX.min(rect.w * 0.5);
                    let plot_h = (rect.h - moon_chart::TIME_AXIS_H * ppp).max(1.0);
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
        let show_empty_logo = axis_panes.is_empty();
        let (slot_w, _) = self.chart.slot_dev_size();
        let logo_w = ((slot_w as f32 / ppp) * 0.28).clamp(180.0, 280.0);
        div()
            .id("chart-slot")
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .relative()
            .track_focus(&self.focus)
            .when(self.order_drag.is_some(), |this| this.cursor_grabbing())
            .when(
                self.order_drag.is_none() && self.order_hover.is_some(),
                |this| this.cursor_grab(),
            )
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, window, cx| {
                if cx.has_active_drag() {
                    return;
                } // идёт drag Dock-панели — не мешаем drop
                if this.main_stack_scroll && this.window_pos_in_glass_zone(e.position) {
                    return;
                }
                let sf = window.scale_factor();
                let Some((pos, within)) = this.chart_local(e.position) else {
                    return;
                };
                // В AddToChart-стеке колесо НАД ЦЕНОВОЙ ОСЬЮ (левее графика) скроллит сам стек,
                // а не зумит: не потребляем событие → оно всплывёт к MoonVirtualList. Над
                // графиком+стаканом — зум (ниже) + stop_propagation, чтобы стек не скроллился.
                if this.num.is_some() && within {
                    if let Some(idx) = this.input.pane_at(pos.0, pos.1) {
                        if let Some((_, rect)) =
                            this.input.pane_rects.iter().find(|(i, _)| *i == idx)
                        {
                            if pos.0 <= rect.x + moon_chart::PRICE_AXIS_W * sf {
                                return;
                            }
                        }
                    }
                }
                let dy = match e.delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
                };
                this.input.last_ptr = pos;
                this.input.cursor = if within { Some(pos) } else { None };
                this.input.hovered_pane = this.input.pane_at(pos.0, pos.1);
                this.sync_native_cursor();
                let fb = this.chart.slot_dev_width();
                let changed = {
                    let input = &mut this.input;
                    this.chart.with_container_mut(|container| {
                        input.wheel(dy, e.modifiers.shift, within, container, fb, sf)
                    })
                };
                if changed {
                    this.mark_input_changed(cx);
                    crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                    cx.notify();
                }
                // Зум-зона графика: гасим всплытие, иначе колесо ещё и проскроллит стек.
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    if cx.has_active_drag() {
                        return;
                    }
                    let sf = window.scale_factor();
                    let Some((pos, within)) = this.chart_local(e.position) else {
                        return;
                    };
                    this.input.last_ptr = pos;
                    this.input.cursor = if within { Some(pos) } else { None };
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    this.sync_native_cursor();
                    if within
                        && this.try_place_order_click(
                            TradeMouseButton::Left,
                            e.modifiers,
                            e.click_count,
                            pos,
                            cx,
                        )
                    {
                        cx.stop_propagation();
                        return;
                    }
                    if within && e.click_count <= 1 && this.try_start_order_drag(pos, cx) {
                        this.sync_native_cursor();
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    // На AddToChart-вкладках дабл-клик по ЧАРТУ → открыть монету на Main (fullscreen).
                    let allow_to_main = this.num.is_some();
                    let fb = this.chart.slot_dev_width();
                    let input_changed = {
                        let input = &mut this.input;
                        this.chart.with_container_mut(|container| {
                            input.mouse_button(
                                input::Btn::Left,
                                true,
                                within,
                                allow_to_main,
                                container,
                                sf,
                                fb,
                            )
                        })
                    };
                    let mut opened_to_main = false;
                    if let Some((core, market)) = this.input.pending_to_main.take() {
                        this.backend.update(cx, |b, bcx| {
                            b.open_request = Some((core, market));
                            b.open_request_rev = b.open_request_rev.wrapping_add(1);
                            // Только этот путь (дабл-клик по чарту) поднимает окно Main (П.1).
                            b.open_request_activate = true;
                            bcx.notify();
                        });
                        opened_to_main = true;
                    }
                    if input_changed || opened_to_main {
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _e: &MouseUpEvent, window, cx| {
                    if this.finish_order_drag(cx) {
                        this.sync_native_cursor();
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    let sf = window.scale_factor();
                    let fb = this.chart.slot_dev_width();
                    let changed = {
                        let input = &mut this.input;
                        this.chart.with_container_mut(|container| {
                            input.mouse_button(
                                input::Btn::Left,
                                false,
                                false,
                                false,
                                container,
                                sf,
                                fb,
                            )
                        })
                    };
                    if changed {
                        this.mark_input_changed(cx);
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    let sf = window.scale_factor();
                    let Some((pos, within)) = this.chart_local(e.position) else {
                        return;
                    };
                    this.input.last_ptr = pos;
                    this.input.cursor = if within { Some(pos) } else { None };
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    this.sync_native_cursor();
                    if within
                        && this.try_place_order_click(
                            TradeMouseButton::Right,
                            e.modifiers,
                            e.click_count,
                            pos,
                            cx,
                        )
                    {
                        cx.stop_propagation();
                        return;
                    }
                    if this.num.is_none() && this.window_pos_in_glass_zone(e.position) {
                        return;
                    }
                    let fb = this.chart.slot_dev_width();
                    let changed = {
                        let input = &mut this.input;
                        this.chart.with_container_mut(|container| {
                            input.mouse_button(
                                input::Btn::Right,
                                true,
                                within,
                                false,
                                container,
                                sf,
                                fb,
                            )
                        })
                    };
                    if changed {
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, e: &MouseUpEvent, window, cx| {
                    if this.num.is_none() && this.window_pos_in_glass_zone(e.position) {
                        return;
                    }
                    let sf = window.scale_factor();
                    let fb = this.chart.slot_dev_width();
                    let changed = {
                        let input = &mut this.input;
                        this.chart.with_container_mut(|container| {
                            input.mouse_button(
                                input::Btn::Right,
                                false,
                                false,
                                false,
                                container,
                                sf,
                                fb,
                            )
                        })
                    };
                    if changed {
                        this.view_dirty = true;
                        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, e: &MouseDownEvent, _window, cx| {
                    let Some((pos, within)) = this.chart_local(e.position) else {
                        return;
                    };
                    this.input.last_ptr = pos;
                    this.input.cursor = if within { Some(pos) } else { None };
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    this.sync_native_cursor();
                    if within
                        && this.try_place_order_click(
                            TradeMouseButton::Middle,
                            e.modifiers,
                            e.click_count,
                            pos,
                            cx,
                        )
                    {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                if cx.has_active_drag() {
                    return;
                } // идёт drag Dock-панели — не перехватываем
                let Some((pos, within)) = this.chart_local(e.position) else {
                    return;
                };
                crate::diag::bump(&crate::diag::CHART_MOUSE_MOVE);
                if e.pressed_button.is_none() {
                    if this.order_drag.take().is_some() {
                        this.apply_order_visual(cx);
                        this.sync_native_cursor();
                        cx.notify();
                    }
                    crate::diag::bump(&crate::diag::CHART_MOUSE_MOVE_FAST);
                    let prev_cursor = this.input.cursor;
                    let prev_hovered = this.input.hovered_pane;
                    this.input.cursor = if within { Some(pos) } else { None };
                    this.input.hovered_pane = if within {
                        this.input.pane_at(pos.0, pos.1)
                    } else {
                        None
                    };
                    let cursor_changed =
                        prev_cursor != this.input.cursor || prev_hovered != this.input.hovered_pane;
                    if cursor_changed && this.sync_native_cursor() {
                        crate::diag::bump(&crate::diag::CHART_CURSOR_UPDATE);
                    }
                    let order_hover_changed = if within {
                        this.sync_order_hover(pos, cx)
                    } else {
                        this.set_order_interaction(None, cx)
                    };
                    if order_hover_changed {
                        cx.notify();
                    }
                    if within {
                        crate::diag::bump(&crate::diag::CHART_MOUSE_FAST_STOP);
                        cx.stop_propagation();
                    }
                    return;
                }
                crate::diag::bump(&crate::diag::CHART_MOUSE_MOVE_ENTITY);
                let sf = window.scale_factor();
                this.input.sync_pressed(
                    e.pressed_button == Some(MouseButton::Left),
                    e.pressed_button == Some(MouseButton::Right),
                );
                if this.order_drag.is_some() {
                    this.update_order_drag(pos, cx);
                    cx.stop_propagation();
                    return;
                }
                let prev_cursor = this.input.cursor;
                let prev_hovered = this.input.hovered_pane;
                this.input.cursor = if within { Some(pos) } else { None };
                this.input.hovered_pane = if within {
                    this.input.pane_at(pos.0, pos.1)
                } else {
                    None
                };
                let fb = this.chart.slot_dev_width();
                let dragging = {
                    let input = &mut this.input;
                    this.chart.with_container_mut(|container| {
                        input.pointer_drag(pos.0, pos.1, container, sf, fb)
                    })
                };
                if dragging {
                    this.mark_input_changed(cx);
                }
                let cursor_changed =
                    prev_cursor != this.input.cursor || prev_hovered != this.input.hovered_pane;
                if cursor_changed {
                    if this.sync_native_cursor() {
                        crate::diag::bump(&crate::diag::CHART_CURSOR_UPDATE);
                    }
                }
                // Drag меняет камеры/оси и GPUI-side controls. Cursor-only move теперь
                // остаётся в retained gpu_canvas: crosshair/readout present без cx.notify().
                if dragging {
                    crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _window, _cx| {
                if !*hovered {
                    let had_order_drag = this.order_drag.take().is_some();
                    let had_order_hover = this.order_hover.take().is_some();
                    if had_order_drag || had_order_hover {
                        this.apply_order_visual(_cx);
                        _cx.notify();
                    }
                    let changed = this.input.cursor.take().is_some()
                        || this.input.hovered_pane.take().is_some();
                    if changed {
                        this.sync_native_cursor();
                    }
                }
            }))
            // own-pass: геометрию слота движок берёт синхронно из `GpuFrameInfo.bounds` в
            // `frame()` (см. data_state::apply_slot_geometry) — поэтому уже первый present рисует
            // в реальном слоте, без «распахивания» дефолтного размера и без лага при рефлоу.
            .child(self.chart.canvas().text_under().absolute().size_full())
            .when(show_empty_logo, |this| {
                // Непрозрачный фон поверх own-pass: пустой слот = логотип на фоне чарта, без
                // просвечивания старого графика (own-pass рисуется ПОД сценой GPUI).
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
            // FireTest probe only. Геометрию самого чарта не берём из GPUI-probe: единственный
            // source of truth для input/own-pass — `GpuFrameInfo.bounds`.
            .child({
                let is_main = self.num.is_none();
                let backend = self.backend.clone();
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        let sf = window.scale_factor();
                        let firetest_probe = crate::firetest::ChartProbe::new(
                            crate::windowing::window_hwnd(window),
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
                // Тусклая заливка зоны управления (стакан скрыт) — без линии-границы.
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(w))
                    .h(px(h))
                    .bg(rgba_from(palette.blue, 0.03))
            }))
            .children(close_btns.into_iter().map(|(idx, right, top)| {
                let entity = cx.entity();
                MoonButton::new(SharedString::from(format!("chart-close-{idx}")))
                    .label("×")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    // Крупнее (было 15×15) — чтобы не мискликнуть мимо на стакан при быстром
                    // закрытии нескольких графиков подряд.
                    .bounds(MoonRect::new(right - 26.0, top + 3.0, 22.0, 22.0))
                    .on_click(move |_, _w, app| {
                        entity.update(app, |this, cx| this.remove_pane(idx, cx));
                    })
                    .render()
            }))
            .children(pin_btns.into_iter().map(|(idx, pinned, left, top)| {
                // Пин-кнопка в левом верхнем углу: заполненный кружок = приколото, контур = нет (П.2).
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
                // Замок справа от пина: клик → этот чарт в начало ряда + ведущий по цене.
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
                // Метла справа от замка (на якоре): «только стакан» у соседей.
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
    }
}
