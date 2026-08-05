//! Shared hotkey recognition and backend action execution for group windows and detached chart
//! windows. Key matching previously lived in scattered `pressed()` checks under
//! `Shell::on_hotkey`, leaving some configured bindings inactive and detached chart windows with
//! no handling. The shared flow is now:
//!
//! - [`resolve`] maps a key-down event plus configuration to a semantic [`HotkeyAction`], comparing
//!   only `modifiers` and `key` rather than full `Keystroke` equality;
//! - [`apply`] executes shared backend actions against the caller-supplied active core and chart
//!   market. Actions requiring caller-level context return `false`: scale belongs to the calling
//!   window; cursor placement and cancellation follow the globally tracked hovered chart;
//!   switching and active-chart closing are group-local; reset and close-all are application-global.
//!
//! Configured shortcuts use GPUI's `Keystroke::parse` syntax. Shipped keyboard defaults deliberately
//! use literal `ctrl-` on both Windows and macOS to match Moonbot; bindings that need no modifier
//! retain their bare function-key or Delete forms.

use gpui::{App, Context, Entity, KeyDownEvent, Keystroke, Modifiers};
use moon_core::config::HotkeysConfig;
use moon_core::feed::ClientSettingsEdit;
use moon_core::figures::FigureTool;
use moon_core::session::CoreId;
use moon_core::session::order_lines::LineKind;

use crate::Backend;

/// Semantic hotkey action independent of its configured key binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Select one of the current window group's order-size presets.
    OrderSize(usize),
    /// Select one of the current window group's fixed-sell slots, making that preset the effective
    /// take profit instead of the main TP setting.
    SellPreset(usize),
    /// Select the indexed manual strategy in the active core's header picker and enable manual
    /// strategy mode, matching a click on that picker item.
    ManualStrategy(usize),
    /// Cancel pending buy orders for the active chart's market.
    CancelBuy,
    /// Cancel all pending buy orders for the active core across every market.
    CancelAllBuys,
    /// Toggle panic sell for the active chart's market.
    PanicSell,
    /// Immediately close the active chart market's position with a market sell.
    PanicSellOne,
    /// Join sell orders for the active chart's market using its current position side.
    JoinSells,
    /// Move active-chart orders by one market price step through `move_order`.
    ///
    /// `sell = false` selects unfilled entry buy lines, using the same guard as line dragging;
    /// `sell = true` selects exit sell lines. `up` chooses the direction.
    ShiftOrder {
        /// Select exit sell lines when true, or entry buy lines when false.
        sell: bool,
        /// Add one price step when true, or subtract one when false.
        up: bool,
    },
    /// Split a sell order for the active chart's market into parts.
    SplitOrder,
    /// Place a manual long order at the hovered chart price.
    ///
    /// The caller routes this through `Backend::hovered_chart`, because only the chart can map the
    /// cursor's pane-relative Y coordinate to a price.
    NewLong,
    /// Place a manual short order at the hovered chart price through the caller.
    NewShort,
    /// Select or toggle a drawing tool in the global figure state.
    FigTool(FigureTool),
    /// Cycle to the next drawing tool and keep drawing mode enabled.
    SwitchFigure,
    /// Switch the group window's active fullscreen Main chart to the next chart.
    ///
    /// The group-window caller routes this through its revision mechanism; detached chart windows
    /// do not handle it.
    SwitchCharts,
    /// Delete the selected figure.
    FigDelete,
    /// Toggle the selected figure's chart alert.
    FigAlert,
    /// Close the group window's active Main chart with the built-in plain Escape binding.
    ///
    /// Matching Moonbot, Escape closes the chart without disabling drawing mode. The group-window
    /// caller executes this action; detached chart windows leave it unhandled.
    CloseActiveChart,
    /// Reset every application window to an on-screen position with built-in Ctrl+Shift+F10.
    ///
    /// The caller executes this because it requires application-wide window access.
    ResetWindows,
    /// Cancel the order under the cursor for the built-in unmodified Tab/Delete binding.
    ///
    /// The caller routes this through `Backend::hovered_chart`, whose `order_hover` state identifies
    /// the order. With no hovered order the action remains unhandled, allowing Tab focus navigation.
    CancelHoveredOrder,
    /// Close every group's Main-stack charts with the built-in Shift+Escape binding.
    ///
    /// The caller increments a global revision observed by every `ChartTabs` instance.
    CloseAllCharts,
    /// Zoom the active chart's Y scale inward through the calling window.
    ScalePlus,
    /// Zoom the active chart's Y scale outward through the calling window.
    ScaleMinus,
}

