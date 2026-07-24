//! Chart-panel figure-layer interaction: pencil mode, Command/Ctrl-left-click drawing, hover,
//! selection, node/body dragging, and the right-click Alert/Delete menu. The selected tool
//! (`Backend::fig_tool`), mode (`fig_draw_mode`), and selection (`fig_selected`) are global. New
//! drawing, selection, and hit testing start only in the chart area; an active draft or drag may
//! continue and finish over the order-book zone.

use gpui::{Context, Pixels, Point, Window};
use rust_i18n::t;

use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _};

use moon_core::figures::{FigNode, Figure, FigureKind, FigureTool};
use moon_core::session::CoreId;

use super::ChartPanel;
use super::geom::PaneMap;
use crate::chartdx::FigureVisual;

/// Figure-line hit-test threshold in pixels before scaling by pixels-per-point, matching order lines.
const HIT_PX: f32 = 6.0;

/// Figure currently being drawn on this panel.
pub(super) struct FigDraft {
    pub pane: usize,
    pub core: CoreId,
    pub market: String,
    pub tool: FigureTool,
    /// Color, thickness, and line-kind snapshot captured from `Backend::fig_style` at draw start.
    pub color: [u8; 4],
    pub thickness: f32,
    pub kind: moon_core::figures::LineKind,
    /// First node of a multi-point figure; `cursor` supplies the preview endpoint.
    pub first: Option<FigNode>,
    /// Triangle's fixed second node while awaiting the third click.
    pub base_b: Option<FigNode>,
    /// Current cursor position in data coordinates.
    pub cursor: FigNode,
}

/// Drag state for an existing figure.
pub(super) struct FigDrag {
    pub core: CoreId,
    pub market: String,
    pub id: u64,
    pub pane: usize,
    pub grab: FigGrab,
    /// Grab-point offset from the anchor node, preventing the figure from jumping to the cursor.
    pub grab_off_price: f64,
    pub grab_off_time: f64,
}

/// Portion of a figure being dragged.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum FigGrab {
    /// First node (`a`) or first channel price.
    NodeA,
    /// Second node (`b`) or second channel price.
    NodeB,
    /// Triangle's third node (`c`).
    NodeC,
    /// Entire body, translated in price and time.
    Body,
}

