//! Shared chart-stack layer for Main and AddToChart: one entry type, scale and cleanup helpers, and
//! a three-mode FIT/SCROLL/COMPRESS layout parameterized by a tile factory. Main-specific behavior
//! such as fullscreen, active selection, and right-click return remains in `MainChartStack`.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonPalette, MoonScrollableElement, MoonScrollbarVisibility, MoonVirtualList,
    MoonVirtualListScrollHandle, h_flex, v_flex,
};

use crate::panels::ChartPanel;
use crate::persistence::chart_persist::{StackLayoutMode, StackOrientation};
use moon_core::session::CoreId;

/// One stack entry or slot containing a core market and its dedicated `ChartPanel`.
pub(super) struct ChartStackEntry {
    pub core: CoreId,
    pub market: String,
    pub panel: Entity<ChartPanel>,
    /// When the chart appeared in this slot. Handed to the panel's own pass, which draws the
    /// new-chart border flash from it, and also used by Main's idle-close deadlines.
    pub arrived_at: Instant,
    /// Whether this slot is empty after its chart closed or expired by TTL but retains its position.
    ///
    /// This applies only to COMPRESS (Fit with pixels): neighbors do not move or resize, a new chart
    /// occupies the first empty slot, and all slots reset once every slot is empty. It renders as a
    /// transparent placeholder.
    pub vacated: bool,
}

impl ChartStackEntry {
    /// Whether this entry IS the chart named by `key`.
    ///
    /// A chart is identified by `(core, market)` and never by its position: the stack renumbers
    /// whenever an entry expires or a comparison lock moves its anchor to the front, and an index
    /// taken before that stays in range while pointing at somebody else's market.
    pub(super) fn is(&self, key: &(CoreId, String)) -> bool {
        self.core == key.0 && self.market == key.1
    }

    pub(super) fn new(core: CoreId, market: String, panel: Entity<ChartPanel>) -> Self {
        Self {
            core,
            market,
            panel,
            arrived_at: Instant::now(),
            vacated: false,
        }
    }
}

/// Default Scroll slot size in pixels when the tab has no override.
pub(super) const DEFAULT_SCROLL_HEIGHT: u16 = 300;

/// Narrow comparison-follower width: order-book `GLASS_ZONE_PX` plus framing.
pub(super) const COMPARE_BOOK_W: f32 = moon_chart::GLASS_ZONE_PX + 2.0;

/// Minimum comparison-anchor slot width in FIT stretch mode (`width=0`).
///
/// The chart itself is at least 1.5 times the order book, plus the price axis and anchor's own order
/// book (`1.5 * GLASS + PRICE_AXIS_W + GLASS`). The anchor flexes and grows but not below this floor.
pub(super) const COMPARE_ANCHOR_MIN_W: f32 =
    moon_chart::GLASS_ZONE_PX * 2.5 + moon_chart::PRICE_AXIS_W;

/// Slot role for comparison-mode sizing; `Normal` uses ordinary sizing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CompareRole {
    Normal,
    Anchor,
    Follower,
}

/// Stable-open-count delay before COMPRESS collapses retained empty slots and lets remaining charts
/// expand into the released space. Any chart appearance or disappearance resets it; see
/// `AddChartStack::touch_count_change`.
pub(super) const COMPACT_STABLE: Duration = Duration::from_millis(5000);

const STACK_GUTTER: f32 = 8.0;
const STACK_HEADER_H: f32 = 20.0;

/// Whether one tile draws the gutter strip below itself.
///
/// The gutter separates tiles from one another, so a lone tile has nothing to separate from: drawn
/// there it is simply an 8px band of panel colour under the chart, and the chart body shrinks by
/// that much. That reads as a defect rather than as spacing — most visibly when right-click
/// switches a single chart between fullscreen and stack presentation, where the two views are then
/// identical except that one of them jumps.
///
/// Args:
///     fullscreen: Whether this is the single full-bleed chart, which never gutters.
///     tile_count: Number of tiles the stack lays out, vacated slots included — a retained empty
///         slot is still something to be separated from.
///
/// Returns:
///     Whether to draw the gutter.
pub(super) fn tile_gutter(fullscreen: bool, tile_count: usize) -> bool {
    !fullscreen && tile_count > 1
}