/// Return whether an event matches a configured GPUI keystroke string.
///
/// Empty or invalid strings do not match. Comparison uses only `modifiers` and `key`: Windows
/// events also carry `key_char`, while a parsed `Keystroke` does not, so full equality previously
/// prevented Ctrl-plus-letter bindings from matching.
fn pressed(raw: &str, ev: &KeyDownEvent) -> bool {
    let raw = raw.trim();
    !raw.is_empty()
        && matches!(
            Keystroke::parse(raw),
            Ok(k) if k.modifiers == ev.keystroke.modifiers && k.key == ev.keystroke.key
        )
}

/// Resolve a key-down event to the first matching configured or built-in action.
///
/// Branch order defines collision precedence: configured figure actions; built-in Shift+Escape,
/// Escape, reset, and Tab/Delete; configured scale actions; order-size and fixed-sell presets;
/// active-market and active-core trading actions; configured `switch_charts`; then manual
/// strategies. Returns `None` when no binding matches.
pub fn resolve(ev: &KeyDownEvent, hk: &HotkeysConfig) -> Option<HotkeyAction> {
    use HotkeyAction as A;
    let p = |raw: &str| pressed(raw, ev);

    // Drawing-layer bindings take precedence over built-ins and trading bindings.
    if p(&hk.draw_hline) {
        return Some(A::FigTool(FigureTool::HLine));
    }
    if p(&hk.draw_segment) {
        return Some(A::FigTool(FigureTool::Segment));
    }
    if p(&hk.draw_triangle) {
        return Some(A::FigTool(FigureTool::Triangle));
    }
    if p(&hk.draw_channel) {
        return Some(A::FigTool(FigureTool::Channel));
    }
    if p(&hk.switch_figure) {
        return Some(A::SwitchFigure);
    }
    if p(&hk.fig_delete) {
        return Some(A::FigDelete);
    }
    if p(&hk.fig_alert) {
        return Some(A::FigAlert);
    }
    // Shift-only Escape closes all Main stacks; the next branch matches modifier-free Escape.
    if ev.keystroke.key == "escape"
        && ev.keystroke.modifiers.shift
        && !ev.keystroke.modifiers.control
        && !ev.keystroke.modifiers.alt
        && !ev.keystroke.modifiers.platform
    {
        return Some(A::CloseAllCharts);
    }
    if ev.keystroke.key == "escape" && ev.keystroke.modifiers == Modifiers::default() {
        return Some(A::CloseActiveChart);
    }
    // Remaining built-in, non-configurable bindings.
    if p("ctrl-shift-f10") {
        return Some(A::ResetWindows);
    }
    if (ev.keystroke.key == "tab" || ev.keystroke.key == "delete")
        && ev.keystroke.modifiers == Modifiers::default()
    {
        return Some(A::CancelHoveredOrder);
    }

    // Window-local Y-scale bindings.
    if p(&hk.scale_plus) {
        return Some(A::ScalePlus);
    }
    if p(&hk.scale_minus) {
        return Some(A::ScaleMinus);
    }

    // Order-size and fixed-sell presets.
    if let Some(i) = hk.order_size.iter().position(|r| p(r)) {
        return Some(A::OrderSize(i));
    }
    if let Some(i) = hk.sell_preset.iter().position(|r| p(r)) {
        return Some(A::SellPreset(i));
    }

    // Order actions for the active chart's market.
    if p(&hk.cancel_buy) {
        return Some(A::CancelBuy);
    }
    if p(&hk.cancel_all_buys) {
        return Some(A::CancelAllBuys);
    }
    if p(&hk.panic_sell) {
        return Some(A::PanicSell);
    }
    if p(&hk.panic_sell_one) {
        return Some(A::PanicSellOne);
    }
    if p(&hk.join_sells) {
        return Some(A::JoinSells);
    }
    if p(&hk.split_order) || p(&hk.split_order_x) {
        return Some(A::SplitOrder);
    }
    if p(&hk.new_long) {
        return Some(A::NewLong);
    }
    if p(&hk.new_short) {
        return Some(A::NewShort);
    }
    if p(&hk.shift_buy_up) {
        return Some(A::ShiftOrder {
            sell: false,
            up: true,
        });
    }
    if p(&hk.shift_buy_down) {
        return Some(A::ShiftOrder {
            sell: false,
            up: false,
        });
    }
    if p(&hk.shift_sell_up) {
        return Some(A::ShiftOrder {
            sell: true,
            up: true,
        });
    }
    if p(&hk.shift_sell_down) {
        return Some(A::ShiftOrder {
            sell: true,
            up: false,
        });
    }
    if p(&hk.switch_charts) {
        return Some(A::SwitchCharts);
    }

    if let Some(i) = hk.manual_strategy.iter().position(|r| p(r)) {
        return Some(A::ManualStrategy(i));
    }
    None
}

