//! Chart-slot input handlers for wheel, buttons, pointer motion, and hover.
//!
//! These free functions use the `(this, event, window, cx)` signature expected by the named
//! `cx.listener` registrations in `render.rs`.

use std::time::{Duration, Instant};

use gpui::*;

use crate::chartdx::input;

/// Smallest gap between two drag-driven `cx.notify()` calls.
///
/// 33 ms is the `chart_render_per_chart` ceiling of 30/s that `firetest/verdict.rs` already holds
/// this panel to — a drag is simply the one gesture the storm phases never produce, so it was
/// running at four times the project's own budget unnoticed.
///
/// Deliberately a constant and not the chart's own present interval, which would read better but
/// could never bind: that pacer is fed from a refresh rate clamped to 30 Hz at the low end, so the
/// two differ by at most a third of a millisecond.
const DRAG_NOTIFY_MIN_INTERVAL: Duration = Duration::from_millis(33);

use super::trade::TradeMouseButton;
use super::ChartPanel;

/// Count one press against the series this panel itself saw, for the trading gestures to match.
///
/// Positions are taken in SCREEN pixels, not window ones: the close mark this consults is shared by
/// every window in the process, and window-local coordinates would make an unrelated press in a
/// detached chart window collide with it.
///
/// Args:
///     this: Panel receiving the press.
///     button: Mouse button pressed.
///     native: Click count the window reported, which pairs presses across every chart in it.
///     position: Press position in window coordinates.
///     window: Window the press arrived in, for its screen origin.
///
/// Returns:
///     The click count belonging to this panel (see [`super::ClickSeries`]), or `None` when the
///     press is left over from closing a chart and must not trade at all.
fn press_count(
    this: &mut ChartPanel,
    button: MouseButton,
    native: usize,
    position: Point<Pixels>,
    window: &Window,
    cx: &mut Context<ChartPanel>,
) -> Option<usize> {
    let now = moon_chart::paint::now_unix_ms();
    // `bounds()`, not `window_bounds()`: the latter is the RESTORE rectangle, which for a maximized
    // window names a position it is not at — and this position is compared against a mark every
    // window in the process shares.
    let origin = window.bounds().origin;
    let pos = (
        f32::from(origin.x + position.x),
        f32::from(origin.y + position.y),
    );
    let count = this.click_series.observe(button, native, now, pos);
    // A press still parked where a × was clicked belongs to that closing, however many charts the
    // reflow has walked under it since, so no gesture may trade on it — not only the double ones,
    // since a single-press binding placed on the middle button or a modifier would sail past a
    // count check. The mark's POSITION follows the press: a hand stabbing one button drifts a few
    // pixels per press, and an anchor left behind would let the chain walk out of range and trade.
    // Its clock does not: refreshing that too would make one stubborn spot unreachable forever.
    if super::click_series::press_is_close_residue(this.backend_close_mark(cx), now, pos) {
        this.mark_close_residue(pos, cx);
        return None;
    }
    Some(count)
}

/// Offers a press to the order-line grab, one call per button.
///
/// Both counts come from the same event here rather than from three call sites: the panel's own
/// count decides a gesture, the window's native one answers "is this the second press of a pair",
/// and a site that passed one where the other belonged would not be caught by anything.
fn grab_order_line(
    this: &mut ChartPanel,
    button: TradeMouseButton,
    e: &MouseDownEvent,
    clicks: Option<usize>,
    pos: (f32, f32),
    cx: &mut Context<ChartPanel>,
) -> bool {
    let grabbed = clicks.is_some_and(|count| {
        this.try_start_order_drag(button, e.modifiers, count, e.click_count <= 1, pos, cx)
    });
    if grabbed {
        this.sync_native_cursor();
        cx.notify();
        cx.stop_propagation();
    }
    grabbed
}