type VisibleRangeHandler = Box<dyn Fn(Range<usize>, &mut Window, &mut App)>;

/// Render the visual shell for one chart host in stack mode.
///
/// The body around `ChartPanel` deliberately has no `.bg()`. The chart own-pass renders through
/// UnderScene, so any opaque quad over the plot zone would hide it. Only the header, border, and a
/// separate gutter outside the plot zone receive color. The caller resolves `title_size` through
/// `design::t_body(cx)` because the `Clone + 'static` layout closure cannot capture `&App`.
/// `gutter` draws the 8px separator strip below the card; it exists to space STACKED tiles apart
/// and is dropped for a single full-bleed chart. `trailing` is an optional muted note pinned to the
/// header's right, such as which chart of how many is on screen.
#[allow(clippy::too_many_arguments)]
pub(super) fn chart_stack_card(
    id: SharedString,
    label: impl Into<SharedString>,
    panel: Entity<ChartPanel>,
    p: MoonPalette,
    border: Rgba,
    title_size: Pixels,
    gutter: bool,
    trailing: Option<SharedString>,
) -> Stateful<Div> {
    let label = label.into();
    let gutter_h = if gutter { STACK_GUTTER } else { 0.0 };
    div()
        .id(id)
        .w_full()
        .relative()
        .overflow_hidden()
        .when(gutter, |this| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(STACK_GUTTER))
                    .bg(rgb(p.gutter)),
            )
        })
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom(px(gutter_h))
                .overflow_hidden()
                .border_1()
                .border_color(border)
                .child(
                    h_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(STACK_HEADER_H))
                        .pl(px(11.0))
                        .pr(px(8.0))
                        .items_center()
                        .overflow_hidden()
                        .bg(rgb(p.panel_head))
                        .border_b_1()
                        .border_color(border)
                        .child(
                            div()
                                .font_family(crate::design::mono())
                                .text_size(title_size)
                                .text_color(rgb(p.text_soft))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(label),
                        )
                        .children(trailing.map(|note| {
                            div()
                                .ml_auto()
                                .flex_none()
                                .pl(px(8.0))
                                .font_family(crate::design::mono())
                                .text_size(title_size)
                                .text_color(rgb(p.text_muted))
                                .whitespace_nowrap()
                                .child(note)
                        })),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(STACK_HEADER_H))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .overflow_hidden()
                        // Do NOT wrap the panel in `AnyView::cached(size_full())` here. It looks
                        // like the obvious barrier — it is what MoonUI's dock puts around every
                        // panel — and it does cut the arrival flash's cost (chart_render during a
                        // pulse fell 33/s to 11/s, measured). But it also drove shell_render and
                        // orders_render from 5/s to 73/s during a mouse storm, all three counters
                        // moving in lockstep, i.e. the whole window re-rendering per frame. That
                        // trade is an order of magnitude the wrong way and FireTest fails on it.
                        // Measured 2026-07-31; the mechanism was not chased down.
                        .child(panel),
                ),
        )
}

/// Resolve per-tab stack layout settings into `(scroll, compress, slot_size)`.
///
/// - `Fit` with size zero stretches slots to share the window: `(false, false, _)`.
/// - `Fit` with size at least 20 selects COMPRESS with fixed size and no scrolling:
///   `(true, true, h)`.
/// - `Scroll` uses fixed size with scrolling: `(true, false, h)`.
pub(super) fn resolve_layout(
    mode: Option<StackLayoutMode>,
    height_fit: Option<u16>,
    height_scroll: Option<u16>,
) -> (bool, bool, f32) {
    match mode.unwrap_or(StackLayoutMode::Fit) {
        StackLayoutMode::Fit => {
            let hf = height_fit.unwrap_or(0);
            if hf == 0 {
                (false, false, 0.0)
            } else {
                (true, true, hf.clamp(20, 4000) as f32)
            }
        }
        StackLayoutMode::Scroll => {
            let hs = height_scroll
                .unwrap_or(DEFAULT_SCROLL_HEIGHT)
                .clamp(20, 4000);
            (true, false, hs as f32)
        }
    }
}

/// Apply price scale to every panel in the stack.
pub(super) fn set_panels_scale<S: 'static>(
    entries: &[ChartStackEntry],
    pct: Option<f32>,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_scale(pct, pcx));
    }
}