/// Cancel the order under the cursor through `Backend::hovered_chart`.
///
/// This is shared by the built-in Tab/Delete route and the caller's `FigDelete` fallback when no
/// figure is selected. The default `fig_delete = Delete` resolves before the built-in branch and
/// would otherwise shadow hovered-order cancellation. Returns `false` when no hovered chart or
/// order exists, allowing the key event to continue propagating.
pub fn cancel_hovered_order(backend: &Entity<Backend>, cx: &mut App) -> bool {
    let chart = backend
        .read(cx)
        .hovered_chart
        .clone()
        .and_then(|w| w.upgrade());
    match chart {
        Some(chart) => chart.update(cx, |p, pcx| p.cancel_hovered_order(pcx)),
        None => false,
    }
}

/// Execute a shared backend action against the caller's trading context.
///
/// `target` is the calling window's active chart market as `(core, market)`, and `active_core` is
/// its active trading core. `group` owns size and exit hotkeys even when no core is live. `true`
/// means the action was handled and the caller should stop key propagation; for command-sending
/// actions it does not guarantee remote success. Actions that require caller-level window,
/// hovered-chart, group-revision, or application context return `false` for caller routing.
pub fn apply(
    action: HotkeyAction,
    b: &mut Backend,
    bcx: &mut Context<Backend>,
    group: &str,
    target: Option<(CoreId, String)>,
    active_core: Option<CoreId>,
) -> bool {
    use HotkeyAction as A;
    match action {
        A::FigTool(tool) => {
            // Repeating the active tool disables drawing; another tool selects it and enables drawing.
            if b.fig_draw_mode && b.fig_tool == tool {
                b.fig_draw_mode = false;
            } else {
                b.fig_tool = tool;
                b.fig_draw_mode = true;
            }
            bcx.notify();
            true
        }
        A::SwitchFigure => {
            // Cycle through tools and keep drawing mode enabled, matching Moonbot's switch-figure
            // action. This action never exits drawing mode; the pencil or active-tool toggle does.
            b.fig_tool = b.fig_tool.next();
            b.fig_draw_mode = true;
            bcx.notify();
            true
        }
        A::FigAlert => {
            // Consumed whenever a figure is selected, even if arming was refused — a tool the core
            // has no type for, or a figure shared across cores, refuses the toggle, and letting the
            // key fall through would be a surprise rather than a refusal. With NO selection the key
            // is not ours, exactly as `FigDelete` leaves it.
            if b.fig_selected.is_none() {
                false
            } else {
                if b.toggle_selected_figure_alert() {
                    bcx.notify();
                }
                true
            }
        }
        A::FigDelete => {
            if let Some((core, market, id)) = b.fig_selected.clone() {
                b.remove_figure(core, &market, id);
                bcx.notify();
                true
            } else {
                false
            }
        }
        A::OrderSize(i) => {
            b.set_order_size_sel(group, i);
            b.order_size_rev = b.order_size_rev.wrapping_add(1);
            bcx.notify();
            true
        }
        A::ManualStrategy(i) => match active_core {
            Some(core) => {
                if crate::controls::select_manual_strategy(b, core, i) {
                    bcx.notify();
                    true
                } else {
                    false
                }
            }
            None => false,
        },
        A::SellPreset(i) => {
            if b.edit_group_exit(group, ClientSettingsEdit::SelectFixedSellSlot(i + 1)) {
                b.order_size_rev = b.order_size_rev.wrapping_add(1);
                bcx.notify();
                true
            } else {
                false
            }
        }
        A::CancelBuy => match target {
            Some((core, market)) => {
                b.cancel_buy_orders(core, &market);
                true
            }
            None => false,
        },
        A::CancelAllBuys => match active_core {
            Some(core) => {
                b.cancel_all_buys_for_core(core);
                true
            }
            None => false,
        },
        A::PanicSell => match target {
            Some((core, market)) => {
                b.toggle_panic_sell(core, market);
                bcx.notify();
                true
            }
            None => false,
        },
        A::PanicSellOne => match target {
            Some((core, market)) => {
                if let Err(error) = b.session.market_sell_position(core, market) {
                    log::warn!("hotkey market sell position failed: {error}");
                }
                true
            }
            None => false,
        },
        A::JoinSells => match target {
            Some((core, market)) => {
                let short = b.market_position_short(core, &market);
                if let Err(error) = b.session.join_sells(core, market, short) {
                    log::warn!("hotkey join sells failed: {error}");
                }
                true
            }
            None => false,
        },
        A::SplitOrder => match target {
            Some((core, market)) => {
                if let Err(error) = b.session.split_order_for_market(core, market, 2) {
                    log::warn!("hotkey split order failed: {error}");
                }
                true
            }
            None => false,
        },
        A::ShiftOrder { sell, up } => match target {
            Some((core, market)) => shift_orders(b, core, &market, sell, up),
            None => false,
        },
        // Caller-routed actions have mixed scope: scale is window-specific; cursor placement and
        // cancellation use `Backend::hovered_chart`; switching and active-chart closing are
        // group-local; reset and close-all are application-global.
        A::ScalePlus
        | A::ScaleMinus
        | A::NewLong
        | A::NewShort
        | A::SwitchCharts
        | A::ResetWindows
        | A::CancelHoveredOrder
        | A::CloseAllCharts
        | A::CloseActiveChart => false,
    }
}