/// Return the pixel distance from a point to a line segment.
fn seg_dist(px: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 1e-6 {
        0.0
    } else {
        (((px.0 - a.0) * dx + (px.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((px.0 - cx).powi(2) + (px.1 - cy).powi(2)).sqrt()
}

impl ChartPanel {
    /// Return the `(core, market)` chart key for a pane index.
    fn fig_pane_key(&self, pane: usize) -> Option<(CoreId, String)> {
        self.chart
            .with_container(|c| c.pane(pane).map(|p| (p.core, p.market.clone())))
    }

    /// Handle a left-button press for the figure layer.
    ///
    /// Interaction requires pencil mode and a true `draw_mod` gate. The caller sets this gate when
    /// the secondary modifier is held—Command on macOS or Ctrl on Windows/Linux—or when a draft is
    /// already active, allowing later nodes without a held modifier. With no draft, an existing
    /// figure is grabbed first; otherwise the click places the next node. Returns whether the
    /// figure layer consumed it.
    pub(super) fn try_fig_click(
        &mut self,
        pos: (f32, f32),
        draw_mod: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.backend.read(cx).fig_draw_mode {
            return false;
        }
        // Match Moonbot: starting a draft or grabbing a figure requires the secondary modifier.
        // The caller also keeps this gate true for an existing draft, so its later clicks continue
        // without the modifier. An unmodified click with no draft continues to trading/navigation.
        if !draw_mod {
            return false;
        }
        let Some(pane) = self.input.pane_at(pos.0, pos.1) else {
            return false;
        };
        // Leave the order-book/reserved control zone to trading input.
        if self.glass_pane_at(pos).is_some() {
            return false;
        }
        let Some(map) = self.pane_map(pane) else {
            return false;
        };
        // With no active draft, modifier-click grabs/selects an existing node or body first. Empty
        // space, or any click continuing a draft, places a node for the new figure.
        if self.fig_draft.is_none() && self.try_fig_grab(pane, pos, &map, cx) {
            return true;
        }
        let tool = self.backend.read(cx).fig_tool;
        let node = map.node_at(pos);
        self.fig_draw_click(pane, tool, node, cx);
        // Retain the placed-node position for the drag-release gesture in `try_fig_release`.
        self.fig_draw_down = self.fig_draft.is_some().then_some(pos);
        true
    }

    /// Place a node in pencil mode, completing the figure when enough nodes exist.
    fn fig_draw_click(
        &mut self,
        pane: usize,
        tool: FigureTool,
        node: FigNode,
        cx: &mut Context<Self>,
    ) {
        let Some((core, market)) = self.fig_pane_key(pane) else {
            return;
        };
        let style = self.backend.read(cx).fig_style;
        // Discard a draft started on another pane or target; drawing follows the current click.
        if self
            .fig_draft
            .as_ref()
            .is_some_and(|d| d.pane != pane || d.core != core || d.market != market)
        {
            self.fig_draft = None;
        }
        let finished: Option<FigureKind> = match (&mut self.fig_draft, tool) {
            // A horizontal line completes on its first click.
            (_, FigureTool::HLine) => Some(FigureKind::HLine { price: node.price }),
            // The first click of any multi-point tool starts a draft.
            (None, _) => {
                self.fig_draft = Some(FigDraft {
                    pane,
                    core,
                    market: market.clone(),
                    tool,
                    color: style.color,
                    thickness: style.thickness,
                    kind: style.kind,
                    first: Some(node),
                    base_b: None,
                    cursor: node,
                });
                None
            }
            // A segment completes on the second click.
            (Some(d), FigureTool::Segment) => {
                let a = d.first.take().unwrap_or(node);
                Some(FigureKind::Segment { a, b: node })
            }
            // A channel uses two price clicks; time is irrelevant to the horizontal band.
            (Some(d), FigureTool::Channel) => {
                let a = d.first.take().unwrap_or(node);
                Some(FigureKind::Channel {
                    price1: a.price,
                    price2: node.price,
                })
            }
            // A triangle uses three clicks: `a`, `b`, and `c`.
            (Some(d), FigureTool::Triangle) => {
                if d.base_b.is_none() {
                    // Retain the second vertex while awaiting the third.
                    d.base_b = Some(node);
                    d.cursor = node;
                    None
                } else {
                    let a = d.first.unwrap_or(node);
                    let b = d.base_b.unwrap();
                    Some(FigureKind::Triangle { a, b, c: node })
                }
            }
        };
        if let Some(kind) = finished {
            self.fig_draft = None;
            let style = self.backend.read(cx).fig_style;
            let fig = Figure {
                id: 0,
                kind,
                color: style.color,
                thickness: style.thickness,
                line_kind: style.kind,
                created_ms: moon_core::util::now_unix_ms_i64(),
                alert: false,
                strategy_id: 0,
                from_server: false,
            };
            let id = self
                .backend
                .read(cx)
                .figures
                .borrow_mut()
                .add(core, &market, fig);
            // Select a new figure immediately so its nodes appear and it can be moved or deleted.
            self.backend.update(cx, |b, bcx| {
                b.fig_selected = Some((core, market.clone(), id));
                bcx.notify();
            });
        }
        self.sync_fig_visual(cx);
    }

    /// Advance a draft with a press-drag-release gesture.
    ///
    /// A release far enough from the placed node counts as the next click, including the next
    /// triangle vertex. A release near the press is not a gesture, so the normal click-click flow
    /// continues. Returns whether the release advanced the draft.
    pub(super) fn try_fig_release(&mut self, pos: (f32, f32), cx: &mut Context<Self>) -> bool {
        let Some(d) = self.fig_draft.as_ref() else {
            return false;
        };
        let Some(down) = self.fig_draw_down else {
            return false;
        };
        let dist = ((pos.0 - down.0).powi(2) + (pos.1 - down.1).powi(2)).sqrt();
        // Require more than the hit threshold so click jitter cannot complete a figure.
        let threshold = 2.0 * HIT_PX * self.last_ppp.max(1.0);
        if dist < threshold {
            return false;
        }
        let (pane, tool) = (d.pane, d.tool);
        let Some(map) = self.pane_map(pane) else {
            return false;
        };
        let node = map.node_at(pos);
        self.fig_draw_click(pane, tool, node, cx);
        self.fig_draw_down = self.fig_draft.is_some().then_some(pos);
        true
    }

    /// Grab a figure on modifier-left-click, preferring selected nodes over the nearest body.
    fn try_fig_grab(
        &mut self,
        pane: usize,
        pos: (f32, f32),
        map: &PaneMap,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((core, market)) = self.fig_pane_key(pane) else {
            return false;
        };
        let threshold = HIT_PX * self.last_ppp.max(1.0);
        let b = self.backend.read(cx);
        let store = b.figures.borrow();
        let figures = store.figures(core, &market);
        if figures.is_empty() {
            return false;
        }
        let selected = b
            .fig_selected
            .as_ref()
            .filter(|(c, m, _)| *c == core && *m == market)
            .map(|(_, _, id)| *id);
        // Give nodes of the selected figure priority over all bodies.
        if let Some(sel_id) = selected {
            if let Some(fig) = figures.iter().find(|f| f.id == sel_id) {
                if let Some(grab) = hit_node(fig, pos, map, threshold) {
                    let (off_p, off_t) = grab_offset(fig, grab, map, pos);
                    drop(store);
                    self.fig_drag = Some(FigDrag {
                        core,
                        market,
                        id: sel_id,
                        pane,
                        grab,
                        grab_off_price: off_p,
                        grab_off_time: off_t,
                    });
                    return true;
                }
            }
        }
        // Otherwise select and drag the nearest figure body.
        let mut best: Option<(u64, f32)> = None;
        for fig in figures {
            let d = hit_body(fig, pos, map);
            if d <= threshold && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((fig.id, d));
            }
        }
        let Some((id, _)) = best else {
            drop(store);
            let _ = b;
            // Clear selection on a miss without consuming the click, allowing pane input to run.
            if self.backend.read(cx).fig_selected.is_some() {
                self.backend.update(cx, |b, bcx| {
                    b.fig_selected = None;
                    bcx.notify();
                });
                self.sync_fig_visual(cx);
            }
            return false;
        };
        let fig = figures.iter().find(|f| f.id == id).unwrap();
        let (off_p, off_t) = grab_offset(fig, FigGrab::Body, map, pos);
        drop(store);
        let _ = b;
        self.backend.update(cx, |b, bcx| {
            b.fig_selected = Some((core, market.clone(), id));
            bcx.notify();
        });
        self.fig_drag = Some(FigDrag {
            core,
            market,
            id,
            pane,
            grab: FigGrab::Body,
            grab_off_price: off_p,
            grab_off_time: off_t,
        });
        self.sync_fig_visual(cx);
        true
    }

    /// Update draft preview, active dragging, and figure hover from a mouse-move event.
    ///
    /// `pressed_left` reports whether the left button remains held. The return value reports an
    /// active drag or a later preview/hover change; cancelling a draft after pencil mode is disabled
    /// synchronizes visuals but can still return `false`, especially when outside the chart.
    pub(super) fn update_fig_pointer(
        &mut self,
        pos: (f32, f32),
        within: bool,
        pressed_left: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // Cancel the draft when Escape or the toolbar disables pencil mode.
        if !self.backend.read(cx).fig_draw_mode && self.fig_draft.is_some() {
            self.fig_draft = None;
            self.sync_fig_visual(cx);
        }
        if !within {
            return false;
        }
        // Edit an actively dragged figure in place and force the same immediate rebuild as an
        // order drag; otherwise the line would update only on data ticks and visibly lag.
        if pressed_left {
            if let Some(drag) = &self.fig_drag {
                let pane = self.input.pane_at(pos.0, pos.1);
                let Some(map) = pane.and_then(|p| self.pane_map(p)) else {
                    return false;
                };
                let target = FigNode {
                    time_ms: map.time_at_x(pos.0) - drag.grab_off_time,
                    price: map.price_at_y(pos.1) - drag.grab_off_price,
                };
                let (core, market, id, grab, dpane) = (
                    drag.core,
                    drag.market.clone(),
                    drag.id,
                    drag.grab,
                    drag.pane,
                );
                let edited =
                    self.backend
                        .read(cx)
                        .figures
                        .borrow_mut()
                        .edit(core, &market, id, |fig| apply_drag(fig, grab, target));
                if edited {
                    self.fig_resync(cx);
                    cx.notify();
                }
                // Keep the crosshair under the pointer, matching order-line dragging.
                self.input.cursor = Some(pos);
                self.input.hovered_pane = Some(dpane);
                self.sync_native_cursor();
                return true;
            }
            return false;
        }
        // Move the draft's preview endpoint with the cursor while it remains on the draft pane.
        let mut changed = false;
        let draft_pane = self.fig_draft.as_ref().map(|d| d.pane);
        if let Some(dp) = draft_pane {
            if self.input.pane_at(pos.0, pos.1) == Some(dp) {
                if let Some(map) = self.pane_map(dp) {
                    let node = map.node_at(pos);
                    if let Some(d) = &mut self.fig_draft {
                        if d.cursor != node {
                            d.cursor = node;
                            changed = true;
                        }
                    }
                }
            }
        }
        // Pencil-mode hover previews which figure a modifier-click would select or drag.
        let hover = if self.backend.read(cx).fig_draw_mode {
            self.fig_hit_at(pos, cx)
        } else {
            None
        };
        if hover != self.fig_hover {
            self.fig_hover = hover;
            changed = true;
        }
        if changed {
            self.sync_fig_visual(cx);
        }
        changed
    }

    /// Return the nearest figure-body ID under the cursor within the scaled hit threshold.
    fn fig_hit_at(&self, pos: (f32, f32), cx: &Context<Self>) -> Option<u64> {
        let pane = self.input.pane_at(pos.0, pos.1)?;
        if self.glass_pane_at(pos).is_some() {
            return None;
        }
        let (core, market) = self.fig_pane_key(pane)?;
        let map = self.pane_map(pane)?;
        let threshold = HIT_PX * self.last_ppp.max(1.0);
        let b = self.backend.read(cx);
        let store = b.figures.borrow();
        let mut best: Option<(u64, f32)> = None;
        for fig in store.figures(core, &market) {
            let d = hit_body(fig, pos, &map);
            if d <= threshold && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((fig.id, d));
            }
        }
        best.map(|(id, _)| id)
    }

    /// Open the Alert/Delete context menu for a right-clicked figure in pencil mode.
    ///
    /// This is limited to the chart area. Returns `true` when the menu opened so the caller can
    /// suppress its fullscreen toggle; returning `false` leaves normal right-click behavior active.
    pub(super) fn try_open_figure_menu(
        &mut self,
        local_pos: (f32, f32),
        menu_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.backend.read(cx).fig_draw_mode {
            return false;
        }
        // `fig_hit_at` excludes the order-book zone through `glass_pane_at`.
        let Some(id) = self.fig_hit_at(local_pos, cx) else {
            return false;
        };
        let Some(pane) = self.input.pane_at(local_pos.0, local_pos.1) else {
            return false;
        };
        let Some((core, market)) = self.fig_pane_key(pane) else {
            return false;
        };
        let armed = self
            .backend
            .read(cx)
            .figures
            .borrow()
            .get(core, &market, id)
            .map(|f| f.alert)
            .unwrap_or(false);
        // Select the right-clicked figure so its nodes are visible.
        self.backend.update(cx, |b, bcx| {
            b.fig_selected = Some((core, market.clone(), id));
            bcx.notify();
        });
        self.sync_fig_visual(cx);
        let alert_label = if armed {
            t!("chart.fig_menu.alert_off")
        } else {
            t!("chart.fig_menu.alert_on")
        }
        .to_string();
        let backend_alert = self.backend.clone();
        let backend_del = self.backend.clone();
        let market_del = market.clone();
        let items: Vec<MoonMenuItem> = vec![
            MoonMenuItem::with_key("fig-alert", alert_label).on_click(move |_, window, app| {
                window.close_context_menu(app);
                backend_alert.update(app, |b, _| {
                    b.toggle_selected_figure_alert();
                });
            }),
            MoonMenuItem::with_key("fig-delete", t!("chart.fig_menu.delete").to_string()).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend_del.update(app, |b, _| {
                        b.remove_figure(core, &market_del, id);
                    });
                },
            ),
        ];
        window.open_moon_context_menu(cx, "chart-fig-menu", menu_pos, items, 160.0);
        cx.notify();
        true
    }

    /// Finish a figure drag and re-upsert its changed coordinates when it is armed as an alert.
    ///
    /// Returns whether an active drag consumed the mouse-up event.
    pub(super) fn finish_fig_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.fig_drag.take() else {
            return false;
        };
        self.backend.update(cx, |b, _| {
            b.reupsert_figure_alert(drag.core, &drag.market, drag.id);
        });
        self.sync_fig_visual(cx);
        true
    }

    /// Publish pencil mode, draft preview, hover, and selection to the chart engine.
    ///
    /// Mouse events and the Backend observer both call this so toggling pencil mode immediately
    /// hides or shows the interactive layer.
    pub(super) fn sync_fig_visual(&mut self, cx: &mut Context<Self>) {
        let b = self.backend.read(cx);
        let draw_mode = b.fig_draw_mode;
        let key = self
            .fig_draft
            .as_ref()
            .map(|d| (d.core, d.market.clone()))
            .or_else(|| b.fig_selected.as_ref().map(|(c, m, _)| (*c, m.clone())))
            .or_else(|| self.input.hovered_pane.and_then(|p| self.fig_pane_key(p)));
        let draft = self.fig_draft.as_ref().and_then(draft_preview);
        let selected = b
            .fig_selected
            .as_ref()
            .filter(|(c, m, _)| key.as_ref().is_some_and(|(kc, km)| kc == c && km == m))
            .map(|(_, _, id)| *id);
        let visual = FigureVisual {
            draw_mode,
            key,
            draft,
            hovered: self.fig_hover,
            selected,
        };
        let _ = b;
        if self.chart.set_figure_visual(visual) {
            // Userdata rebuilds only through `sync_orders_*`; trigger it immediately so preview and
            // highlighting do not wait for the next data tick or Backend notification.
            self.fig_resync(cx);
            cx.notify();
        }
    }

    /// Immediately rebuild userdata, where figures travel with the order layers.
    ///
    /// Force the rebuild as for order dragging so the normal gate cannot skip drag frames.
    fn fig_resync(&mut self, cx: &Context<Self>) {
        let b = self.backend.read(cx);
        self.chart.sync_orders_if_visible(&b.session, true);
    }
}