/// Apply the order-book toggle to every panel in the stack.
pub(super) fn set_panels_orderbook_enabled<S: 'static>(
    entries: &[ChartStackEntry],
    enabled: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel
            .update(cx, |p, pcx| p.set_orderbook_enabled(enabled, pcx));
    }
}

/// Apply the control-zone fill toggle to every panel in the stack.
pub(super) fn set_panels_show_zone<S: 'static>(
    entries: &[ChartStackEntry],
    show: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_show_zone(show, pcx));
    }
}

/// Apply the auto-pin-on-order toggle to every panel in the stack.
pub(super) fn set_panels_auto_pin<S: 'static>(
    entries: &[ChartStackEntry],
    on: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_auto_pin(on, pcx));
    }
}

/// Apply market-action button positions for Cancel Buy and Panic Sell to every stack panel.
pub(super) fn set_panels_action_btn_pos<S: 'static>(
    entries: &[ChartStackEntry],
    cancel: crate::persistence::chart_persist::ChartBtnPos,
    panic: crate::persistence::chart_persist::ChartBtnPos,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel
            .update(cx, |p, pcx| p.set_action_btn_pos(cancel, panic, pcx));
    }
}

/// Apply the Left, Right, or Hidden price-axis position to every stack panel.
pub(super) fn set_panels_price_axis_pos<S: 'static>(
    entries: &[ChartStackEntry],
    pos: crate::persistence::chart_persist::PriceAxisPos,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_price_axis_pos(pos, pcx));
    }
}

/// Apply time-axis visibility to every panel in the stack.
pub(super) fn set_panels_time_axis_visible<S: 'static>(
    entries: &[ChartStackEntry],
    visible: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel
            .update(cx, |p, pcx| p.set_time_axis_visible(visible, pcx));
    }
}

/// Apply line-label visibility to every panel in the stack.
pub(super) fn set_panels_line_labels<S: 'static>(
    entries: &[ChartStackEntry],
    show: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_line_labels(show, pcx));
    }
}

/// Apply the liquidation-trade toggle to every panel in the stack.
pub(super) fn set_panels_liquidations<S: 'static>(
    entries: &[ChartStackEntry],
    enabled: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel
            .update(cx, |p, pcx| p.set_liquidations_enabled(enabled, pcx));
    }
}

/// Apply candle rendering settings to every stack panel, with `None` meaning the global default.
pub(super) fn set_panels_candle_view<S: 'static>(
    entries: &[ChartStackEntry],
    cfg: Option<moon_core::market::CandleViewCfg>,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_candle_view(cfg, pcx));
    }
}

/// Apply chart-drawing settings to every panel in the stack.
pub(super) fn set_panels_chart_graphics<S: 'static>(
    entries: &[ChartStackEntry],
    cfg: Option<moon_core::config::ChartGraphicsCfg>,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_chart_graphics(cfg, pcx));
    }
}

/// Apply chart caption labels to every panel in the stack.
pub(super) fn set_panels_chart_labels<S: 'static>(
    entries: &[ChartStackEntry],
    cfg: Option<moon_core::config::ChartLabelsCfg>,
    cx: &mut Context<S>,
) {
    for e in entries {
        let cfg = cfg.clone();
        e.panel.update(cx, |p, pcx| p.set_chart_labels(cfg, pcx));
    }
}

/// Apply crosshair-label visibility to every panel in the stack.
pub(super) fn set_panels_cursor_labels<S: 'static>(
    entries: &[ChartStackEntry],
    show: bool,
    cx: &mut Context<S>,
) {
    for e in entries {
        e.panel.update(cx, |p, pcx| p.set_cursor_labels(show, pcx));
    }
}

