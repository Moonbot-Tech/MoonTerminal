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
//! Configured shortcuts use GPUI's `Keystroke::parse` syntax. Shipped keyboard defaults are
//! Moonbot's own keys, read off its Hotkeys page — mostly `alt-`, with `ctrl-` where Moonbot uses it
//! and for the Terminal's own drawing tools, which Moonbot has no equivalent of. The literal
//! modifier is used on both Windows and macOS, as Moonbot does; bindings that need no modifier keep
//! their bare function-key or Delete forms.

mod layout;
#[cfg(test)]
mod tests;

use gpui::{App, Context, Entity, KeyDownEvent, Keystroke, Modifiers};
use moon_core::config::{HotkeysConfig, SPLIT_ORDER_PARTS};
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
    /// Spread the active market's sell orders across the drawn rectangle — Moonbot's
    /// "sells to rectangle", the take-profit grid stretched over a box.
    SellsToRect,
    /// Split a sell order for the active chart's market into `parts`.
    ///
    /// The count comes from the binding that matched: plain Split Order always splits into
    /// [`SPLIT_ORDER_PARTS`], as Moonbot does, while `Split N` uses the configured `split_parts`.
    SplitOrder { parts: i32 },
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
///
/// A letter is compared against the PHYSICAL key as well as the name the platform gave it, so a
/// binding does not die when the keyboard layout changes — see [`layout::us_letter`].
fn pressed(raw: &str, ev: &KeyDownEvent) -> bool {
    let raw = raw.trim();
    if raw.is_empty() {
        return false;
    }
    let Ok(k) = Keystroke::parse(raw) else {
        return false;
    };
    k.modifiers == ev.keystroke.modifiers
        && (k.key == ev.keystroke.key
            || layout::us_letter(&ev.keystroke.key).is_some_and(|physical| k.key == physical))
}

/// Resolve a key-down event to the action bound to it.
pub fn resolve(ev: &KeyDownEvent, hk: &HotkeysConfig) -> Option<HotkeyAction> {
    resolve_binding(ev, hk)
}

impl HotkeyAction {
    /// Whether a held key's repeats must NOT re-run this action.
    ///
    /// The line is what one repeat COSTS. Everything that creates, multiplies or closes something
    /// pays per press and nothing downstream deduplicates it — moonproto gives these commands no
    /// unique key — so a key left held would queue one order, one split or one market sell per OS
    /// repeat. A TOGGLE is worse than useless repeated: it flaps at the repeat rate, and a figure
    /// alert flaps on the wire, upserting and deleting a core-side object tens of times a second.
    /// Cycling through the drawing tools at that rate is the same kind of nonsense.
    ///
    /// The ones deliberately left repeating: cancels (the second press finds nothing left to
    /// cancel), the presets (setting a value twice sets it once) and the order SHIFTS, whose whole
    /// point is to nudge a line while the key is held.
    fn suppress_on_repeat(self) -> bool {
        matches!(
            self,
            Self::SplitOrder { .. }
                | Self::SellsToRect
                | Self::NewLong
                | Self::NewShort
                | Self::JoinSells
                | Self::PanicSell
                | Self::PanicSellOne
                | Self::FigAlert
                | Self::FigTool(_)
                | Self::SwitchFigure
        )
    }
}