/// Build the transient preview figure drawn under the cursor from a draft.
fn draft_preview(d: &FigDraft) -> Option<Figure> {
    let kind = match (d.tool, d.first, d.base_b) {
        (FigureTool::HLine, ..) => FigureKind::HLine {
            price: d.cursor.price,
        },
        (FigureTool::Segment, Some(a), _) => FigureKind::Segment { a, b: d.cursor },
        // Preview a channel as horizontal lines at the first and cursor prices.
        (FigureTool::Channel, Some(a), _) => FigureKind::Channel {
            price1: a.price,
            price2: d.cursor.price,
        },
        // After one click preview an `a`-to-cursor edge; after two, preview the full triangle.
        (FigureTool::Triangle, Some(a), None) => FigureKind::Segment { a, b: d.cursor },
        (FigureTool::Triangle, Some(a), Some(b)) => FigureKind::Triangle { a, b, c: d.cursor },
        _ => return None,
    };
    Some(Figure {
        id: 0,
        kind,
        color: d.color,
        thickness: d.thickness,
        line_kind: d.kind,
        created_ms: 0,
        alert: false,
        strategy_id: 0,
        from_server: false,
    })
}

/// Return the pixel distance from a point to the nearest part of a figure body.
fn hit_body(fig: &Figure, pos: (f32, f32), map: &PaneMap) -> f32 {
    match &fig.kind {
        FigureKind::HLine { price } => (pos.1 - map.y_of_price(*price)).abs(),
        FigureKind::Segment { a, b } => seg_dist(pos, node_px(a, map), node_px(b, map)),
        FigureKind::Triangle { a, b, c } => {
            let (pa, pb, pc) = (node_px(a, map), node_px(b, map), node_px(c, map));
            seg_dist(pos, pa, pb)
                .min(seg_dist(pos, pb, pc))
                .min(seg_dist(pos, pc, pa))
        }
        FigureKind::Channel { price1, price2 } => (pos.1 - map.y_of_price(*price1))
            .abs()
            .min((pos.1 - map.y_of_price(*price2)).abs()),
    }
}

