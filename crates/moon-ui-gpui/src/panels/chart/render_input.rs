//! Обработчики ввода слота чарта (колесо мыши / кнопки / движение / ховер): тела замыканий
//! `cx.listener` из `render.rs`, вынесенные вербатим в свободные функции (та же сигнатура
//! `(this, event, window, cx)`, подставляются в `cx.listener` по имени).

use gpui::*;

use crate::input;

use super::ChartPanel;
use super::trade::TradeMouseButton;

/// Колесо: зум/скролл чарта (тело `on_scroll_wheel`).
pub(super) fn scroll_wheel(
    this: &mut ChartPanel,
    e: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
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
            if let Some((_, rect)) = this.input.pane_rects.iter().find(|(i, _)| *i == idx) {
                if pos.0 <= rect.x + moon_chart::PRICE_AXIS_W * sf {
                    return;
                }
            }
        }
    }
    // Lines — дискретное колесо мыши (Windows): ±1/±3 за щелчок. Pixels — точный ввод
    // (тачпад/Magic Mouse на macOS): непрерывный поток с инерцией. Их нельзя мешать в
    // одну величину — передаём `precise`, чтобы input.wheel зумил их по-разному.
    let (dy, precise) = match e.delta {
        ScrollDelta::Lines(p) => (p.y, false),
        ScrollDelta::Pixels(p) => (f32::from(p.y), true),
    };
    this.input.last_ptr = pos;
    this.input.cursor = if within { Some(pos) } else { None };
    this.input.hovered_pane = this.input.pane_at(pos.0, pos.1);
    this.sync_native_cursor();
    let fb = this.chart.slot_dev_width();
    let changed = {
        let input = &mut this.input;
        this.chart.with_container_mut(|container| {
            // Встроенный хоткей: Shift ИЛИ Alt + колесо = пан по времени (влево/вправо),
            // без модификаторов = зум по времени.
            let pan = e.modifiers.shift || e.modifiers.alt;
            input.wheel(dy, precise, pan, within, container, fb, sf)
        })
    };
    if changed {
        this.mark_input_changed(cx);
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
    }
    // Зум-зона графика: гасим всплытие, иначе колесо ещё и проскроллит стек.
    cx.stop_propagation();
}

/// ЛКМ down: фигуры/торговля/drag ордера/навигация (тело `on_mouse_down(Left)`).
pub(super) fn mouse_down_left(
    this: &mut ChartPanel,
    e: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
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
    // Слой рисования фигур (только в режиме карандаша): фигуры трогаются ТОЛЬКО по Ctrl
    // (secondary) — рисование новой / захват существующей. Обычный ЛКМ без модификатора
    // try_fig_click отдаёт false → клик идёт в торговлю/навигацию (паритет с Moonbot: карандаш
    // включён, но обычный ЛКМ торгует). Вне режима карандаша try_fig_click тоже сразу false.
    // secondary() = ⌘ на macOS, Ctrl на Windows/Linux. На macOS именно Ctrl нельзя:
    // ОС превращает Ctrl+ЛКМ в правый клик, поэтому событие рисования не доходит.
    // Активный драфт = продолжение рисования: следующие клики завершают фигуру и
    // БЕЗ модификатора (держать ⌘/Ctrl нужно только на первом клике).
    if within
        && e.click_count <= 1
        && this.try_fig_click(pos, e.modifiers.secondary() || this.fig_draft.is_some(), cx)
    {
        cx.notify();
        cx.stop_propagation();
        return;
    }
    if within
        && this.try_place_order_click(TradeMouseButton::Left, e.modifiers, e.click_count, pos, cx)
    {
        cx.stop_propagation();
        return;
    }
    // Клик по кресту начала невыполненного входа — отмена ордера (до drag: с креста
    // перетаскивание не начинается).
    if within && e.click_count <= 1 && this.try_cancel_order_click(pos, cx) {
        cx.notify();
        cx.stop_propagation();
        return;
    }
    if within && e.click_count <= 1 && this.try_start_order_drag(pos, cx) {
        this.sync_native_cursor();
        cx.notify();
        cx.stop_propagation();
        return;
    }
    // Раздельные зоны: в зоне управления (стакан/резерв-полоса) ЛКМ — только
    // торговля; обычную навигацию чарта (пан, дабл-клик «на Main») здесь не пускаем.
    if this.window_pos_in_control_zone(e.position, cx) {
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
}

/// ЛКМ up: завершение drag фигуры/ордера/навигации (тело `on_mouse_up(Left)`).
pub(super) fn mouse_up_left(
    this: &mut ChartPanel,
    e: &MouseUpEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
    // Drag-release рисования: «⌘/Ctrl+нажал — потянул — отпустил» завершает фигуру
    // (отрезок/канал) без второго клика; клик на месте — не жест, ждём второго клика.
    if let Some((pos, _)) = this.chart_local(e.position) {
        if this.try_fig_release(pos, cx) {
            cx.notify();
            cx.stop_propagation();
            return;
        }
    }
    if this.finish_fig_drag(cx) {
        cx.notify();
        cx.stop_propagation();
        return;
    }
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
            input.mouse_button(input::Btn::Left, false, false, false, container, sf, fb)
        })
    };
    if changed {
        this.mark_input_changed(cx);
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
    }
}