/// Handle comparison-lock clicks by draining pending requests from every panel.
///
/// A click toggles the anchor: clicking the current anchor again disables comparison; otherwise the
/// clicked panel becomes the anchor and moves to index zero at the far left. Returns `true` when the
/// anchor or order changes.
pub(super) fn handle_compare_lock_requests<S: 'static>(
    entries: &mut Vec<ChartStackEntry>,
    anchor: &mut Option<(CoreId, String)>,
    cx: &mut Context<S>,
) -> bool {
    let mut clicked: Option<(CoreId, String)> = None;
    for e in entries.iter() {
        if e.panel.update(cx, |p, _| p.take_compare_lock_request()) {
            clicked = Some((e.core, e.market.clone()));
        }
    }
    let Some(key) = clicked else {
        return false;
    };
    if anchor.as_ref() == Some(&key) {
        *anchor = None; // clicking the anchor again disables comparison
    } else {
        if let Some(pos) = entries
            .iter()
            .position(|e| e.core == key.0 && e.market == key.1)
        {
            let e = entries.remove(pos);
            entries.insert(0, e); // move the anchor to the row's left edge
        }
        *anchor = Some(key);
    }
    true
}

/// Handle broom clicks for follower order-book-only mode by draining every panel's pending request.
///
/// A click toggles `broom_on`. Returns `true` when it changes.
pub(super) fn handle_compare_broom_requests<S: 'static>(
    entries: &[ChartStackEntry],
    broom_on: &mut bool,
    cx: &mut Context<S>,
) -> bool {
    let mut clicked = false;
    for e in entries.iter() {
        if e.panel.update(cx, |p, _| p.take_compare_broom_request()) {
            clicked = true;
        }
    }
    if clicked {
        *broom_on = !*broom_on;
    }
    clicked
}

/// Apply comparison state to panels, setting `compare_eligible = horizontal` on all of them.
///
/// With active comparison (horizontal with an anchor), mark the anchor and impose its Y window on
/// every follower; otherwise clear the lock. The anchor is ALWAYS the leader and remains stable
/// across observer passes, so synchronization converges in a few passes without a notification
/// loop. Panning or zooming the anchor moves everyone; dragging a follower snaps back to the anchor
/// window. Leader-to-all synchronization is separate from pan-everywhere; see internal docs.
///
/// IMPORTANT: never use a follower window as the leader. `set_locked_y` only writes a panel field
/// and reaches the engine on render. During the synchronous observe-to-notify cycle its `y_window()`
/// is still stale, so detecting which panel moved from it causes oscillation and an infinite loop.
pub(super) fn apply_compare<S: 'static>(
    entries: &[ChartStackEntry],
    anchor: &Option<(CoreId, String)>,
    shared: &mut Option<(f32, f32)>,
    horizontal: bool,
    orderbook_only: bool,
    cx: &mut Context<S>,
) {
    let key = anchor
        .as_ref()
        .filter(|k| entries.iter().any(|e| e.core == k.0 && e.market == k.1));
    let active = horizontal && key.is_some();
    if !active {
        *shared = None;
        for e in entries {
            e.panel.update(cx, |p, c| {
                p.set_compare_eligible(horizontal, c);
                p.set_compare_anchor(false, c);
                p.set_locked_y(None, c);
                p.set_orderbook_only(false, c);
                p.set_compare_broom_on(false, c);
                // Inactive comparison has no peers; this also clears the panel's own ghost.
                p.set_ghost_peers(Vec::new());
                p.set_compare_ref_price(None);
            });
        }
        return;
    }
    // Ghost crosshair across all charts: give each panel weak engine handles for every OTHER panel.
    // The hovered panel sends them its cursor price on mouse movement; see `sync_native_cursor`.
    let ghosts: Vec<crate::chartdx::ChartGhostCursor> = entries
        .iter()
        .map(|e| e.panel.read(cx).ghost_cursor_handle())
        .collect();
    let key = key.unwrap();
    // The leader is the anchor's current window, stable within an observer cycle for convergence.
    // Do NOT lock the anchor: it retains its tab-scale, auto, or pan mode while followers copy its
    // live window. Locking the anchor would freeze Y and disable scale and auto behavior.
    let window = entries
        .iter()
        .find(|e| e.core == key.0 && e.market == key.1)
        .and_then(|e| e.panel.read(cx).y_window());
    *shared = window;
    // Anchor Last price for the large "+0.12%" follower delta in broom mode. It renders only on
    // order-book-only panels, gated in `chartdx/text/prepare.rs::prepare_text` by `pr.orderbook_only`.
    let ref_price = entries
        .iter()
        .find(|e| e.core == key.0 && e.market == key.1)
        .and_then(|e| e.panel.read(cx).last_price());
    for (ix, e) in entries.iter().enumerate() {
        let is_anchor = e.core == key.0 && e.market == key.1;
        let peers: Vec<crate::chartdx::ChartGhostCursor> = ghosts
            .iter()
            .enumerate()
            .filter(|(gx, _)| *gx != ix)
            .map(|(_, g)| g.clone())
            .collect();
        e.panel.update(cx, |p, c| {
            p.set_compare_eligible(true, c);
            p.set_compare_anchor(is_anchor, c);
            // The anchor remains free and respects scale or auto; followers lock to its window.
            p.set_locked_y(if is_anchor { None } else { window }, c);
            // Broom mode makes followers order-book-only; the full anchor shows the active broom button.
            p.set_orderbook_only(!is_anchor && orderbook_only, c);
            p.set_compare_broom_on(is_anchor && orderbook_only, c);
            p.set_ghost_peers(peers);
            p.set_compare_ref_price(if is_anchor { None } else { ref_price });
        });
    }
}

