//! Chart-panel figure-layer interaction: drawing mode, Command/Ctrl-left-click drawing, hover,
//! selection, handle/body dragging, and the right-click menu. The selected tool
//! (`Backend::fig_tool`), mode (`fig_draw_mode`), and selection (`fig_selected`) are global. New
//! drawing, selection, and hit testing start only in the chart area; an active draft or drag may
//! continue and finish over the order-book zone.
//!
//! Nothing here knows a figure TYPE. Placing nodes goes through the tool registry (see
//! [`draft`]), hit testing and dragging go through `ToolShape`, so this file stays the same size
//! as tools are added.

mod draft;

use gpui::{Context, Pixels, Point, Window};
use rust_i18n::t;

use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _};

use moon_core::figures::proj::pt_dist;
use moon_core::figures::{
    FigNode, Figure, FigureTool, Grab, Proj, drag_figure, pick_figure, pick_handle,
};
use moon_core::session::CoreId;

use super::ChartPanel;
use super::geom::PaneMap;
use crate::chartdx::FigureVisual;

pub(super) use self::draft::FigDraft;

/// Figure-line hit-test threshold in pixels before scaling by pixels-per-point, matching order lines.
const HIT_PX: f32 = 6.0;

/// Drag state for an existing figure.
pub(super) struct FigDrag {
    pub core: CoreId,
    pub market: String,
    pub id: u64,
    pub pane: usize,
    pub grab: Grab,
    /// Cursor position, in data coordinates, of the previous drag step.
    ///
    /// Dragging applies the DELTA between steps rather than moving the figure to an absolute
    /// target: the figure then follows the cursor without jumping to it, and a price-only tool
    /// drops the time component by itself instead of needing a per-tool anchor table here.
    pub last: FigNode,
}

impl ChartPanel {
    /// Return the `(core, market)` chart key for a pane index.
    fn fig_pane_key(&self, pane: usize) -> Option<(CoreId, String)> {
        self.chart
            .with_container(|c| c.pane(pane).map(|p| (p.core, p.market.clone())))
    }

    /// Handle a left-button press for the figure layer.
    ///
    /// Interaction requires a true `draw_mod` gate. The caller sets this gate when the secondary
    /// modifier is held—Command on macOS or Ctrl on Windows/Linux—or when a draft is already
    /// active, allowing later nodes without a held modifier. With no draft, an existing figure is
    /// grabbed first — which needs no armed tool, since a figure is grabbable whenever it is drawn;
    /// otherwise the click places the next node, which does. Returns whether the figure layer
    /// consumed it.
    pub(super) fn try_fig_click(
        &mut self,
        pos: (f32, f32),
        draw_mod: bool,
        cx: &mut Context<Self>,
    ) -> bool {
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
        // With no active draft, modifier-click grabs/selects an existing handle or body first.
        // Empty space, or any click continuing a draft, places a node for the new figure.
        if self.fig_draft.is_none() && self.try_fig_grab(pane, pos, &map, cx) {
            return true;
        }
        // Grabbing works with no tool armed — that is what the Cursor entry leaves behind — but
        // PLACING a node needs one. Without this the click would fall through to trading, which is
        // exactly right: with no tool selected a modifier-click that hits nothing is not a draw.
        if !self.backend.read(cx).fig_draw_mode {
            return false;
        }
        let tool = self.backend.read(cx).fig_tool;
        let node = map.node_at(pos);
        self.fig_draw_click(pane, tool, node, cx);
        // Retain the placed-node position for the drag-release gesture in `try_fig_release`.
        self.fig_draw_down = self.fig_draft.is_some().then_some(pos);
        true
    }

