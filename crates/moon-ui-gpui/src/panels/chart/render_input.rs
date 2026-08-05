//! Chart-slot input handlers for wheel, buttons, pointer motion, and hover.
//!
//! These free functions use the `(this, event, window, cx)` signature expected by the named
//! `cx.listener` registrations in `render.rs`.

use gpui::*;

use crate::chartdx::input;

use super::ChartPanel;
use super::trade::TradeMouseButton;

/// Routes a wheel event to chart zoom/pan or the surrounding stack scroll.
pub(super) fn scroll_wheel(
    this: &mut ChartPanel,
    e: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
    if cx.has_active_drag() {
        return;
    } // Do not interfere with a dock-panel drag/drop operation.
    if this.main_stack_scroll && this.window_pos_in_glass_zone(e.position) {
        return;
    }
    let sf = window.scale_factor();
    let Some((pos, within)) = this.chart_local(e.position) else {
        return;
    };
    // In an AddToChart stack, wheel input over the left price-axis strip scrolls the stack rather
    // than zooming: leave the event unconsumed so it bubbles to MoonVirtualList. Over the graph or
    // book, handle zoom below and stop propagation so the stack does not scroll too.
    if this.num.is_some() && within {
        if let Some(idx) = this.input.pane_at(pos.0, pos.1) {
            if let Some((_, rect)) = this.input.pane_rects.iter().find(|(i, _)| *i == idx) {
                if pos.0 <= rect.x + moon_chart::PRICE_AXIS_W * sf {
                    return;
                }
            }
        }
    }
    // Lines represent discrete mouse-wheel clicks, commonly +/-1 or +/-3 on Windows. Pixels are
    // precise trackpad/Magic Mouse input on macOS, delivered as a continuous inertial stream.
    // Preserve the distinction through `precise` so input.wheel scales them differently.
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
            // Built-in gesture: Shift OR Alt + wheel pans time left/right; no modifier zooms time.
            let pan = e.modifiers.shift || e.modifiers.alt;
            input.wheel(dy, precise, pan, within, container, fb, sf)
        })
    };
    if changed {
        this.mark_input_changed(cx);
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
    }
    // Stop propagation in the chart zoom zone so the wheel does not also scroll the stack.
    cx.stop_propagation();
}

/// Routes left-button down through figures, trading, order drag, and chart navigation.
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
    // The figure layer reacts only in pencil mode and only to the secondary modifier when starting
    // or grabbing a figure. An ordinary unmodified left click makes try_fig_click return false and
    // continues to trading/navigation, matching Moonbot even while the pencil is enabled. Outside
    // pencil mode it also returns false immediately. secondary() is Command on macOS and Ctrl on
    // Windows/Linux; macOS Ctrl cannot be used because the OS converts Ctrl+left-click to a right
    // click before the drawing event arrives. An active draft continues without a modifier, so
    // Command/Ctrl is required only on the first click.
    if within
        && e.click_count <= 1
        && this.try_fig_click(pos, e.modifiers.secondary() || this.fig_draft.is_some(), cx)
    {
        cx.notify();
        cx.stop_propagation();
        return;
    }
    // The click was not the figure layer's — but a click that landed on no figure still ends the
    // selection, which is what every editor does and what the handles left on screen otherwise
    // contradict. Deliberately here rather than inside `try_fig_click`: that path returns early
    // without the modifier, and this must hold for the ordinary clicks that make up most of them.
    // The settings panel swallows its own input, so a click arriving here landed outside it — the
    // dismissal every popup on this chart uses. It CONSUMES the click: the first click outside an
    // open panel closes it and does nothing else, or dismissing the panel could cancel a live
    // order, place one, or start a drag, depending on where it happened to land.
    if within && e.click_count <= 1 && this.fig_settings.take().is_some() {
        cx.notify();
        cx.stop_propagation();
        return;
    }
    if within && e.click_count <= 1 {
        this.fig_clear_selection_on_miss(pos, cx);
    }
    if within
        && this.try_place_order_click(TradeMouseButton::Left, e.modifiers, e.click_count, pos, cx)
    {
        cx.stop_propagation();
        return;
    }
    // Clicking the start cross of an unfilled entry cancels it before drag handling, so dragging
    // never starts from that cross.
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
    // With separate zones, left clicks in the control area (book/reserved strip) are trading-only.
    // Do not route normal chart pan or the "open on Main" double-click through this area.
    if this.window_pos_in_control_zone(e.position, cx) {
        return;
    }
    // On AddToChart tabs, double-clicking the CHART opens its coin on fullscreen Main.
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
            b.open_on_main((core, market), true);
            bcx.notify();
        });
        opened_to_main = true;
    }
    if input_changed || opened_to_main {
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
    }
}

