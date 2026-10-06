//! Main chart tab: each market owns a separate `ChartPanel`/`gpu_canvas`; the active chart is
//! fullscreen, and right-clicking its chart plot expands the whole stack. The shared stack
//! renderer lives in [`super::stack`].

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{MoonPalette, MoonTabItem, MoonTabStrip, MoonVirtualListScrollHandle, v_flex};

use super::stack::grid;
use super::stack::{
    ChartStackEntry, SlotOwner, apply_setting, chart_stack_card, compare_role, force_panels_scale,
    render_chart_stack, resolve_layout, retain_nonempty_panels, set_panels_auto_pin,
    set_panels_candle_view, set_panels_chart_graphics, set_panels_chart_labels,
    set_panels_cursor_labels, set_panels_line_labels, set_panels_orderbook_enabled,
    set_panels_price_axis_pos, set_panels_scale, set_panels_show_zone,
    set_panels_time_axis_visible, sync_compare, tile_gutter,
};
use crate::Backend;
use crate::panels::ChartPanel;
use crate::persistence::chart_persist::{PriceAxisPos, StackLayoutMode, StackOrientation};
use moon_core::config::ChartTheme;
use moon_core::config::layout::EmptyPlaces;
use moon_core::session::CoreId;

/// Main tab where each market owns a separate `ChartPanel`/`gpu_canvas`.
/// A regular market click in a table opens or focuses it fullscreen. Right-clicking the current
/// chart plot toggles between fullscreen and the whole stack; the order-book/control zone retains
/// its trading actions. Multiple markets never return to one `ChartEngine`.
pub(crate) struct MainChartStack {
    backend: Entity<Backend>,
    group: String,
    epoch: f64,
    theme: ChartTheme,
    charts: Vec<ChartStackEntry>,
    active: Option<usize>,
    show_stack: bool,
    scale: Option<f32>,
    /// Per-tab layout mode (`Fit`/`Scroll`); `None` uses the default `Fit` mode.
    layout_mode: Option<StackLayoutMode>,
    /// Slot height in `Fit` mode: zero stretches, while values of at least 20 compress.
    layout_height_fit: Option<u16>,
    /// Slot height in `Scroll` mode; `None` uses the default.
    layout_height_scroll: Option<u16>,
    /// Per-window order-book visibility for this tab; `None` defaults to enabled.
    orderbook_enabled: Option<bool>,
    /// Candle and trade display settings for the tab; `None` uses the global default.
    candle_view: Option<moon_core::market::CandleViewCfg>,
    /// Chart-drawing settings for the tab; `None` uses the global `layout.chart_graphics` default.
    chart_graphics: Option<moon_core::config::ChartGraphicsCfg>,
    /// Chart captions for the tab; `None` uses the default of the tab's KIND — the main chart is
    /// a comparison while the anchor lock is on, and then follows the comparison set.
    chart_labels: Option<moon_core::config::ChartLabelsCfg>,
    /// Captions a panel's own right-click menu produced, on their way to the host that persists
    /// them. See `panels::chart::volume_menu` for why they travel rather than being written here.
    pending_labels: Option<moon_core::config::ChartLabelsCfg>,
    /// Window X scale in px/ms, synchronized with Shift+middle-click; `None` uses the built-in default.
    /// New charts inherit it, and synchronization applies it to every chart.
    x_ppm: Option<f32>,
    /// Per-window control-zone fill visibility; `None` defaults to enabled.
    show_zone: Option<bool>,
    /// Per-window automatic chart pinning when an order is placed; `None` defaults to disabled.
    auto_pin: Option<bool>,
    /// Per-window stack orientation; `None` defaults to `Vertical`.
    layout_orientation: Option<StackOrientation>,
    /// Per-window price-axis position for stack charts; `None` defaults to `Left`.
    price_axis_pos: Option<PriceAxisPos>,
    /// Per-window time-axis visibility for stack charts; `None` defaults to enabled.
    time_axis_visible: Option<bool>,
    /// Per-window line-label visibility for stack charts; `None` defaults to enabled.
    line_labels: Option<bool>,
    /// Per-window crosshair-label visibility for stack charts; `None` defaults to enabled.
    cursor_labels: Option<bool>,
    /// Comparison anchor `(core, market)` that leads the price scale; `None` disables comparison.
    /// The kind last pushed down to the panels, so an unchanged one costs nothing.
    pushed_kind: Option<moon_core::config::ChartTabKind>,
    compare_anchor: Option<(CoreId, String)>,
    /// Shared comparison Y range copied from the anchor's current Y window.
    compare_y: Option<(f32, f32)>,
    /// Broom mode makes the anchor's neighbors show only their order books.
    compare_orderbook_only: bool,
    /// The crowd statistics drawn on the empty screen, while at least one of their tables is
    /// switched on and there is an empty screen to draw them on. See [`empty`].
    ///
    /// Held rather than rebuilt per frame because it owns the market feed: it exists exactly while
    /// it is visible, and dropping it is what closes the connections. The set of tables it was
    /// built for is kept beside it, because that set decides which connections exist — a change
    /// there rebuilds the view rather than being passed to it.
    crowd: Option<(
        crate::crowd::CrowdParts,
        Entity<crate::crowd::CrowdStatsView>,
    )>,
    /// Whether the empty screen's ⚙ popup is open.
    empty_settings_open: bool,
    /// The crowd rule's two threshold fields, built the first time that popup is opened. See
    /// [`empty::DetectInputs`]: a text field needs a `Window`, and this stack is built without one.
    empty_detect: Option<empty::DetectInputs>,
    /// The five dropdowns that place the blocks of the empty screen, built with the same window as
    /// the rule's fields beside them and kept for the same reason: a `MoonSelectState` cannot be
    /// rebuilt per frame without throwing away an open menu.
    empty_places: Option<empty::arrange::PlaceSelects>,
    /// Whether the one-shot inactivity auto-close timer is armed.
    /// It ticks at about 1 Hz while configured and charts exist, then rearms itself.
    idle_timer_armed: bool,
    /// Screen divider for the expanded stack: columns, whether the number is exact, and the
    /// smallest slot the divider works to in FIT-stretch. Fullscreen ignores all three — one chart
    /// full bleed is not a grid.
    layout_columns: Option<u8>,
    layout_columns_exact: Option<bool>,
    layout_min_slot: Option<u16>,
    /// Size the stack was last painted at, written by the render probe. See `AddChartStack`.
    measured: Rc<Cell<Size<Pixels>>>,
    /// Whether the host presenting this stack is on screen.
    ///
    /// The dock writes it through [`Self::set_scene_visible`]. Paint stores `true` because a
    /// rendered stack is present. Prune and virtual-list callbacks AND this flag, so a hidden
    /// host cannot be switched back on by a local visibility update.
    host_visible: bool,
    scroll: MoonVirtualListScrollHandle,
}