/// Return the selected figure node or channel line under the cursor for dragging.
fn hit_node(fig: &Figure, pos: (f32, f32), map: &PaneMap, threshold: f32) -> Option<FigGrab> {
    let near = |n: &FigNode| {
        let p = node_px(n, map);
        ((pos.0 - p.0).powi(2) + (pos.1 - p.1).powi(2)).sqrt() <= threshold
    };
    match &fig.kind {
        FigureKind::HLine { .. } => None,
        FigureKind::Segment { a, b } => {
            if near(a) {
                Some(FigGrab::NodeA)
            } else if near(b) {
                Some(FigGrab::NodeB)
            } else {
                None
            }
        }
        FigureKind::Triangle { a, b, c } => {
            if near(a) {
                Some(FigGrab::NodeA)
            } else if near(b) {
                Some(FigGrab::NodeB)
            } else if near(c) {
                Some(FigGrab::NodeC)
            } else {
                None
            }
        }
        // Grab the nearest channel line by Y distance because channels render no node marker.
        FigureKind::Channel { price1, price2 } => {
            let d1 = (pos.1 - map.y_of_price(*price1)).abs();
            let d2 = (pos.1 - map.y_of_price(*price2)).abs();
            if d1 <= threshold && d1 <= d2 {
                Some(FigGrab::NodeA)
            } else if d2 <= threshold {
                Some(FigGrab::NodeB)
            } else {
                None
            }
        }
    }
}