/// ПКМ down: меню фигур/ордеров, торговля, пан/зум (тело `on_mouse_down(Right)`).
pub(super) fn mouse_down_right(
    this: &mut ChartPanel,
    e: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
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
    // ПКМ по нарисованной фигуре (в режиме карандаша) → меню Alert/Удалить.
    // Приоритетнее всего; suppress_rmb_up гасит парный up → фулскрин НЕ рвётся.
    if within && this.try_open_figure_menu(pos, e.position, window, cx) {
        this.suppress_rmb_up = true;
        cx.stop_propagation();
        return;
    }
    // ПКМ по линии ордера → контекстное меню (Cancel / Join sells / Split).
    // Приоритетнее постановки/зума: на линии всегда открываем меню. Гасим и
    // последующий ПКМ-up (флаг), чтобы родитель не вышел из фулскрина и т.п.
    if within && this.try_open_order_menu(pos, e.position, window, cx) {
        this.suppress_rmb_up = true;
        cx.stop_propagation();
        return;
    }
    if within
        && this.try_place_order_click(TradeMouseButton::Right, e.modifiers, e.click_count, pos, cx)
    {
        this.suppress_rmb_up = true;
        cx.stop_propagation();
        return;
    }
    // Раздельные зоны: в зоне управления ПКМ — только торговля/меню ордеров;
    // обычный ПКМ-пан/зум чарта подавляем (а тоггл fullscreen — в main_stack.rs).
    if this.window_pos_in_control_zone(e.position, cx) {
        return;
    }
    let fb = this.chart.slot_dev_width();
    let changed = {
        let input = &mut this.input;
        this.chart.with_container_mut(|container| {
            input.mouse_button(input::Btn::Right, true, within, false, container, sf, fb)
        })
    };
    if changed {
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
    }
}

/// ПКМ up (тело `on_mouse_up(Right)`).
pub(super) fn mouse_up_right(
    this: &mut ChartPanel,
    e: &MouseUpEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
    // ПКМ-down открыл меню ордера → гасим парный up (и не пускаем его к родителю,
    // который иначе вышел бы из фулскрина Main-стека).
    if this.suppress_rmb_up {
        this.suppress_rmb_up = false;
        cx.stop_propagation();
        return;
    }
    if this.window_pos_in_control_zone(e.position, cx) {
        return;
    }
    let sf = window.scale_factor();
    let fb = this.chart.slot_dev_width();
    let changed = {
        let input = &mut this.input;
        this.chart.with_container_mut(|container| {
            input.mouse_button(input::Btn::Right, false, false, false, container, sf, fb)
        })
    };
    if changed {
        this.view_dirty = true;
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
    }
}

/// СКМ down: постановка ордера средней кнопкой (тело `on_mouse_down(Middle)`).
pub(super) fn mouse_down_middle(
    this: &mut ChartPanel,
    e: &MouseDownEvent,
    _window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
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
        return;
    }
    // [Shift+СКМ] на графике — синхронизация временного X-масштаба чартов ЭТОГО окна
    // (Moonbot). Торговый жест (если назначен на Shift+СКМ) имеет приоритет — выше.
    if within && e.modifiers.shift && this.sync_x_scale_window(_window, cx) {
        cx.stop_propagation();
    }
}

/// Движение мыши: курсор/ховер/drag (тело `on_mouse_move`).
pub(super) fn mouse_move(
    this: &mut ChartPanel,
    e: &MouseMoveEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
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
        // Фигуры: превью драфта за курсором + hover-подсветка.
        this.update_fig_pointer(pos, within, false, cx);
        let order_hover_changed = if within {
            this.sync_order_hover(pos, cx)
        } else {
            // Курсор ушёл с чарта: сбросить точку порога, чтобы возврат в ту же
            // окрестность (<1px) не «застрял» без пересчёта ховера.
            this.order_hover_probe = None;
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
    if this.fig_drag.is_some() {
        this.update_fig_pointer(pos, within, true, cx);
        cx.stop_propagation();
        return;
    }
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
        this.chart
            .with_container_mut(|container| input.pointer_drag(pos.0, pos.1, container, sf, fb))
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
}

/// Уход курсора со слота (тело `on_hover`).
pub(super) fn hover(
    this: &mut ChartPanel,
    hovered: &bool,
    _window: &mut Window,
    _cx: &mut Context<ChartPanel>,
) {
    // Отметить/снять «чарт под курсором» для курсор-зависимых хоткеев (new_long/new_short).
    // Enter/leave — редкое событие (не на каждый пиксель), backend без notify → без рендера.
    let self_id = _cx.entity_id();
    let weak = _cx.entity().downgrade();
    let hov = *hovered;
    this.backend.update(_cx, |b, _| {
        if hov {
            b.hovered_chart = Some(weak);
        } else if b.hovered_chart.as_ref().map(|w| w.entity_id()) == Some(self_id) {
            b.hovered_chart = None;
        }
    });
    if !*hovered {
        let had_order_drag = this.order_drag.take().is_some();
        let had_order_hover = this.order_hover.take().is_some();
        if had_order_drag || had_order_hover {
            this.apply_order_visual(_cx);
            _cx.notify();
        }
        let changed =
            this.input.cursor.take().is_some() || this.input.hovered_pane.take().is_some();
        if changed {
            this.sync_native_cursor();
        }
    }
}