/// Return slot `ix`'s broom-mode role for both Main and AddToChart stacks.
///
/// The role is `Normal` while order-book-only mode is off, `Anchor` for the `(core, market)` anchor,
/// and `Follower` for every other slot.
pub(super) fn compare_role(
    entries: &[ChartStackEntry],
    anchor: &Option<(CoreId, String)>,
    orderbook_only: bool,
    ix: usize,
) -> CompareRole {
    if !orderbook_only {
        return CompareRole::Normal;
    }
    match entries.get(ix) {
        Some(e) => {
            let is_anchor = anchor
                .as_ref()
                .is_some_and(|k| k.0 == e.core && k.1 == e.market);
            if is_anchor {
                CompareRole::Anchor
            } else {
                CompareRole::Follower
            }
        }
        None => CompareRole::Normal,
    }
}

/// Synchronize stack comparison mode for Main and AddToChart.
///
/// Vertical layout clears the anchor. Drain panel lock and broom clicks to change or clear the
/// anchor, move it left, and toggle order-book-only mode. Disable that mode without an anchor, then
/// impose the shared Y window and flags on panels. Returns `true` when anchor or order changes and
/// the stack needs notification.
pub(super) fn sync_compare<S: 'static>(
    entries: &mut Vec<ChartStackEntry>,
    anchor: &mut Option<(CoreId, String)>,
    shared: &mut Option<(f32, f32)>,
    orderbook_only: &mut bool,
    orientation: Option<StackOrientation>,
    cx: &mut Context<S>,
) -> bool {
    let horizontal = orientation
        .unwrap_or(StackOrientation::Vertical)
        .is_horizontal();
    if !horizontal {
        *anchor = None;
    }
    let mut changed = handle_compare_lock_requests(entries, anchor, cx);
    changed |= handle_compare_broom_requests(entries, orderbook_only, cx);
    if anchor.is_none() {
        *orderbook_only = false;
    }
    apply_compare(entries, anchor, shared, horizontal, *orderbook_only, cx);
    changed
}

/// Apply a new stack-setting field value to every panel.
///
/// Return when unchanged; otherwise assign the field, update panels through `apply`, and call
/// `cx.notify()`. This removes repeated compare-assign-apply-notify sequences from Main and
/// AddToChart setters. `field` and `entries` are disjoint fields of the calling stack, so `apply`
/// does not capture `self`.
pub(super) fn apply_setting<S, T, F>(
    field: &mut T,
    new: T,
    entries: &[ChartStackEntry],
    cx: &mut Context<S>,
    apply: F,
) where
    S: 'static,
    T: PartialEq,
    F: FnOnce(&[ChartStackEntry], &mut Context<S>),
{
    if *field == new {
        return;
    }
    *field = new;
    apply(entries, cx);
    cx.notify();
}

/// Remove panels without charts from a stack, returning whether its composition changed.
pub(super) fn retain_nonempty_panels(entries: &mut Vec<ChartStackEntry>, cx: &App) -> bool {
    let before = entries.len();
    entries.retain(|e| e.panel.read(cx).pane_count() > 0);
    entries.len() != before
}

