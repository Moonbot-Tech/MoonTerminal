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

/// Glyph shown beside the crosshair while the Sells-to-zone drawing mode is armed.
///
/// A mark, not a word: it sits ON the chart next to the pointer, where a label would cover price.
/// Taken from the same geometric/dingbat range as the toolbar's own tool glyphs (`ToolDef::glyph`),
/// which the UI font is already proven to draw.
const SELLS_ZONE_BADGE: &str = "✎";

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
    /// Whether any step actually moved the figure.
    ///
    /// A tool may refuse a step — Moonbot's own Fibonacci refuses a purely sideways drag, since its
    /// levels run the whole chart and nothing it draws would move — and an armed figure that did not
    /// move must not be re-upserted: that would write a blob into another program's object to say
    /// nothing, and hand it back the values this side repaired on decode.
    pub moved: bool,
}

impl ChartPanel {
    /// Whether the Sells-to-zone drawing mode is armed, for the input paths that must treat the
    /// chart as modal while it is.
    pub(super) fn sells_zone_armed(&self, cx: &mut Context<Self>) -> bool {
        self.backend.read(cx).sells_zone_armed()
    }

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
        //
        // NOT while the Sells-to-zone mode is armed: the mode is a drawing posture, so a click that
        // happens to land within grab range of an old figure must still start a band rather than
        // silently turn into a drag. Editing figures resumes when the mode ends.
        //
        // Grabbing works with no tool armed — that is what the Cursor entry leaves behind — but
        // PLACING a node needs one. Without this the click would fall through to trading, which is
        // exactly right: with no tool selected a modifier-click that hits nothing is not a draw.
        let (tool, draw_mode, armed) = {
            let b = self.backend.read(cx);
            (b.fig_tool, b.fig_draw_mode, b.sells_zone_armed())
        };
        if self.fig_draft.is_none() && !armed && self.try_fig_grab(pane, pos, &map, cx) {
            return true;
        }
        if !draw_mode {
            return false;
        }
        let node = map.node_at(pos);
        self.fig_draw_click(pane, tool, node, &[], armed, cx);
        // Retain the press for the drag-release gesture in `try_fig_release`. On the draft itself,
        // so a figure finished by this very click leaves no press behind at all.
        if let Some(d) = self.fig_draft.as_mut() {
            d.down = Some(pos);
        }
        true
    }

    /// Place one or more nodes in drawing mode, completing the figure once the tool has all its
    /// clicks.
    ///
    /// `node` is the click's own; `derived` is whatever the tool built from a press-drag-release
    /// gesture, empty for an ordinary click. They go in as ONE placement on purpose: the draft is
    /// resolved once, the visual is published once, and no half-built shape reaches the screen or a
    /// second draft between them. Nodes past the one that finishes the figure are dropped rather
    /// than starting another.
    fn fig_draw_click(
        &mut self,
        pane: usize,
        tool: FigureTool,
        node: FigNode,
        derived: &[FigNode],
        armed: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((core, market)) = self.fig_pane_key(pane) else {
            return;
        };
        // Discard a draft started on another pane, chart, tool or drawing mode; drawing follows the
        // current click. Tested here as well as in `sync_fig_visual` so the answer does not depend
        // on an observer having run between a keypress and this click.
        if self
            .fig_draft
            .as_ref()
            .is_some_and(|d| !d.belongs_to(pane, core, &market, tool, armed))
        {
            self.fig_draft = None;
        }
        // Derived nodes describe the draft that was alive when the gesture's press happened. If the
        // reset above just took that draft — the pane's coin changed under it, or the tool did — a
        // fresh one starts from this node alone: a derived vertex in it would be a point the user
        // never aimed at.
        let derived = if self.fig_draft.is_some() { derived } else { &[] };
        let draft = match self.fig_draft.as_mut() {
            Some(d) => d,
            None => {
                let (style, switches) = {
                    let b = self.backend.read(cx);
                    (b.fig_style(tool), b.tool_switches(tool).clone())
                };
                self.fig_draft.insert(
                    FigDraft::new(pane, core, market.clone(), tool, style, switches, node)
                        .for_sells_zone(armed),
                )
            }
        };
        // Every click, including the first, goes through `place`: a one-click tool simply finishes
        // on it.
        // The style the draft captured when it started, so a style edited mid-draw does not reach
        // the figure being placed. Read before the draft is dropped: it is the only source there
        // has ever been for a finished figure.
        let style = draft.style;
        let sells_zone = draft.sells_zone;
        // All of them against THIS draft, rather than re-entering and being tested against a draft
        // the first node may already have finished. The break is a guard, not a live branch: the
        // registry contract test pins `2 + derived == clicks`, so only the last node can complete
        // the figure — but a tool that ever broke that must drop its surplus, not spill it into a
        // second draft.
        let mut finished = draft.place(node);
        for &extra in derived {
            if finished.is_some() {
                break;
            }
            finished = draft.place(extra);
        }
        if let Some(kind) = finished {
            self.fig_draft = None;
            // A Sells-to-zone band ends HERE and goes no further: the prices go to the core and
            // the figure is dropped, never reaching the store, `figures.json` or the selection.
            // Moonbot draws the same band as a throwaway `CO_SysRect`.
            //
            // The MODE stays armed. Spreading sells over a chart is aiming work — the first band
            // is rarely the last — so the key turns the mode on and off, and every pair of
            // Ctrl+clicks in between sends its own band. Ctrl+S again, Escape, or picking a tool
            // ends it.
            if sells_zone {
                match kind.price_band() {
                    Some((a, z)) => self.send_sells_to_zone(core, &market, a, z, cx),
                    // Unreachable while the mode arms the Zone tool, and logged rather than
                    // swallowed if that ever stops being true: the band would otherwise vanish
                    // with no command and no trace of why.
                    None => log::warn!(
                        "sells to zone: {} drew no band, nothing sent",
                        tool.def().key
                    ),
                }
            } else {
                let fig = Figure::new(kind, style, moon_core::util::now_unix_ms_i64() as f64);
                let id = self
                    .backend
                    .read(cx)
                    .figures
                    .borrow_mut()
                    .add(core, &market, fig);
                // Select a new figure immediately so its handles appear and it can be moved or
                // deleted.
                self.backend.update(cx, |b, bcx| {
                    b.fig_selected = Some((core, market.clone(), id));
                    bcx.notify();
                });
            }
        }
        self.sync_fig_visual(cx);
    }

    /// Advance a draft with a press-drag-release gesture.
    ///
    /// A release far enough from the placed node counts as the next click, including the next
    /// triangle vertex. A release near the press is not a gesture, so the normal click-click flow
    /// continues. Returns whether the release advanced the draft.
    ///
    /// `draw_mod` reports whether the secondary modifier is still held, and a Sells-to-zone band
    /// requires it: its finishing "click" sends a live bulk move, so a modifier dropped part way
    /// through the drag must leave the band unsent rather than complete it on the way up.
    pub(super) fn try_fig_release(
        &mut self,
        pos: (f32, f32),
        draw_mod: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // The mode as it stands now, for the same reason the press path reads it: the release must
        // not finish a band against a mode that was dropped since its first click.
        let armed = self.backend.read(cx).sells_zone_armed();
        let Some(d) = self.fig_draft.as_ref() else {
            return false;
        };
        if d.needs_modifier() && !draw_mod {
            return false;
        }
        let Some(down) = d.down else {
            return false;
        };
        if pt_dist(pos, down) < self.fig_gesture_threshold() {
            return false;
        }
        let (pane, tool) = (d.pane, d.tool);
        // The release belongs to the pane the draft is being drawn on. A stack shows a different
        // market on its own time and price scale in the pane next door, and the preview stops
        // following the cursor the moment it leaves this one — so finishing there would land a
        // figure built from a projection the user was never shown. The glass/order-book strip is
        // part of its pane and still finishes, which is deliberate and older than this gesture.
        if self.input.pane_at(pos.0, pos.1) != Some(pane) {
            return false;
        }
        let Some(map) = self.pane_map(pane) else {
            return false;
        };
        let node = map.node_at(pos);
        // Read BEFORE the release node is placed: what a tool derives from a gesture is defined
        // against a draft holding the press alone, and placing first would make that test fail.
        // The release node and the derived ones then go in together, so a tool drawn by dragging
        // PART of itself finishes here rather than waiting for the clicks it is still short of.
        let rest = self.fig_gesture_rest(pos, node, &map);
        self.fig_draw_click(pane, tool, node, &rest, armed, cx);
        // The button is up, so this draft holds no press any more. Carrying the release point
        // forward would say a press is held with no button down at all, and the preview reads that
        // to decide a held pointer is this draft's.
        if let Some(d) = self.fig_draft.as_mut() {
            d.down = None;
        }
        true
    }

    /// Distance a press must travel before its release counts as the next click rather than as
    /// click jitter.
    ///
    /// One statement of the rule, read by the release that applies it and by the preview that has
    /// to agree with it: a gesture previewing a figure the release would not build is worse than
    /// no preview at all.
    fn fig_gesture_threshold(&self) -> f32 {
        2.0 * HIT_PX * self.last_ppp.max(1.0)
    }

    /// The draft's own pane, when the pointer is on it.
    ///
    /// A pane of a stack shows another market on its own time and price scale, so a draft has
    /// nothing to say about a pointer that has wandered into one.
    fn fig_draft_pane(&self, pos: (f32, f32)) -> Option<usize> {
        self.fig_draft
            .as_ref()
            .map(|d| d.pane)
            .filter(|dp| self.input.pane_at(pos.0, pos.1) == Some(*dp))
    }

    /// Whether a press-drag-release gesture is live for the pointer at `pos`.
    ///
    /// The single answer to "could this pointer still be drawing", and deliberately CHEAP: a held
    /// button, on the chart, on the draft's own pane. It is asked on every raw mouse move, ahead of
    /// the movement threshold, because the answer turning false is what retires a stale preview —
    /// and a preview may not wait for the next accepted move to stop showing a figure nothing will
    /// build. What the gesture has actually derived costs a projection and is computed only for a
    /// move the threshold accepted.
    fn fig_gesture_live(&self, pos: (f32, f32), within: bool, pressed_left: bool) -> bool {
        within && pressed_left && self.fig_draft_pane(pos).is_some()
    }

    /// The nodes the draft's tool derives from the gesture ending at `at`, empty when it derives
    /// none.
    ///
    /// Answered for the pointer position the caller is previewing or placing — never a fresher
    /// one: the derived geometry and the node it is measured against have to describe the same
    /// instant, or the preview shows a figure built from two different pointer positions.
    fn fig_gesture_rest(&self, pos: (f32, f32), at: FigNode, map: &PaneMap) -> Vec<FigNode> {
        let Some(d) = self.fig_draft.as_ref() else {
            return Vec::new();
        };
        let Some(down) = d.down else {
            return Vec::new();
        };
        // Still inside the jitter box: this press is a click so far, and the release would place a
        // single node. Previewing the whole figure here would show one that never lands.
        if pt_dist(pos, down) < self.fig_gesture_threshold() {
            return Vec::new();
        }
        let Some(rest) = d.drag_rest_rule() else {
            return Vec::new();
        };
        // Both ends are read back from NODES rather than from raw pixels. The start, because the
        // chart moves under a live gesture — it follows the live edge and autofits Y — so the press
        // pixel and the node placed there drift apart, and geometry raised off the pixel would
        // stand on a base that is no longer under it. The far end, because the pointer projection
        // clamps to the visible price: a pointer below the plot — the time-axis strip belongs to
        // the pane too — lands a base end at the bottom price while the raw pixel says otherwise.
        let (from, to) = (map.px_of(d.nodes[0]), map.px_of(at));
        let derived: Vec<FigNode> = rest(from, to)
            .into_iter()
            // Unclamped: a derived point is meant to be able to leave the visible range, and the
            // pointer-clamping projection would flatten it onto the view's edge price.
            .map(|p| map.node_at_unclamped(p))
            .collect();
        // A derived price must be one a chart can carry. Unclamped, it may run off the top of a
        // wide view and come out at or below zero — a price no market has, which would reach
        // `figures.json` and, for an armed triangle, the core's chart-object blob. The gesture then
        // derives NOTHING and the release places its own node alone: the figure is finished by the
        // clicks it is short of, which is the behaviour before this feature and not a silent
        // half-figure. Same bounds the tools that compute prices already apply to their own
        // (`mb_fib::is_price`, `fib_retracement`'s level skip).
        if derived
            .iter()
            .any(|n| !(n.price.is_finite() && n.price > 0.0 && n.price <= f32::MAX as f64))
        {
            return Vec::new();
        }
        derived
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
                    moved: false,
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
            moved: false,
        });
        self.sync_fig_visual(cx);
        true
    }

    /// Update draft preview, active dragging, and figure hover from a mouse-move event.
    ///
    /// `pressed_left` reports whether the left button remains held. The return value reports an
    /// active drag or a later preview/hover change; cancelling a draft after drawing mode is disabled
    /// synchronizes visuals but can still return `false`, especially when outside the chart.
    ///
    /// It is NOT a "repaint the GPUI tree" request, and every caller discards it: everything this
    /// updates reaches the screen through the chart's own pass. A caller that ever does need the
    /// tree repainted has to decide that for itself.
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
        // Retiring the gesture's derived nodes happens HERE, in one place, before any early return
        // can skip it. They are what the preview shows INSTEAD of the tool's partial shape, so a
        // set left behind by a gesture that has ended keeps a finished figure painted on a chart
        // that will not build it — and every way a gesture ends (the button up, the pointer off the
        // chart or off the draft's own pane, a release that reached no handler at all) is the same
        // fact, said once. Ungated by the movement threshold on purpose: a preview must stop the
        // moment its gesture does, not on the next accepted move. UPDATING the set is the other
        // half and costs a projection, so it waits for the threshold, below.
        let mut changed = false;
        if !self.fig_gesture_live(pos, within, pressed_left) {
            if let Some(d) = self.fig_draft.as_mut() {
                changed |= d.set_drag_rest(Vec::new());
            }
            // Retire the movement probe with the gesture, exactly as leaving the chart does. The
            // threshold measures from the last ACCEPTED position, so a pointer that steps into the
            // next pane and back within a pixel would otherwise keep showing the partial shape
            // until it moved far enough for a move to be accepted again.
            self.fig_draft_probe = None;
        }
        if !within {
            // The pointer left the chart: the hover goes with it, since the readout follows the
            // hover and would otherwise keep being drawn for a figure the pointer is nowhere near.
            // A DRAG keeps its hover: the figure under the cursor is still the one being moved,
            // and the pointer is expected to leave the pane mid-drag.
            changed |= self.fig_drag.is_none() && self.fig_hover.take().is_some();
            if changed {
                self.sync_fig_visual(cx);
                return true;
            }
            return false;
        }
        // Edit an actively dragged figure in place and force the same immediate rebuild as an
        // order drag; otherwise the line would update only on data ticks and visibly lag.
        if pressed_left {
            if self.fig_drag.is_some() && changed {
                // A draft and a figure drag can only coexist after a stranded drag, but the clear
                // above has already changed what the preview holds, and this branch returns without
                // reaching the publish at the end of the function.
                self.sync_fig_visual(cx);
            }
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
                if let Some(d) = self.fig_drag.as_mut() {
                    d.moved |= edited;
                }
                if edited {
                    // No `cx.notify()`, for the same reason as `sync_fig_visual`: a dragged figure
                    // is redrawn by the chart's own pass out of the userdata layer rebuilt here,
                    // and no GPUI element reads the drag. Order-line dragging keeps a notify but
                    // PACES it (`render_input.rs`, `DRAG_NOTIFY_MIN_INTERVAL`) precisely because an
                    // unpaced one repaints the whole window at the mouse-event rate — roughly 110 a
                    // second, above anything the own pass can present. Figures need neither: with
                    // nothing in the tree to refresh, the cheapest correct rate is none at all.
                    self.fig_resync(cx);
                }
                // Keep the crosshair under the pointer, matching order-line dragging.
                self.input.cursor = Some(pos);
                self.input.hovered_pane = Some(dpane);
                self.sync_native_cursor(cx);
                return true;
            }
            // No figure drag under the held button, but a draft may be mid-gesture: press, drag,
            // release is how Moonbot draws, and its preview has to follow the cursor exactly as it
            // does between two clicks. Everything below the draft block is hover work — which
            // figure a click would grab — and a button already down is past deciding that.
            if self.fig_draft.is_none() {
                return false;
            }
        }
        // Move the draft's preview endpoint with the cursor while it remains on the draft pane.
        let draft_pane = self.fig_draft_pane(pos).filter(|_| {
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
            // From the same accepted position as the cursor node above, so the previewed figure is
            // the one `make` would build from the nodes being shown rather than a blend of two
            // pointer positions a fraction of a pixel apart.
            let rest = if pressed_left {
                self.fig_gesture_rest(pos, node, &map)
            } else {
                Vec::new()
            };
            if let Some(d) = self.fig_draft.as_mut() {
                if d.cursor != node {
                    d.cursor = node;
                    changed = true;
                }
                changed |= d.set_drag_rest(rest);
            }
        }
        // Drawing-mode hover previews which figure a modifier-click would select or drag. Gated by
        // the same cursor-movement threshold as order lines (docs-internal/INPUT_HOTPATH_NORMS.md
        // §1): raw MouseMove arrives far more often than the cursor actually moves, and this walks
        // every visible figure.
        // Reached with a held button only for a draft mid-gesture (everything else returned above),
        // and a gesture in progress has nothing to decide about the next click's target.
        if !pressed_left && super::trade::hover_probe_due(self.fig_hover_probe, pos) {
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
        // One lookup for everything the menu needs about this figure: `FigureStore::get` falls back
        // to scanning every core's list, so a second identical fetch would pay for that twice.
        // `band` is the pair of prices it would spread sells across, `None` for a tool that draws
        // no band — which is most of them.
        let state = {
            let b = self.backend.read(cx);
            let store = b.figures.borrow();
            store
                .get(core, &market, id)
                .map(|f| {
                    (
                        (f.alert, f.shared, f.can_alert(), f.can_share()),
                        f.kind.price_band(),
                    )
                })
                .map(|(s, band)| (s, band, store.owns(core, &market, id)))
        };
        // Whether the core can be commanded at all. A chart alert is attempted once and never
        // retried, so `Backend::set_figure_alert` refuses while the core is not `Ready` — and this
        // menu must not offer a click that would then do nothing.
        let core_ready = self.backend.read(cx).core_can_command(core);
        let Some(((armed, shared, can_alert, can_share), band, owned)) = state else {
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
        // The point the panel is anchored at travels beside the target, not inside it: it is where
        // THIS host puts the frame, and the other hosts of the same panel have no click to speak of.
        // The MENU's point, in window coordinates — the settings frame is snapped to the window the
        // way the menu itself is, so it needs the same coordinate space and not the slot's.
        let settings_target = (
            crate::figstyle::FigStyleTarget {
                core,
                market: market.clone(),
                id,
            },
            menu_pos,
        );
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
        // Spreading this market's sells across the band the figure already describes — the standing
        // counterpart of drawing one in Ctrl+S mode. Offered for any figure that IS a band, with no
        // `core_ready` gate: this is an ordinary trade command that queues on the core's channel
        // like the panic-sell and join hotkeys, unlike the alert entry below, whose upsert is
        // attempted once and never retried. Gating it would also make the two ways of naming a band
        // behave differently.
        if let Some((a, z)) = band {
            let view_zone = cx.entity();
            let market_zone = market.clone();
            items.push(
                MoonMenuItem::with_key(
                    "fig-sells-zone",
                    t!("chart.fig_menu.sells_to_zone").to_string(),
                )
                .on_click(move |_, window, app| {
                    window.close_context_menu(app);
                    view_zone.update(app, |this: &mut Self, vcx| {
                        this.send_sells_to_zone(core, &market_zone, a, z, vcx);
                    });
                }),
            );
        }
        // A tool the core has no chart-object type for cannot be armed at all, and neither arming
        // nor disarming reaches a core that is not connected: offer the item only where it would
        // work, instead of failing after the click.
        if (armed || can_alert) && core_ready {
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
        if drag.moved {
            self.backend.update(cx, |b, _| {
                b.reupsert_figure_alert(drag.core, &drag.market, drag.id);
            });
        }
        self.sync_fig_visual(cx);
        true
    }

    /// Closes the per-figure settings panel when what it edits stops existing: the figure deleted,
    /// or the pane it was opened on gone or showing another market. Called from the backend-notify
    /// path, so an edit made in
    /// another window closes it here too. Disarming the tool does NOT close it: the figure it
    /// edits is still on the chart, still selected and still editable.
    pub(super) fn drop_stale_fig_settings(&mut self, cx: &mut Context<Self>) {
        let Some((target, _)) = self.fig_settings.clone() else {
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
        // The Sells-to-zone mode changes what a FINISHING click does while leaving the tool alone,
        // so a draft started on the other side of that switch is abandoned here too: entering the
        // mode must not adopt a half-drawn figure as a live command, and leaving it must not turn a
        // half-drawn command into a stored figure.
        let (tool, drawing, sells_zone) = {
            let b = self.backend.read(cx);
            (b.fig_tool, b.fig_draw_mode, b.sells_zone_armed())
        };
        if self
            .fig_draft
            .as_ref()
            .is_some_and(|d| d.tool != tool || !drawing || d.sells_zone != sells_zone)
        {
            self.fig_draft = None;
        }
        // The badge riding the crosshair. Published from here rather than from the render path
        // because this runs on the backend observer: the mode becomes visible on the keypress
        // itself, through the chart's own present, instead of waiting for the next GPUI repaint —
        // and it costs no userdata rebuild.
        self.chart
            .set_cursor_badge(sells_zone.then_some(SELLS_ZONE_BADGE));
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
            //
            // NO `cx.notify()`. Everything this changes — the hover highlight, the selection
            // handles, the draft preview — is drawn by the chart's OWN pass out of the userdata
            // layer rebuilt on the line above, and nothing in the GPUI tree reads `fig_hover`,
            // `fig_selected` or the draft. A notify here dirties the view AND every ancestor, and a
            // re-rendered root bypasses each descendant's view cache, so one hover change repainted
            // the whole window — measured on the fixture bench at 90 chart+shell renders per second
            // under a mouse storm against 1/s with no figures. News marks take the same route for
            // the same reason (`news.rs::publish_news_marks`); a caller that ever needs the GPUI
            // tree repainted must ask for it itself.
            self.fig_resync(cx);
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