fn node_px(n: &FigNode, map: &PaneMap) -> (f32, f32) {
    (map.x_of_time(n.time_ms), map.y_of_price(n.price))
}

/// Return the grab offset from a figure anchor as `(price_offset, time_offset)`.
///
/// Preserving this offset prevents a drag from snapping the figure to the cursor. Horizontal lines
/// and channels do not use time, so their time offset is zero.
fn grab_offset(fig: &Figure, grab: FigGrab, map: &PaneMap, pos: (f32, f32)) -> (f64, f64) {
    let cur = FigNode {
        time_ms: map.time_at_x(pos.0),
        price: map.price_at_y(pos.1),
    };
    // Choose price from the figure kind and grab target. Use cursor time for horizontal lines and
    // channels, yielding zero time offset; segment and triangle nodes use their actual time.
    let anchor = match (&fig.kind, grab) {
        (FigureKind::HLine { price }, _) => (*price, cur.time_ms),
        (FigureKind::Channel { price1, .. }, FigGrab::NodeA)
        | (FigureKind::Channel { price1, .. }, FigGrab::Body) => (*price1, cur.time_ms),
        (FigureKind::Channel { price2, .. }, FigGrab::NodeB) => (*price2, cur.time_ms),
        (FigureKind::Channel { price1, .. }, FigGrab::NodeC) => (*price1, cur.time_ms),
        (FigureKind::Segment { a, .. }, FigGrab::NodeA)
        | (FigureKind::Triangle { a, .. }, FigGrab::NodeA)
        | (FigureKind::Segment { a, .. }, FigGrab::Body)
        | (FigureKind::Triangle { a, .. }, FigGrab::Body) => (a.price, a.time_ms),
        (FigureKind::Segment { b, .. }, FigGrab::NodeB)
        | (FigureKind::Triangle { b, .. }, FigGrab::NodeB) => (b.price, b.time_ms),
        (FigureKind::Triangle { c, .. }, FigGrab::NodeC) => (c.price, c.time_ms),
        // `NodeC` is unreachable for a segment; use the cursor as a harmless fallback.
        (FigureKind::Segment { .. }, FigGrab::NodeC) => (cur.price, cur.time_ms),
    };
    (cur.price - anchor.0, cur.time_ms - anchor.1)
}