    /// Place a node in drawing mode, completing the figure once the tool has all its clicks.
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
        // Discard a draft started on another pane, chart or tool; drawing follows the current click.
        if self
            .fig_draft
            .as_ref()
            .is_some_and(|d| !d.belongs_to(pane, core, &market, tool))
        {
            self.fig_draft = None;
        }
        let draft = match self.fig_draft.as_mut() {
            Some(d) => d,
            None => {
                let (style, switches) = {
                    let b = self.backend.read(cx);
                    (b.fig_style(tool), b.tool_switches(tool).clone())
                };
                self.fig_draft.insert(FigDraft::new(
                    pane,
                    core,
                    market.clone(),
                    tool,
                    style,
                    switches,
                    node,
                ))
            }
        };
        // Every click, including the first, goes through `place`: a one-click tool simply finishes
        // on it.
        // The style the draft captured when it started, so a style edited mid-draw does not reach
        // the figure being placed. Read before the draft is dropped: it is the only source there
        // has ever been for a finished figure.
        let style = draft.style;
        let finished = draft.place(node);
        if let Some(kind) = finished {
            self.fig_draft = None;
            let fig = Figure::new(kind, style, moon_core::util::now_unix_ms_i64());
            let id = self
                .backend
                .read(cx)
                .figures
                .borrow_mut()
                .add(core, &market, fig);
            // Select a new figure immediately so its handles appear and it can be moved or deleted.
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
        let dist = pt_dist(pos, down);
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

    /// Grab a figure on modifier-left-click, preferring the selected figure's handles over the
    /// nearest body.
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
        if !store.has_visible(core, &market) {
            return false;
        }
        let selected = b
            .fig_selected
            .as_ref()
            .filter(|(c, m, _)| *c == core && *m == market)
            .map(|(_, _, id)| *id);
        // Give the selected figure's handles priority over all bodies.
        if let Some(sel_id) = selected {
            let grab = store
                .get(core, &market, sel_id)
                .and_then(|fig| pick_handle(&fig.kind, pos, map, threshold));
            if let Some(i) = grab {
                drop(store);
                self.fig_drag = Some(FigDrag {
                    core,
                    market,
                    id: sel_id,
                    pane,
                    grab: Grab::Handle(i),
                    last: map.node_at(pos),
                });
                // Publish the drag, as the body branch below does: the renderer suppresses a
                // dragged figure's fill, and an unpublished drag would keep re-baking the base
                // cache for the whole gesture.
                self.sync_fig_visual(cx);
                return true;
            }
        }
        // Otherwise select and drag the nearest figure body.
        let hit = pick_figure(store.visible(core, &market), pos, map, threshold);
        let Some(id) = hit else {
            drop(store);
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
        drop(store);
        self.backend.update(cx, |b, bcx| {
            b.fig_selected = Some((core, market.clone(), id));
            bcx.notify();
        });
        self.fig_drag = Some(FigDrag {
            core,
            market,
            id,
            pane,
            grab: Grab::Body,
            last: map.node_at(pos),
        });
        self.sync_fig_visual(cx);
        true
    }

    /// Update draft preview, active dragging, and figure hover from a mouse-move event.
    ///
    /// `pressed_left` reports whether the left button remains held. The return value reports an
    /// active drag or a later preview/hover change; cancelling a draft after drawing mode is disabled
    /// synchronizes visuals but can still return `false`, especially when outside the chart.
    pub(super) fn update_fig_pointer(
        &mut self,
        pos: (f32, f32),
        within: bool,
        pressed_left: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // Cancel the draft when the tool was disarmed while the cursor sat still. `sync_fig_visual`
        // already does this the moment it happens; this is the second guard, for a draft started
        // before that path could run.
        if !self.backend.read(cx).fig_draw_mode && self.fig_draft.is_some() {
            self.fig_draft = None;
            self.sync_fig_visual(cx);
        }
        if !within {
            // The pointer left the chart: the hover goes with it, since the readout follows the
            // hover and would otherwise keep being drawn for a figure the pointer is nowhere near.
            // A DRAG keeps its hover: the figure under the cursor is still the one being moved,
            // and the pointer is expected to leave the pane mid-drag.
            if self.fig_drag.is_none() && self.fig_hover.take().is_some() {
                self.sync_fig_visual(cx);
                return true;
            }
            return false;
        }
        // Edit an actively dragged figure in place and force the same immediate rebuild as an
        // order drag; otherwise the line would update only on data ticks and visibly lag.
        if pressed_left {
            if let Some(drag) = &self.fig_drag {
                // The map of the pane the drag STARTED on, never the one under the cursor. The
                // cursor may cross into a neighbouring pane of the stack, which shows another
                // market on its own time and price scale; a delta measured across two different
                // projections would teleport the figure instead of moving it by the mouse.
                let Some(map) = self.pane_map(drag.pane) else {
                    return false;
                };
                let cur = map.node_at(pos);
                let (dt_ms, dp) = (cur.time_ms - drag.last.time_ms, cur.price - drag.last.price);
                let (core, market, id, grab, dpane) = (
                    drag.core,
                    drag.market.clone(),
                    drag.id,
                    drag.grab,
                    drag.pane,
                );
                if let Some(d) = self.fig_drag.as_mut() {
                    d.last = cur;
                }
                let edited =
                    self.backend
                        .read(cx)
                        .figures
                        .borrow_mut()
                        .edit(core, &market, id, |fig| drag_figure(fig, grab, dt_ms, dp));
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
        let draft_pane = self
            .fig_draft
            .as_ref()
            .map(|d| d.pane)
            .filter(|dp| self.input.pane_at(pos.0, pos.1) == Some(*dp));
        let draft_pane = draft_pane.filter(|_| {
            // Same Delphi threshold the hover hit-test uses (INPUT_HOTPATH_NORMS §1): raw
            // MouseMove arrives far more often than the cursor moves, and each accepted move
            // rebuilds this pane's whole figure geometry.
            let due = super::trade::hover_probe_due(self.fig_draft_probe, pos);
            if due {
                self.fig_draft_probe = Some(pos);
            }
            due
        });
        if let Some(map) = draft_pane.and_then(|dp| self.pane_map(dp)) {
            let node = map.node_at(pos);
            match &mut self.fig_draft {
                Some(d) if d.cursor != node => {
                    d.cursor = node;
                    changed = true;
                }
                _ => {}
            }
        }
        // Drawing-mode hover previews which figure a modifier-click would select or drag. Gated by
        // the same cursor-movement threshold as order lines (docs-internal/INPUT_HOTPATH_NORMS.md
        // §1): raw MouseMove arrives far more often than the cursor actually moves, and this walks
        // every visible figure.
        if super::trade::hover_probe_due(self.fig_hover_probe, pos) {
            self.fig_hover_probe = Some(pos);
            let hover = self.fig_hit_at(pos, cx);
            if hover != self.fig_hover {
                self.fig_hover = hover;
                changed = true;
            }
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
        pick_figure(store.visible(core, &market), pos, &map, threshold)
    }

    /// Drop the selection when a click lands on no figure at all.
    ///
    /// Called for clicks the figure layer did NOT consume — which is most of them, since grabbing
    /// a figure needs the secondary modifier and an ordinary click belongs to trading and
    /// navigation. Without this a selection outlived every click that missed it: its handles kept
    /// sitting on the chart, and Delete/Alert kept pointing at a figure the user had stopped
    /// working on. Never consumes the click; the caller carries on with it.
    pub(super) fn fig_clear_selection_on_miss(&mut self, pos: (f32, f32), cx: &mut Context<Self>) {
        // Only for a selection made on THIS chart. `fig_selected` is global: a click in another
        // panel or in a detached window would otherwise silently drop a selection its user can
        // still see.
        let Some(pane) = self.input.pane_at(pos.0, pos.1) else {
            return;
        };
        let Some((core, market)) = self.fig_pane_key(pane) else {
            return;
        };
        if !self
            .backend
            .read(cx)
            .fig_selected
            .as_ref()
            .is_some_and(|(c, m, _)| *c == core && *m == market)
        {
            return;
        }
        // A click ON a figure is not a miss, modifier or not: pointing at a figure and losing its
        // handles for it would read as the click having gone somewhere else.
        if self.fig_hit_at(pos, cx).is_some() {
            return;
        }
        self.backend.update(cx, |b, bcx| {
            b.fig_selected = None;
            bcx.notify();
        });
        self.sync_fig_visual(cx);
    }

    /// Open the context menu for a right-clicked figure: arm it as a core alert,
    /// share it with every core on this market, or delete it.
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
        let state = {
            let b = self.backend.read(cx);
            let store = b.figures.borrow();
            store
                .get(core, &market, id)
                .map(|f| (f.alert, f.shared, f.can_alert(), f.can_share()))
                .map(|s| (s, store.owns(core, &market, id)))
        };
        let Some(((armed, shared, can_alert, can_share), owned)) = state else {
            return false;
        };
        // Select the right-clicked figure so its handles are visible.
        self.backend.update(cx, |b, bcx| {
            b.fig_selected = Some((core, market.clone(), id));
            bcx.notify();
        });
        self.sync_fig_visual(cx);
        let mut items: Vec<MoonMenuItem> = Vec::new();
        // First item: this is what a right-click on a figure is FOR most of the time. Arming,
        // sharing and deleting are one-shot actions; the look is what gets fiddled with.
        let view = cx.entity();
        let settings_target = crate::figstyle::FigStyleTarget {
            core,
            market: market.clone(),
            id,
            at: local_pos,
        };
        items.push(
            MoonMenuItem::with_key("fig-settings", t!("chart.fig_menu.settings").to_string())
                .on_click(move |_, window, app| {
                    window.close_context_menu(app);
                    view.update(app, |this: &mut Self, vcx| {
                        this.fig_settings = Some(settings_target.clone());
                        vcx.notify();
                    });
                }),
        );
        // A tool the core has no chart-object type for cannot be armed at all: offer the item only
        // where it would work, instead of failing after the click.
        if armed || can_alert {
            let label = if armed {
                t!("chart.fig_menu.alert_off")
            } else {
                t!("chart.fig_menu.alert_on")
            }
            .to_string();
            let backend = self.backend.clone();
            items.push(MoonMenuItem::with_key("fig-alert", label).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend.update(app, |b, _| {
                        b.toggle_selected_figure_alert();
                    });
                },
            ));
        }
        if shared || can_share {
            let label = if shared {
                t!("chart.fig_menu.unshare")
            } else {
                t!("chart.fig_menu.share")
            }
            .to_string();
            let backend = self.backend.clone();
            let market_share = market.clone();
            items.push(MoonMenuItem::with_key("fig-share", label).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend.update(app, |b, _| {
                        b.set_figure_shared(core, &market_share, id, !shared);
                    });
                },
            ));
        }
        let backend_del = self.backend.clone();
        let market_del = market.clone();
        // Deleting a figure this chart only SEES destroys another core's original for everyone.
        // Say so in the label rather than letting the two cases look identical.
        let delete_label = if owned {
            t!("chart.fig_menu.delete")
        } else {
            t!("chart.fig_menu.delete_shared")
        }
        .to_string();
        items.push(MoonMenuItem::with_key("fig-delete", delete_label).on_click(
            move |_, window, app| {
                window.close_context_menu(app);
                backend_del.update(app, |b, _| {
                    b.remove_figure(core, &market_del, id);
                });
            },
        ));
        window.open_moon_context_menu(cx, "chart-fig-menu", menu_pos, items, 200.0);
        cx.notify();
        true
    }

    /// Finish a figure drag and re-upsert its changed coordinates when it is armed as an alert.
    ///
    /// Called on mouse-up, and from the no-button motion path for a drag whose mouse-up was lost.
    /// Returns whether there was an active drag to finish.
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

    /// Closes the per-figure settings panel when what it edits stops existing: the figure deleted,
    /// or the pane it was opened on gone or showing another market. Called from the backend-notify
    /// path, so an edit made in
    /// another window closes it here too. Disarming the tool does NOT close it: the figure it
    /// edits is still on the chart, still selected and still editable.
    pub(super) fn drop_stale_fig_settings(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.fig_settings.clone() else {
            return;
        };
        let b = self.backend.read(cx);
        let alive = b
            .figures
                .borrow()
                .get(target.core, &target.market, target.id)
                .is_some()
            && self.chart.with_container(|c| {
                c.panes()
                    .iter()
                    .any(|p| p.core == target.core && p.market == target.market)
            });
        if !alive {
            self.fig_settings = None;
            cx.notify();
        }
    }

    /// Publish drawing mode, draft preview, hover, and selection to the chart engine.
    ///
    /// Mouse events and the Backend observer both call this so a tool switch, a finished draft or a
    /// new selection reaches the engine on the same frame it happens.
    pub(super) fn sync_fig_visual(&mut self, cx: &mut Context<Self>) {
        // A tool switch abandons the draft: its placed nodes belong to the tool that started it,
        // and its preview would otherwise keep drawing until the next click discards it. Choosing
        // Cursor abandons it for the same reason, and HERE rather than on the next mouse move over
        // the chart — this runs from the backend observer, so the half-placed nodes go the moment
        // the tool is disarmed instead of lingering until the cursor happens to pass over them.
        let (tool, armed) = {
            let b = self.backend.read(cx);
            (b.fig_tool, b.fig_draw_mode)
        };
        if self
            .fig_draft
            .as_ref()
            .is_some_and(|d| d.tool != tool || !armed)
        {
            self.fig_draft = None;
        }
        let b = self.backend.read(cx);
        let key = self
            .fig_draft
            .as_ref()
            .map(|d| (d.core, d.market.clone()))
            .or_else(|| b.fig_selected.as_ref().map(|(c, m, _)| (*c, m.clone())))
            .or_else(|| self.input.hovered_pane.and_then(|p| self.fig_pane_key(p)));
        let draft = self.fig_draft.as_ref().and_then(FigDraft::preview);
        let selected = b
            .fig_selected
            .as_ref()
            .filter(|(c, m, _)| key.as_ref().is_some_and(|(kc, km)| kc == c && km == m))
            .map(|(_, _, id)| *id);
        let visual = FigureVisual {
            key,
            draft,
            hovered: self.fig_hover,
            selected,
            dragging: self.fig_drag.as_ref().map(|d| d.id),
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
    pub(super) fn fig_resync(&mut self, cx: &Context<Self>) {
        let b = self.backend.read(cx);
        self.chart.sync_orders_if_visible(&b.session, true);
    }
}