/// Resolve a key-down event to the first matching configured or built-in action.
///
/// Branch order defines collision precedence: configured figure actions; built-in Shift+Escape,
/// Escape, reset, and Tab/Delete; configured scale actions; order-size and fixed-sell presets;
/// active-market and active-core trading actions; configured `switch_charts`; then manual
/// strategies. Returns `None` when no binding matches.
fn resolve_binding(ev: &KeyDownEvent, hk: &HotkeysConfig) -> Option<HotkeyAction> {
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
    // Split Order splits into a fixed three, as Moonbot does; Split Order X into the configured
    // count. Repeats of a held key are dropped for both by `pre_dispatch`.
    if p(&hk.split_order) {
        return Some(A::SplitOrder {
            parts: SPLIT_ORDER_PARTS,
        });
    }
    if p(&hk.split_order_x) {
        return Some(A::SplitOrder {
            parts: hk.split_n_parts(),
        });
    }
    if p(&hk.sells_to_rect) {
        return Some(A::SellsToRect);
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

/// Rewrite a recorded keystroke onto the physical key, so the settings file is layout-independent.
///
/// A key recorded under a Cyrillic layout arrives named `"ф"`; stored as such it would work only in
/// that layout, and the file could not be shared with anyone using another one. Recording is where
/// this belongs: matching accepts both forms anyway, and the user sees the Latin letter they
/// physically pressed.
pub fn recorded_keystroke(mut keystroke: Keystroke) -> Keystroke {
    if let Some(physical) = layout::us_letter(&keystroke.key) {
        keystroke.key = physical;
    }
    keystroke
}

/// Cancel the order under the cursor through `Backend::hovered_chart`.
///
/// This is shared by the built-in Tab/Delete route and the caller's `FigDelete` fallback when no
/// figure is selected. The default `fig_delete = Delete` resolves before the built-in branch and
/// would otherwise shadow hovered-order cancellation. Returns `false` when no hovered chart or
/// order exists, allowing the key event to continue propagating.
pub fn cancel_hovered_order(backend: &Entity<Backend>, cx: &mut App) -> bool {
    with_hovered_chart(backend, cx, |panel, pcx| panel.cancel_hovered_order(pcx))
}

/// Run `f` against the globally hovered chart panel, if there is one.
///
/// Cursor-addressed actions all need the same lookup: the chart under the pointer, whichever window
/// holds it, upgraded from the weak handle the backend keeps. `false` means no chart is hovered.
fn with_hovered_chart(
    backend: &Entity<Backend>,
    cx: &mut App,
    f: impl FnOnce(&mut crate::panels::ChartPanel, &mut Context<crate::panels::ChartPanel>) -> bool,
) -> bool {
    let chart = backend
        .read(cx)
        .hovered_chart
        .clone()
        .and_then(|w| w.upgrade());
    match chart {
        Some(chart) => chart.update(cx, f),
        None => false,
    }
}

/// Apply the policies that belong to the action itself, before a window's own routing.
///
/// Every window's `on_hotkey` calls this once, ahead of its own match, so a third window inherits
/// both rules instead of restating them. `true` means the event is spent and the caller stops.
///
/// - **Auto-repeat**: a held key repeats at the system rate. That is harmless for a toggle and
///   multiplies anything that creates orders, so a repeat of such an action is consumed and does
///   nothing — consumed rather than merely ignored, or the repeats of a bound key would sail on to
///   whatever else that key means in the window.
/// - **Cursor-addressed target**: a hotkey has no click, so the order the pointer rests on is the
///   one the user means, the rule the built-in Tab/Delete cancellation already follows. Split needs
///   it; with nothing hovered it falls through to [`apply`]'s market-level split, which the core
///   performs only when the market has exactly one active sell order.
pub fn pre_dispatch(
    action: HotkeyAction,
    ev: &KeyDownEvent,
    backend: &Entity<Backend>,
    cx: &mut App,
) -> bool {
    if ev.is_held && action.suppress_on_repeat() {
        return true;
    }
    match action {
        HotkeyAction::SplitOrder { parts } => {
            with_hovered_chart(backend, cx, |panel, pcx| {
                panel.split_hovered_order(parts, pcx)
            })
        }
        // The drawn band belongs to the chart it was drawn on, so the pointer picks the chart. With
        // the pointer elsewhere this falls through to the window's own active market.
        HotkeyAction::SellsToRect => {
            with_hovered_chart(backend, cx, |panel, pcx| panel.sells_to_rect_at_cursor(pcx))
        }
        _ => false,
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
            // action. This action never exits drawing mode; the Cursor entry or the active-tool toggle does.
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
        A::SplitOrder { parts } => match target {
            Some((core, market)) => {
                if let Err(error) = b.session.split_order_for_market(core, market, parts) {
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
        A::SellsToRect => match target {
            Some((core, market)) => sells_to_rect(b, core, &market),
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

/// Spread the market's sell orders across the price band drawn on its chart.
///
/// Two tools draw a band and both count: the **Zone** (`Channel`), which is Moonbot's own
/// chart-object type 5 and therefore the band a core can also send us, and the local **Rect**.
/// Every other tool is a single line or level and would only let this guess at a band the user did
/// not draw. The band is the SELECTED figure when the selection is one of those two on this chart,
/// and otherwise the most recently drawn one. Returns `false` when there is none, leaving the key
/// unhandled rather than sending a zone nobody asked for.
pub(crate) fn sells_to_rect(b: &mut Backend, core: CoreId, market: &str) -> bool {
    let zone = {
        let store = b.figures.borrow();
        let selected = b
            .fig_selected
            .as_ref()
            .filter(|(fig_core, fig_market, _)| *fig_core == core && fig_market == market)
            .and_then(|(_, _, id)| store.get(core, market, *id))
            .and_then(figure_zone);
        selected.or_else(|| {
            // With nothing selected, only an UNAMBIGUOUS band is taken: exactly one on this chart.
            // "The newest of several" reads well and behaves badly — a forgotten box from last week,
            // scrolled far off screen, would quietly decide where live sells land. Several bands is
            // a question only the user can answer, by selecting one.
            //
            // `figures`, not `visible`: the latter also yields bands another core merely SHARES
            // onto this market, and a box drawn elsewhere is not this chart's answer either.
            let mut bands = store.figures(core, market).iter().filter_map(figure_zone);
            let only = bands.next()?;
            bands.next().is_none().then_some(only)
        })
    };
    let Some((a, z)) = zone else {
        log::warn!(
            "hotkey sells to rectangle: core={} market={market} has no single Zone/Rect to spread into — draw one or select it",
            moon_core::feed::core_label(core)
        );
        return false;
    };
    // Refuse a band no thinner than one price STEP of this market — the authoritative number, not a
    // constant picked here: below it every sell rounds onto the same level, which is the opposite of
    // spreading them. An unknown step falls back to demanding two distinct prices. Logged, like the
    // other refusals, because the ways this key can legitimately do nothing — no band, a flat one,
    // no sell order, a core that is not connected — are otherwise indistinguishable from a command
    // that went out, and this one moves live money.
    let thin = match b.session.market_source().price_step(core, market) {
        Some(step) => (a - z).abs() < step,
        None => a == z,
    };
    if thin || !(a.is_finite() && z.is_finite()) || a <= 0.0 || z <= 0.0 {
        log::warn!(
            "hotkey sells to rectangle: core={} market={market} zone {a:.8}..{z:.8} is thinner than one price step or out of range, nothing sent",
            moon_core::feed::core_label(core)
        );
        return true;
    }
    let short = b.market_position_short(core, market);
    log::info!(
        "hotkey sells to rectangle: core={} market={market} zone={:.8}..{:.8} short={short}",
        moon_core::feed::core_label(core),
        a.min(z),
        a.max(z),
    );
    if let Err(error) = b.session.sells_to_zone(core, market.to_string(), a, z, short) {
        log::warn!("hotkey sells to rectangle failed: {error}");
    }
    true
}

/// Return a figure's two band prices, or `None` for a tool that draws no band.
///
/// `Channel` is shown to the user as **Zone** and is Moonbot's own two-price object; `Rect` is the
/// local box. Both describe exactly the pair of prices this command needs.
fn figure_zone(fig: &moon_core::figures::Figure) -> Option<(f64, f64)> {
    use moon_core::figures::FigureKind;
    match &fig.kind {
        FigureKind::Rect(rect) => Some((rect.a.price, rect.b.price)),
        FigureKind::Channel(channel) => Some((channel.price1, channel.price2)),
        _ => None,
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