/// Render the three-mode stack layout selected in Settings, with orientation from `horizontal`.
///
/// - `scroll=false` selects FIT: panels share window height vertically or width horizontally.
/// - `scroll=true, compress=false` selects SCROLL: fixed `cfg_h` size with vertical
///   `MoonVirtualList` or a horizontal `overflow_x_scroll` container.
/// - `scroll=true, compress=true` selects COMPRESS: fixed size without scrolling, shrinking on
///   overflow.
///
/// `cfg_h` is the fixed slot size along the stack axis: vertical height or horizontal width.
/// `panel_at` retrieves a panel by index, while
/// `tile(s, ix, panel, size, flex, horizontal, border, ent)` builds one tile. FIT and COMPRESS
/// iterate the supplied `s`, which is the calling stack's `&self`; vertical SCROLL retrieves panels
/// through a weak entity in App context to avoid a RefCell panic during render.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_chart_stack<S, P, T, R>(
    base_id: &str,
    s: &S,
    entity: Entity<S>,
    count: usize,
    scroll: bool,
    compress: bool,
    horizontal: bool,
    cfg_h: f32,
    scroll_handle: &MoonVirtualListScrollHandle,
    border: Rgba,
    panel_at: P,
    tile: T,
    role: R,
    on_visible_range: Option<VisibleRangeHandler>,
) -> AnyElement
where
    S: Render + 'static,
    P: Fn(&S, usize) -> Option<Entity<ChartPanel>> + Clone + 'static,
    // tile(s, ix, panel, size, flex, min_w, horizontal, border, ent)
    //   flex=true:  size -> max_w, min_w -> min_w for a stretching anchor.
    //   flex=false: size -> fixed width WITHOUT shrinking, so SCROLL overflows and can scroll.
    T: Fn(
            &S,
            usize,
            Entity<ChartPanel>,
            Option<f32>,
            bool,
            Option<f32>,
            bool,
            Rgba,
            Entity<S>,
        ) -> AnyElement
        + Clone
        + 'static,
    // Broom-mode slot role: Anchor gets its own width, Follower gets order-book width, and Normal is ordinary.
    R: Fn(&S, usize) -> CompareRole + Clone + 'static,
{
    if scroll && !compress {
        if horizontal {
            // Horizontal SCROLL: `MoonVirtualList` only supports vertical layout through GPUI's
            // `uniform_list`, so build a non-virtualized fixed-width row in `overflow_x_scroll`.
            // There are only a few charts, so virtualization is unnecessary. Each tile has fixed
            // WIDTH `cfg_h` and full height.
            let mut tiles: Vec<AnyElement> = Vec::with_capacity(count);
            for ix in 0..count {
                if let Some(panel) = panel_at(s, ix) {
                    // SCROLL with broom: followers use fixed order-book width; anchor and Normal use `cfg_h`.
                    let w = if role(s, ix) == CompareRole::Follower {
                        COMPARE_BOOK_W
                    } else {
                        cfg_h
                    };
                    tiles.push(tile(
                        s,
                        ix,
                        panel,
                        Some(w),
                        false,
                        None,
                        true,
                        border,
                        entity.clone(),
                    ));
                }
            }
            // `overflow_x_scrollbar()` provides horizontal scrolling with a visible MoonUI scrollbar.
            // Tiles do not shrink (`min=max`), so they overflow and create scrollable content.
            return div()
                .relative()
                .size_full()
                .child(h_flex().h_full().children(tiles))
                .overflow_x_scrollbar()
                .into_any_element();
        }
        // Vertical SCROLL uses fixed height and a virtual list with scrollbar. Build each tile through
        // a weak entity because the `MoonVirtualList` factory receives `App`, not `Context`.
        let weak = entity.downgrade();
        let panel_at_v = panel_at.clone();
        let tile_v = tile.clone();
        let list = MoonVirtualList::new(
            format!("{base_id}-vlist"),
            count,
            cfg_h,
            move |ix, _window, app| {
                let Some(ent) = weak.upgrade() else {
                    return div().into_any_element();
                };
                let s = ent.read(app);
                let Some(panel) = panel_at_v(s, ix) else {
                    return div().into_any_element();
                };
                tile_v(
                    s,
                    ix,
                    panel,
                    Some(cfg_h),
                    false,
                    None,
                    false,
                    border,
                    ent.clone(),
                )
            },
        )
        .track_scroll(scroll_handle)
        .surface(false)
        .border(false)
        .radius(0.0)
        .scrollbar_visibility(MoonScrollbarVisibility::Hover);
        let list = if let Some(on_visible_range) = on_visible_range {
            list.on_visible_range(on_visible_range)
        } else {
            list
        };
        return div()
            .id(format!("{base_id}-scroll"))
            .relative()
            .size_full()
            .child(list)
            .into_any_element();
    }

    // FIT and COMPRESS fill the window with vertical `v_flex` or horizontal `h_flex`, without scroll.
    // In COMPRESS each slot flexes with a `cfg_h` cap (`size=Some`, `flex=true` sets an axis maximum):
    // few charts each use `cfg_h` and leave a tail; many shrink toward `window/count`. FIT has no cap.
    let mut tiles: Vec<AnyElement> = Vec::with_capacity(count);
    for ix in 0..count {
        // Slot size along the axis in broom mode:
        // - Anchor uses its own width: compress is `flex+max(cfg)`, stretch zero is growing flex.
        // - Follower with stretch zero is `flex+max(order book)`, narrow and proportionally
        //   shrinkable; with compress it is uncapped flex sharing remaining window space.
        // - Normal follows ordinary sizing: max cfg in compress, otherwise flex.
        // For `(size=max_w, flex, min_w)` specifically:
        // - Follower at width zero uses `flex+max(order book)`, keeping all order books even while
        //   allowing them to shrink.
        // - Follower at positive width uses uncapped flex and shares the remaining window.
        // - Anchor in stretch uses `flex+min(1.5 order books)` so it remains larger and cannot collapse.
        // - Anchor in compress uses `flex+max(cfg)` for its configured width.
        // - Normal retains standard sizing: max cfg in compress and flex otherwise.
        let (size, flex, min_w) = match role(s, ix) {
            // FIT width zero: followers are even (`flex+max order book`); anchor is larger (`flex+min`).
            CompareRole::Follower if !compress => (Some(COMPARE_BOOK_W), true, None),
            CompareRole::Anchor if !compress => (None, true, Some(COMPARE_ANCHOR_MIN_W)),
            // FIT positive width in compress: anchor has a FIXED configured pixel width without
            // shrinking, while followers continuously share the remainder through uncapped flex.
            CompareRole::Anchor => (Some(cfg_h), false, None),
            CompareRole::Follower => (None, true, None),
            // Normal non-broom sizing: COMPRESS is `flex+max(cfg)`; FIT stretch is flex.
            CompareRole::Normal => {
                if compress {
                    (Some(cfg_h), true, None)
                } else {
                    (None, true, None)
                }
            }
        };
        match panel_at(s, ix) {
            Some(panel) => tiles.push(tile(
                s,
                ix,
                panel,
                size,
                flex,
                min_w,
                horizontal,
                border,
                entity.clone(),
            )),
            None => {
                // A retained empty COMPRESS slot is a transparent placeholder with the same axis size.
                let mut e = div().relative().overflow_hidden();
                e = if horizontal { e.h_full() } else { e.w_full() };
                if flex {
                    e = e.flex_1();
                    let m = min_w.unwrap_or(0.0);
                    e = if horizontal {
                        e.min_w(px(m))
                    } else {
                        e.min_h(px(m))
                    };
                    if let Some(v) = size {
                        e = if horizontal {
                            e.max_w(px(v))
                        } else {
                            e.max_h(px(v))
                        };
                    }
                } else if let Some(v) = size {
                    // Fixed WITHOUT shrinking (`min=max=v`); otherwise SCROLL flex removes overflow.
                    e = if horizontal {
                        e.w(px(v)).min_w(px(v))
                    } else {
                        e.h(px(v)).min_h(px(v))
                    };
                }
                tiles.push(e.into_any_element());
            }
        }
    }
    let inner = if horizontal {
        h_flex().size_full().children(tiles)
    } else {
        v_flex().size_full().children(tiles)
    };
    div()
        .id(format!("{base_id}-fit"))
        .relative()
        .size_full()
        .overflow_hidden()
        .child(inner)
        .into_any_element()
}

#[cfg(test)]
mod tests;