/// Routes left-button up to finish a figure/order drag or chart navigation.
pub(super) fn mouse_up_left(
    this: &mut ChartPanel,
    e: &MouseUpEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
    // A draw-drag-release gesture (Command/Ctrl down, drag, release) completes a segment/channel
    // without a second click. A stationary click is not a drag gesture and waits for click two.
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

/// Routes right-button down through figure/order menus, trading, and chart pan/zoom.
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
    // Right-clicking a drawn figure in pencil mode opens its Alert/Delete menu. This has highest
    // priority; suppress_rmb_up consumes the paired release so fullscreen remains intact.
    if within && this.try_open_figure_menu(pos, e.position, window, cx) {
        this.suppress_rmb_up = true;
        cx.stop_propagation();
        return;
    }
    // Right-clicking a Buy or Sell order line opens its side-specific menu before placement or
    // zoom. Other line kinds fall through to normal right-button routing. Suppress the paired
    // release when the menu opens so a parent does not exit fullscreen or perform another action.
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
    // With separate zones, right clicks in the control area are only for trading/order menus.
    // Suppress normal chart right-button pan/zoom there; main_stack.rs owns fullscreen toggling.
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

/// Routes right-button up or consumes it after a context-menu action or handled trading gesture.
pub(super) fn mouse_up_right(
    this: &mut ChartPanel,
    e: &MouseUpEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
    // When right-button down opened a figure/order menu or handled a trading gesture, consume its
    // paired release instead of letting a parent exit the Main stack's fullscreen mode.
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

/// Routes middle-button down to trading or window-local X-scale synchronization.
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
    // Shift+middle-click on the graph synchronizes the time X scale across charts in THIS window,
    // matching Moonbot. A trading gesture bound to Shift+middle-click takes priority above.
    if within && e.modifiers.shift && this.sync_x_scale_window(_window, cx) {
        cx.stop_propagation();
    }
}

/// Routes pointer motion through retained cursor/hover updates and active drags.
pub(super) fn mouse_move(
    this: &mut ChartPanel,
    e: &MouseMoveEvent,
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
    if cx.has_active_drag() {
        return;
    } // Do not intercept pointer motion during a dock-panel drag.
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
        // Same for a figure drag whose mouse-up was lost (a window switch, a capture steal): a
        // stranded drag would move the figure on the next press and, until then, keep its fill
        // suppressed. Only from motion INSIDE the chart: the Windows fork reports a non-client
        // mouse move with no pressed button, which happens during a perfectly live drag.
        if within && this.fig_drag.is_some() {
            this.finish_fig_drag(cx);
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
        // Update figure-draft preview under the cursor and figure hover highlighting.
        this.update_fig_pointer(pos, within, false, cx);
        // News marks: one Y comparison unless the pointer is in the marks' row along the bottom
        // edge. Repaints only while the Ctrl card is on screen.
        this.note_news_modifiers(e.modifiers, cx);
        this.note_warn_modifiers(e.modifiers, cx);
        if this.sync_news_hover(pos, within, cx) {
            cx.notify();
        }
        if this.sync_warn_hover(pos, within, cx) {
            cx.notify();
        }
        let order_hover_changed = if within {
            this.sync_order_hover(pos, cx)
        } else {
            // On leaving the chart, clear the threshold probe so returning within the same <1 px
            // neighborhood still recomputes hover instead of getting stuck.
            this.order_hover_probe = None;
            this.fig_hover_probe = None;
            this.fig_draft_probe = None;
            this.set_order_interaction(None, cx)
        };
        if order_hover_changed {
            // One notify here re-renders the WHOLE window: `mark_view_dirty` marks every ancestor,
            // and a re-rendered root sets `refreshing`, which bypasses each descendant's view
            // cache. Counted because the existing mouse counters do not instrument this branch at
            // all — the harness gates "mouse-move must not wake the scene" on counters blind to it.
            crate::diag::bump(&crate::diag::CHART_HOVER_NOTIFY);
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
    // Dragging changes cameras/axes and GPUI-side controls. Cursor-only motion remains in retained
    // gpu_canvas, which presents the crosshair/readout without cx.notify().
    if dragging {
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
    }
}

/// Tracks chart-slot enter/leave state and clears cursor/order interaction on leave.
pub(super) fn hover(
    this: &mut ChartPanel,
    hovered: &bool,
    _window: &mut Window,
    _cx: &mut Context<ChartPanel>,
) {
    // Track the chart under the pointer for cursor-dependent new_long/new_short hotkeys. Enter/leave
    // is infrequent rather than per-pixel, and the backend update omits notify to avoid rendering.
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
        // Leaving the slot must also drop a news card: the pointer can exit without a final
        // mouse-move inside the chart.
        if this.clear_news_hover(_cx) {
            _cx.notify();
        }
        if this.clear_warn_hover(_cx) {
            _cx.notify();
        }
        let changed =
            this.input.cursor.take().is_some() || this.input.hovered_pane.take().is_some();
        if changed {
            this.sync_native_cursor();
        }
    }
}