/// Ends a drag on the release of the button that owns it, one call per button.
fn release_order_drag(
    this: &mut ChartPanel,
    button: TradeMouseButton,
    cx: &mut Context<ChartPanel>,
) -> bool {
    let released = this.finish_order_drag(button, cx);
    if released {
        this.sync_native_cursor();
        cx.notify();
        cx.stop_propagation();
    }
    released
}

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
    // Count the press against the series THIS panel saw before a trading gesture reads it as a
    // double click; `e.click_count` pairs presses per window, blind to which chart received them.
    // The `<= 1` gates below stay on the NATIVE count: their question is "is this the second press
    // of a pair", which holds however the pair split between charts. The two that change live
    // state — cancelling an entry, grabbing an order line — additionally require a press this
    // panel is allowed to act on at all, because a press left over from a × is none of its
    // business either, however the window happens to count it. The grab carries its native answer
    // as an argument rather than a condition: its built-in half needs the gate, its configured
    // double-click gestures are exactly the presses the gate rejects.
    let clicks = press_count(
        this,
        MouseButton::Left,
        e.click_count,
        e.position,
        window,
        cx,
    );
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
    // The figure layer reacts only in drawing mode and only to the secondary modifier when starting
    // or grabbing a figure. An ordinary unmodified left click makes try_fig_click return false and
    // continues to trading/navigation, matching Moonbot even while drawing mode is enabled. Outside
    // drawing mode it also returns false immediately. secondary() is Command on macOS and Ctrl on
    // Windows/Linux; macOS Ctrl cannot be used because the OS converts Ctrl+left-click to a right
    // click before the drawing event arrives. An active draft continues without a modifier, so
    // Command/Ctrl is required only on the first click.
    // A Sells-to-zone band is the one draft that does NOT relax the modifier for its later clicks:
    // its finishing click sends a live bulk move, and an unmodified left click on a chart is the
    // trading/navigation gesture. Both of its clicks are held to Ctrl/Command — here for the press
    // and in `try_fig_release` for the drag gesture — which is also how Moonbot's own rectangle is
    // drawn.
    //
    // While the MODE is armed the click-count gate lifts for the press that STARTS a band: band
    // after band is drawn in it, and beginning the next one where the last ended lands inside the
    // system's double-click box, which would otherwise send that press to the trading gestures
    // below instead of to the figure layer. It does NOT lift for the press that FINISHES one — that
    // press sends a live bulk move, and an accidental Ctrl+double-click must not be what sends it.
    let sells_zone_mode = this.sells_zone_armed(cx);
    let starting_band = sells_zone_mode && this.fig_draft.is_none();
    if within
        && (e.click_count <= 1 || starting_band)
        && this.try_fig_click(
            pos,
            e.modifiers.secondary()
                || this
                    .fig_draft
                    .as_ref()
                    .is_some_and(|draft| !draft.needs_modifier()),
            cx,
        )
    {
        cx.notify();
        cx.stop_propagation();
        return;
    }
    // The press was not the figure layer's, so the drag-release gesture must not be measured
    // against it: a draft's `down` says "the figure layer accepted this press and still holds it",
    // and it is written only by an accepted press. Leaving a stale one behind would let a press
    // refused here — over the order book, outside a pane, or a double-click, which skips the figure
    // layer entirely — release into the plot and finish a figure. For a Sells-to-zone band that
    // release sends a live bulk move; for a gesture-completed tool it invents the vertices the drag
    // was never asked for; and the press that WAS accepted meanwhile pans the chart or drags an
    // order line, which is the pointer the preview would otherwise follow.
    if let Some(d) = this.fig_draft.as_mut() {
        d.down = None;
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
    // Second, the TRADING gestures are off while the Sells-to-zone mode is armed: the mode is a
    // drawing posture — the badge and the tool picker both say so — and a press meant for a band
    // must not place or cancel an order instead. That covers the order book too, whose click also
    // reaches `try_place_order_click`. Deliberately narrower than swallowing the press outright:
    // panning and the open-on-Main double click keep working, so reaching the part of the chart the
    // next band belongs on does not need leaving the mode.
    if within
        && !sells_zone_mode
        && clicks.is_some_and(|count| {
            this.try_place_order_click(TradeMouseButton::Left, e.modifiers, count, pos, cx)
        })
    {
        cx.stop_propagation();
        return;
    }
    // Clicking the start cross of an unfilled entry cancels it before drag handling, so dragging
    // never starts from that cross.
    if within
        && !sells_zone_mode
        && clicks.is_some()
        && e.click_count <= 1
        && this.try_cancel_order_click(pos, cx)
    {
        cx.notify();
        cx.stop_propagation();
        return;
    }
    // Not gated on `click_count <= 1` like the branches above: a move gesture may be bound to a
    // double click (`CTRL_Dbl` and friends). The native gate is not dropped, it is passed along —
    // the built-in plain-left grab still requires it, and only double-click gestures pass it.
    if within && !sells_zone_mode && grab_order_line(this, TradeMouseButton::Left, e, clicks, pos, cx)
    {
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
        let workspace_group = this.workspace_group.clone();
        opened_to_main = this.backend.update(cx, |b, bcx| {
            if !b.workspace_action_allows_core(workspace_group.as_deref(), core) {
                return false;
            }
            b.open_on_main((core, market), true);
            bcx.notify();
            true
        });
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
    // Before every early return below: the release ends the gesture whichever branch takes it.
    settle_paced_drag(this, cx);
    // A draw-drag-release gesture (Command/Ctrl down, drag, release) completes a segment/channel
    // without a second click. A stationary click is not a drag gesture and waits for click two.
    if let Some((pos, _)) = this.chart_local(e.position) {
        if this.try_fig_release(pos, e.modifiers.secondary(), cx) {
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
    if release_order_drag(this, TradeMouseButton::Left, cx) {
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
    // See `mouse_down_left`: trading matches the presses this panel saw, not the window's.
    let clicks = press_count(
        this,
        MouseButton::Right,
        e.click_count,
        e.position,
        window,
        cx,
    );
    // The flag means "swallow the release paired with THIS press", so a new press starts without
    // one. It is cleared here rather than where a gesture is abandoned: a drag dropped by the
    // pointer leaving the slot gets no release at all, and clearing it there would have to trust
    // the fork's non-client mouse moves, which report no pressed button DURING a live drag.
    this.suppress_rmb_up = false;
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
    // Right-clicking a drawn figure in drawing mode opens its Alert/Delete menu. This has highest
    // priority; suppress_rmb_up consumes the paired release so fullscreen remains intact.
    if within && this.try_open_figure_menu(pos, e.position, window, cx) {
        this.suppress_rmb_up = true;
        cx.stop_propagation();
        return;
    }
    // A move gesture bound to the right button grabs the line BEFORE the order menu, which would
    // otherwise swallow every right press over a line. Nothing is bound to the right button by
    // default, so the menu keeps the plain right click. A right DOUBLE-click move gesture stays out
    // of reach over a line: press one opens the menu, whose overlay consumes press two — the menu
    // is the older contract and a gesture nobody has bound is not worth deferring it for.
    if within && grab_order_line(this, TradeMouseButton::Right, e, clicks, pos, cx) {
        this.suppress_rmb_up = true;
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
        && clicks.is_some_and(|count| {
            this.try_place_order_click(TradeMouseButton::Right, e.modifiers, count, pos, cx)
        })
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
    // A right-button drag zooms Y and is paced like any other, but `mouse_button(Right, false, ..)`
    // always reports unchanged, so this release would otherwise never settle what the pacer owed.
    settle_paced_drag(this, cx);
    // A right-button move gesture ends on this release, so finishing comes BEFORE the suppression
    // check below — that branch returns early for exactly the press that started this drag. It
    // clears the flag itself, because the release it was armed for is this one. A drag owned by
    // another button is left alone: `finish_order_drag` reports `false` and this release keeps its
    // ordinary behavior.
    if release_order_drag(this, TradeMouseButton::Right, cx) {
        this.suppress_rmb_up = false;
        return;
    }
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
    window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
    // See `mouse_down_left`: trading matches the presses this panel saw, not the window's.
    let clicks = press_count(
        this,
        MouseButton::Middle,
        e.click_count,
        e.position,
        window,
        cx,
    );
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
        && clicks.is_some_and(|count| {
            this.try_place_order_click(TradeMouseButton::Middle, e.modifiers, count, pos, cx)
        })
    {
        cx.stop_propagation();
        return;
    }
    // A move gesture bound to the middle button, before the X-scale synchronization below claims
    // Shift+middle for itself.
    if within && grab_order_line(this, TradeMouseButton::Middle, e, clicks, pos, cx) {
        return;
    }
    // Shift+middle-click on the graph synchronizes the time X scale across charts in THIS window,
    // matching Moonbot. A trading gesture bound to Shift+middle-click takes priority above.
    if within && e.modifiers.shift && this.sync_x_scale_window(window, cx) {
        cx.stop_propagation();
    }
}

/// Finishes a middle-button order drag.
///
/// The middle button has no other release behavior, and without this handler a drag started by a
/// middle-button move gesture would reach no release path at all: the next pointer move with no
/// button held DROPS the drag silently, so the moved line would spring back and no `move_order`
/// would ever be sent.
pub(super) fn mouse_up_middle(
    this: &mut ChartPanel,
    _e: &MouseUpEvent,
    _window: &mut Window,
    cx: &mut Context<ChartPanel>,
) {
    settle_paced_drag(this, cx);
    release_order_drag(this, TradeMouseButton::Middle, cx);
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
        // No button held means no press held either, including one whose mouse-up never reached a
        // handler — released outside the slot, or stolen with the capture. A draft's `down` is the
        // figure layer's record of an accepted, still-held press, so it recovers here the same way
        // `sync_pressed` recovers the chart's own drag state, rather than staying true until the
        // next press happens to overwrite it.
        if let Some(d) = this.fig_draft.as_mut() {
            d.down = None;
        }
        crate::diag::bump(&crate::diag::CHART_MOUSE_MOVE_FAST);
        // A move with no button held normally means the drag is over — including one whose release
        // landed outside this slot and so reached no handler at all, both being hitbox-gated. Only
        // from motion INSIDE the chart, for the same reason the figure-drag rescue just above is:
        // the Windows fork reports non-client mouse moves with no pressed button DURING a live
        // drag, and settling on those would reset the pacer on every one of them and restore the
        // full event-rate notify this exists to stop.
        if within {
            settle_paced_drag(this, cx);
        }
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
        // Trade arrows: gated by the same movement threshold, then a plot-bounds test that fails
        // for almost every pointer position before anything scans the clusters.
        if this.sync_trade_hover(pos, within, cx) {
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
    // A draft under a held LEFT button is a press-drag-release gesture, and without this its
    // preview froze at the press: the fast path above runs only for a move with no button down,
    // and the branch just above only for a drag of an EXISTING figure. Deliberately without an
    // early return of its own — the crosshair, the native cursor and the chart's own navigation are
    // the common path's below, and a draft, unlike a figure drag, does not own the pointer.
    // Gated on the left button specifically: a draft outlives its clicks, and the right button
    // meanwhile zooms price, which is not this gesture. And on the draft holding a press — its own
    // record that the figure layer accepted the one being held — because a press the layer REFUSED
    // (the order-book strip, a double-click) drags an order line or pans the chart, and this would
    // otherwise paint a preview following that same pointer.
    if this
        .fig_draft
        .as_ref()
        .is_some_and(|d| d.down.is_some())
        && e.pressed_button == Some(MouseButton::Left)
    {
        this.update_fig_pointer(pos, within, true, cx);
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
    //
    // PACED, because one notify here is not one repaint of this panel: `mark_view_dirty` marks
    // every ancestor and a re-rendered root bypasses each descendant's view cache, so it repaints
    // the WHOLE window. Measured under a drag, `shell_render` tracked `chart_render` exactly, both
    // at the mouse-event rate — around 110 a second. The chart's own pass cannot show more than
    // `present_rate_hz` anyway, so anything above that is a window repaint nobody can see. The
    // camera itself is NOT paced: the drag has already moved it above, and the own-pass presents it
    // on its own tick without the GPUI tree.
    if dragging {
        // The rate this yields is the event rate quantized DOWN to the grid, not exactly the
        // grid — the stamp restarts at each notify rather than advancing by a whole interval. The
        // undershoot is in the direction we want and it never bursts after a pause.
        let interval = DRAG_NOTIFY_MIN_INTERVAL;
        let now = Instant::now();
        if this
            .drag_notify_at
            .is_none_or(|last| now.duration_since(last) >= interval)
        {
            this.drag_notify_at = Some(now);
            this.drag_notify_pending = false;
            crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
            cx.notify();
        } else {
            crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY_PACED);
            this.drag_notify_pending = true;
        }
    }
}

/// Settle a drag move the pacer dropped, at the end of the gesture that owed it.
///
/// Called from EVERY release path, before their own early exits. The debt is not this panel's
/// alone: compare-lock followers take their locked Y from an observer on this entity's notify
/// (`chart_tabs/main_stack.rs`), with no render path that would catch up later, so a swallowed
/// final move leaves sibling charts on a stale scale.
fn settle_paced_drag(this: &mut ChartPanel, cx: &mut Context<ChartPanel>) {
    this.drag_notify_at = None;
    if std::mem::take(&mut this.drag_notify_pending) {
        crate::diag::bump(&crate::diag::CHART_INPUT_NOTIFY);
        cx.notify();
    }
}

/// Tracks chart-slot enter/leave state and clears cursor/order interaction on leave.
pub(super) fn hover(
    this: &mut ChartPanel,
    hovered: &bool,
    window: &mut Window,
    _cx: &mut Context<ChartPanel>,
) {
    // Track the chart under the pointer for cursor-dependent new_long/new_short hotkeys. Enter/leave
    // is infrequent rather than per-pixel, and the backend update omits notify to avoid rendering.
    let self_id = _cx.entity_id();
    let weak = _cx.entity().downgrade();
    let hov = *hovered;
    // Recorded on ENTER only, under this panel's own OS window: the chart shot resolves through it
    // once the pointer has left, and it must not answer a keystroke that arrived at a DIFFERENT
    // window. Dead entries are dropped on the way past - a closed chart leaves a weak handle that
    // no longer upgrades, and a closed window leaves one nothing will ever ask for again.
    let window_handle = window.window_handle();
    this.backend.update(_cx, |b, _| {
        if hov {
            b.hovered_chart = Some(weak.clone());
            b.last_chart.retain(|_, chart| chart.upgrade().is_some());
            b.last_chart.insert(window_handle, weak);
        } else if b.hovered_chart.as_ref().map(|w| w.entity_id()) == Some(self_id) {
            b.hovered_chart = None;
        }
    });
    if !*hovered {
        // The pointer leaving is the last event this panel is guaranteed while a gesture may still
        // be owed a repaint: a release outside the slot reaches neither mouse-up nor mouse-move,
        // both of which are hitbox-gated. Settling here costs nothing when nothing is owed.
        settle_paced_drag(this, _cx);
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
        if this.clear_trade_hover(_cx) {
            _cx.notify();
        }
        let changed =
            this.input.cursor.take().is_some() || this.input.hovered_pane.take().is_some();
        if changed {
            this.sync_native_cursor();
        }
    }
}