/// Apply a drag using `target` as the new anchor-node position or anchor price.
fn apply_drag(fig: &mut Figure, grab: FigGrab, target: FigNode) -> bool {
    match (&mut fig.kind, grab) {
        (FigureKind::HLine { price }, _) => {
            if *price == target.price {
                return false;
            }
            *price = target.price;
        }
        (FigureKind::Channel { price1, .. }, FigGrab::NodeA) => {
            if *price1 == target.price {
                return false;
            }
            *price1 = target.price;
        }
        (FigureKind::Channel { price2, .. }, FigGrab::NodeB) => {
            if *price2 == target.price {
                return false;
            }
            *price2 = target.price;
        }
        (FigureKind::Channel { price1, price2 }, FigGrab::Body) => {
            let dp = target.price - *price1;
            if dp == 0.0 {
                return false;
            }
            *price1 += dp;
            *price2 += dp;
        }
        (FigureKind::Segment { a, .. }, FigGrab::NodeA)
        | (FigureKind::Triangle { a, .. }, FigGrab::NodeA) => {
            if *a == target {
                return false;
            }
            *a = target;
        }
        (FigureKind::Segment { b, .. }, FigGrab::NodeB)
        | (FigureKind::Triangle { b, .. }, FigGrab::NodeB) => {
            if *b == target {
                return false;
            }
            *b = target;
        }
        (FigureKind::Triangle { c, .. }, FigGrab::NodeC) => {
            if *c == target {
                return false;
            }
            *c = target;
        }
        (FigureKind::Segment { a, b }, FigGrab::Body) => {
            let (dp, dt) = (target.price - a.price, target.time_ms - a.time_ms);
            if dp == 0.0 && dt == 0.0 {
                return false;
            }
            a.price += dp;
            a.time_ms += dt;
            b.price += dp;
            b.time_ms += dt;
        }
        (FigureKind::Triangle { a, b, c }, FigGrab::Body) => {
            let (dp, dt) = (target.price - a.price, target.time_ms - a.time_ms);
            if dp == 0.0 && dt == 0.0 {
                return false;
            }
            for n in [a, b, c] {
                n.price += dp;
                n.time_ms += dt;
            }
        }
        // `NodeC` is unreachable for segments and channels.
        (FigureKind::Segment { .. }, FigGrab::NodeC)
        | (FigureKind::Channel { .. }, FigGrab::NodeC) => return false,
    }
    true
}