/// Select one chart for Escape and update Main presentation state for the remaining chart count.
///
/// Args:
///     active: Current active index, updated to the chart replacing the removed slot.
///     show_stack: Current presentation mode, updated to stack when charts remain.
///     chart_count: Number of charts before removal.
///
/// Returns:
///     Index to remove, or `None` when the stack is empty.
fn take_active_close_index(
    active: &mut Option<usize>,
    show_stack: &mut bool,
    chart_count: usize,
) -> Option<usize> {
    if chart_count == 0 {
        *active = None;
        *show_stack = false;
        return None;
    }
    let index = active.filter(|&i| i < chart_count).unwrap_or(0);
    let remaining = chart_count - 1;
    *active = (remaining > 0).then_some(index.min(remaining.saturating_sub(1)));
    *show_stack = remaining > 0;
    Some(index)
}

/// Decide what a right-click on the chart body does to Main's presentation mode.
///
/// Fullscreen means "this chart alone, full bleed"; stack means "all of them, tiled". With a
/// single chart the two render the same content, so flipping into stack presentation changes
/// nothing the user asked for and only shifts the chart by the gutter the tiled layout adds — a
/// vertical jump with no visible cause. Returning to fullscreen FROM the stack stays available at
/// any count, because a stack can be left holding one chart after the others expire.
///
/// Args:
///     show_stack: Current presentation mode.
///     chart_count: Number of charts the stack holds.
///
/// Returns:
///     The new `show_stack` value, or `None` when the gesture has nothing to change.
fn stack_toggle_target(show_stack: bool, chart_count: usize) -> Option<bool> {
    if chart_count < 2 {
        return show_stack.then_some(false);
    }
    Some(!show_stack)
}

/// Resolve an active index after entries were reordered while preserving the active identity.
///
/// Args:
///     item_count: Number of entries after reordering.
///     active_key: Stable identity of the entry that was active before reordering.
///     fallback: Previous numeric index used only when the identity disappeared.
///     matches: Predicate that checks whether an index still owns the active identity.
///
/// Returns:
///     Remapped active index, a bounded fallback, or `None` for an empty stack.
fn remap_active_index<T>(
    item_count: usize,
    active_key: Option<&T>,
    fallback: Option<usize>,
    mut matches: impl FnMut(usize, &T) -> bool,
) -> Option<usize> {
    if item_count == 0 {
        return None;
    }
    active_key
        .and_then(|key| (0..item_count).find(|&ix| matches(ix, key)))
        .or_else(|| Some(fallback.unwrap_or(0).min(item_count - 1)))
}

mod close;
mod focus;
mod open;
mod prefs;
mod render;
mod tabs;

mod empty;

pub(crate) use empty::{crowd_cards, crowd_rule_for_run, empty_logo};

#[cfg(test)]
mod tests;