/// Move eligible active-market order lines by one configured market price step.
///
/// The function uses `move_order`, matching line dragging. Buy entries are eligible only while
/// unfilled; open sell exits remain eligible. It returns `false` when the price step is unavailable
/// or no eligible line has a finite positive current price. Once at least one eligible line is
/// found it returns `true`; non-positive destination prices are skipped and send errors are logged.
fn shift_orders(b: &mut Backend, core: CoreId, market: &str, sell: bool, up: bool) -> bool {
    let Some(step) = b.session.market_source().price_step(core, market) else {
        return false;
    };
    let kind = if sell { LineKind::Sell } else { LineKind::Buy };
    let mut moves: Vec<(u64, f64)> = Vec::new();
    if let Some(core_data) = b.session.store().core(core) {
        for order in core_data
            .order_lines
            .iter_market(market)
            .filter(|order| order.closed_ms.is_none())
        {
            // A filled entry is historical and cannot be replaced; this matches dragging in trade.rs.
            if !sell && order.fill_pct > 0.0 {
                continue;
            }
            if let Some(price) = order.lines[kind as usize]
                .current_price()
                .filter(|p| p.is_finite() && *p > 0.0)
            {
                moves.push((order.uid, f64::from(price)));
            }
        }
    }
    if moves.is_empty() {
        return false;
    }
    for (uid, price) in moves {
        let next = if up { price + step } else { price - step };
        if next <= 0.0 {
            continue;
        }
        if let Err(error) = b.session.move_order(core, uid, next) {
            log::warn!("hotkey shift order failed: uid={uid} price={next:.8}: {error}");
        }
    }
    true
}
