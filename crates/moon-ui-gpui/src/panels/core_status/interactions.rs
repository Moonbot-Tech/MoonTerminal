//! Core Status user-interaction handlers: chart-span and server selection, tree expansion, the core
//! multi-select filter, inline server rename, sort, and presentation mode. Split out of `mod.rs` as
//! the mutation half of the panel, distinct from its render-cache pipeline and its rendering.

use std::collections::HashSet;
use std::net::IpAddr;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonInputEvent, MoonInputState,
    MoonNotification, MoonPalette, MoonWindowExt as _, h_flex, v_flex,
};

use super::by_ip_header::ByIpDragAnchor;
use super::by_ip_widths::{ByIpCol, MAX_COL_W, MIN_COL_W};
use super::model::ServerKey;
use super::update_menu;
use super::{ChartWindow, CoreStatusMode, CoreStatusView, ordering, server_view};
use crate::design;
use moon_core::feed::{ConnStatus, UpdateTarget};
use moon_core::session::CoreId;
use rust_i18n::t;

/// Which of the footer's three bulk buttons opened the confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FleetUpdateKind {
    /// Every offerable core in the panel's scope, or in its selection.
    All,
    /// Only those behind the fleet's newest build.
    Behind,
    /// Every offerable core, to a build name the operator types inside the confirm.
    Named,
}

/// Exactly what a footer bulk update would enqueue, resolved before the confirm opens.
pub(super) struct FleetUpdatePlan {
    /// Cores to enqueue, in on-screen order.
    pub(super) cores: Rc<[CoreId]>,
    /// Distinct server lanes they span -- the queue runs one core at a time per lane, and the
    /// confirm says so.
    pub(super) lanes: usize,
}
impl CoreStatusView {
    /// Record where a By IP header divider drag began.
    ///
    /// Called once per drag, from the handle's `on_drag` constructor, which GPUI runs a single time
    /// after the drag threshold is crossed. The anchor is in LOGICAL (pre-shrink) width space — see
    /// [`Self::drag_by_ip_col`] for why that matters.
    ///
    /// Args:
    ///     anchor: The column, the pointer x, and the column's logical width at the grab.
    ///
    /// Returns:
    ///     Nothing; no repaint, because nothing has moved yet.
    pub(super) fn begin_by_ip_resize(&mut self, anchor: ByIpDragAnchor) {
        self.by_ip_drag = Some(anchor);
    }

    /// Apply a live By IP divider drag: the anchored width plus the pointer's travel since the grab.
    ///
    /// Deliberately NOT `pointer_x - cell_origin_x`, which is how MoonUI's data table does it. The
    /// By IP header puts a `flex_1` spacer between IP and CPU, so every column right of it is
    /// right-anchored: widening one moves its OWN left edge left by the same amount, and the next
    /// event measures against the moved origin. That loop triples the sensitivity per frame. An
    /// anchor captured once at the grab is immune to relayout.
    ///
    /// The anchor is the LOGICAL width, so on a shrunk panel the column tracks the pointer at the
    /// shrink factor's speed rather than 1:1. That is the correct trade: anchoring on the PAINTED
    /// width would write back an already-scaled value, the resolver would scale it a second time,
    /// and the column would jump narrower on the first pixel of the drag.
    ///
    /// Args:
    ///     col: Column whose divider is being dragged.
    ///     pointer_x: Current pointer x, in window pixels.
    ///     cx: View context; the width bag's observer persists and repaints.
    ///
    /// Returns:
    ///     Nothing. A drag for a different column than the live anchor, or with no anchor at all, is
    ///     ignored rather than guessed at.
    pub(super) fn drag_by_ip_col(&mut self, col: ByIpCol, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(anchor) = self.by_ip_drag else {
            return;
        };
        if anchor.col != col {
            return;
        }
        let width = (anchor.width + (pointer_x - anchor.mouse_x)).clamp(MIN_COL_W, MAX_COL_W);
        self.by_ip_col_widths.update(cx, |state, cx| {
            if state.column_widths.get(col.key()).copied() == Some(width) {
                return;
            }
            state.set_column_width(col.key(), width);
            cx.notify();
        });
    }

    /// Restore automatic width for one By IP column, or for every one of them.
    ///
    /// Mirrors the `MoonDataTable` divider gesture so the two views answer the same input: a plain
    /// double-click drops this column back to its design width, Shift+double-click drops all of them
    /// (the toolbar button's equivalent, and the only route to it in a detached window — see
    /// `toolbar_buttons`).
    ///
    /// An already-clear bag changes nothing and must NOT notify: the observer would otherwise arm
    /// `layout_dirty` and schedule a layout write for a double-click that did nothing.
    ///
    /// Args:
    ///     col: Column to reset when `all` is false.
    ///     all: Reset every column instead of just `col`.
    ///     cx: View context; the width bag's observer persists and repaints.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn reset_by_ip_col(&mut self, col: ByIpCol, all: bool, cx: &mut Context<Self>) {
        self.by_ip_col_widths.update(cx, |state, cx| {
            let changed = if all {
                let had_any = !state.column_widths.is_empty();
                state.column_widths.clear();
                had_any
            } else {
                state.column_widths.remove(col.key()).is_some()
            };
            if changed {
                cx.notify();
            }
        });
    }

    /// Switch the detached-window chart span and repaint.
    ///
    /// Args:
    ///     window: Requested X-axis span.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; only the span and a repaint change.
    pub(super) fn set_chart_window(&mut self, window: ChartWindow, cx: &mut Context<Self>) {
        if self.chart_window != window {
            self.chart_window = window;
            cx.notify();
        }
    }

    /// Select which server's chart the detached window shows (from a server-row click).
    ///
    /// Args:
    ///     key: Clicked server identity.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; only the selection and a repaint change.
    pub(super) fn select_chart_server(&mut self, key: ServerKey, cx: &mut Context<Self>) {
        // A server-row click charts the machine aggregate, so any per-core selection is cleared.
        if self.chart_server != Some(key) || self.chart_core.is_some() {
            self.chart_server = Some(key);
            self.chart_core = None;
            cx.notify();
        }
    }

    /// Chart one specific core (from a core-row click in the expanded server list) instead of the
    /// server aggregate. Clicking the already-charted core clears it, reverting to the server.
    ///
    /// Args:
    ///     id: Clicked core identity.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; only the selection and a repaint change.
    pub(super) fn select_chart_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        self.chart_core = if self.chart_core == Some(id) {
            None
        } else {
            Some(id)
        };
        cx.notify();
    }

    /// Toggle one server's expansion from a chevron click (the headless tree does not do it).
    ///
    /// Args:
    ///     key: Server identity whose folder to expand or collapse.
    ///     cx: View context used to update the tree state and repaint.
    ///
    /// Returns:
    ///     Nothing; the tree's expanded set flips for this server.
    pub(super) fn toggle_server_expand(&mut self, key: ServerKey, cx: &mut Context<Self>) {
        let id = SharedString::from(key.tree_id());
        self.tree_state.update(cx, |state, cx| {
            let mut ids = state.expanded_ids().into_iter().collect::<HashSet<_>>();
            if !ids.remove(&id) {
                ids.insert(id);
            }
            state.set_expanded(ids, cx);
        });
        cx.notify();
    }

    /// Apply a flat-table header sort from a column click.
    ///
    /// Args:
    ///     key: Column key from the table header.
    ///     ascending: Whether the click requested ascending order.
    ///     cx: View context used to repaint with the new order.
    ///
    /// Returns:
    ///     Nothing; only the sort state and a repaint change.
    pub(super) fn set_flat_sort(&mut self, key: &str, ascending: bool, cx: &mut Context<Self>) {
        let next =
            super::ordering::restore_flat_sort(Some(moon_core::config::TableSortPreference {
                column: key.to_string(),
                ascending,
            }));
        if self.flat_sort != next {
            self.flat_sort = next;
            let preference = self.flat_sort.as_ref().map(|(column, ascending)| {
                moon_core::config::TableSortPreference {
                    column: column.clone(),
                    ascending: *ascending,
                }
            });
            crate::persistence::table_persist::set_sort(
                &self.backend,
                &self.widths_id,
                preference,
                cx,
            );
            cx.notify();
        }
    }

    /// Apply a By IP header sort from a column click: flip direction on the active column, else select
    /// the newly clicked column ascending. Warnings still pin to the top (enforced in `rebuild_cache`).
    ///
    /// Args:
    ///     field: The column the header click chose.
    ///     cx: View context; the cache is rebuilt so the tree reorders, then a repaint is requested.
    ///
    /// Returns:
    ///     Nothing; the group sort state, group order, and tree change.
    pub(super) fn set_group_sort(
        &mut self,
        field: super::ordering::GroupSortField,
        cx: &mut Context<Self>,
    ) {
        let (current, ascending) = self.group_sort;
        self.group_sort = if current == field {
            (field, !ascending)
        } else {
            (field, true)
        };
        let preference =
            (self.group_sort != (super::ordering::GroupSortField::Name, true)).then(|| {
                moon_core::config::TableSortPreference {
                    column: self.group_sort.0.key().to_string(),
                    ascending: self.group_sort.1,
                }
            });
        let id = crate::persistence::table_persist::ctx_id("core-status-by-ip", self.detached);
        crate::persistence::table_persist::set_sort(&self.backend, &id, preference, cx);
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Toggle one core in the retained Classic filter, or toggle its All item.
    ///
    /// `Some(id)` toggles one core. `None` clears the explicit selection back to the
    /// empty-means-all state. Auto mode owns and pins the effective scope, so this method becomes a
    /// no-op.
    ///
    /// Args:
    ///     id: Core to toggle, or `None` for the All row.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; Classic updates the retained filter and cache, while Auto changes neither.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        if !crate::controls::toggle_core_selection(&mut self.sel_cores, id) {
            return;
        }
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Replace the retained Classic filter with the one the Profit Monitor broadcast.
    ///
    /// `apply_core_broadcast` owns the release / ignore / intersect rule shared by every adopting
    /// panel. The retained set is written even under Auto, where it is dormant, so a later switch
    /// back to Classic shows the terminal's current core focus rather than a filter from before;
    /// only the rebuild is skipped there.
    ///
    /// Args:
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a broadcast about other scopes and an unchanged selection both rebuild nothing.
    pub(super) fn adopt_broadcast_core_filter(&mut self, cx: &mut Context<Self>) {
        let broadcast = self.backend.read(cx).core_filter().clone();
        // Nothing published and nothing retained: leave before paying for the scope's core list.
        if broadcast.is_empty() && self.sel_cores.is_empty() {
            return;
        }
        let available: Vec<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if !crate::controls::apply_core_broadcast(&mut self.sel_cores, &broadcast, available) {
            return;
        }
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Toggle every still-available core from one exchange section in the Classic filter.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Rendered ids that left this panel's group are ignored. Auto mode leaves the retained Classic
    /// selection and cache unchanged.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a Classic change rebuilds once, while stale-only and Auto-owned calls are
    ///     no-ops.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<CoreId>,
        cx: &mut Context<Self>,
    ) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        let available = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.rebuild_cache(cx);
            cx.notify();
        }
    }

    /// Hide or show the whole By-IP address column.
    ///
    /// One state for the whole column, not one per server: a fleet of servers is masked or shown in
    /// a single click, and the panel does not have to hold — or leak — which rows a user happened to
    /// open. The state is transient and never persisted, so it lasts only as long as this panel.
    ///
    /// Deliberately NOT tied to focus. An earlier per-row reveal was cleared by the panel's blur,
    /// and a docked panel loses focus on nearly any click, so an address vanished as fast as it
    /// appeared.
    ///
    /// Args:
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; the mask flag is transient and never persisted.
    pub(super) fn toggle_ip_mask(&mut self, cx: &mut Context<Self>) {
        self.ip_masked = !self.ip_masked;
        cx.notify();
    }

    /// Begin inline renaming of one server, seeding the field with its current display name.
    ///
    /// Args:
    ///     key: Server identity being renamed. Only address servers persist a name.
    ///     current: Current display name used as the initial field value.
    ///     window: Host window used to build and focus the input state.
    ///     cx: View context used to create the input entity and subscribe to its commit events.
    ///
    /// Returns:
    ///     Nothing; the input entity and its subscription live until the edit commits or cancels.
    pub(super) fn start_rename(
        &mut self,
        key: ServerKey,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(current));
        cx.subscribe(&state, move |this, state, event: &MoonInputEvent, cx| {
            if matches!(
                event,
                MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                let value = state.read(cx).value().to_string();
                this.commit_rename(key, value, cx);
            }
        })
        .detach();
        self.edit_input = Some(state);
        self.editing = Some(key);
        cx.notify();
    }

    /// Commit or clear a server's custom name, then re-resolve display names.
    ///
    /// An empty name removes the custom entry so the default `Server N` ordinal returns. Only
    /// address servers persist; an unknown-endpoint edit simply closes.
    ///
    /// Args:
    ///     key: Server identity whose name is being committed.
    ///     text: New name from the input; trimmed, with empty meaning "reset to default".
    ///     cx: View context used to update the backend layout and rebuild caches.
    ///
    /// Returns:
    ///     Nothing; a persisted change marks the layout dirty for the shared save path.
    fn commit_rename(&mut self, key: ServerKey, text: String, cx: &mut Context<Self>) {
        if self.editing != Some(key) {
            return;
        }
        if let ServerKey::Address(address) = key {
            let ip = address.to_string();
            let text = text.trim().to_string();
            self.backend.update(cx, |b, _| {
                let changed = if text.is_empty() {
                    b.layout.core_server_names.remove(&ip).is_some()
                } else if b.layout.core_server_names.get(&ip) != Some(&text) {
                    b.layout.core_server_names.insert(ip, text);
                    true
                } else {
                    false
                };
                if changed {
                    b.layout_dirty = true;
                }
            });
        }
        self.editing = None;
        self.edit_input = None;
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Switch the Core Status presentation.
    ///
    /// The choice is remembered per host context, so this panel reopens in the same mode after a
    /// dock rebuild or a restart; a docked tab and a detached window keep their own selections.
    ///
    /// Args:
    ///     mode: Requested presentation.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; the changed mode is persisted while data caches and visibility state are retained.
    pub(super) fn set_mode(&mut self, mode: CoreStatusMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            // The selection belongs to the rows that were on screen when it was made. Problems,
            // Warnings and Updates draw no core rows at all, and By-IP and Flat draw different
            // ones, so carrying it across a mode change would leave a set nothing highlights --
            // in a panel where the next gesture enqueues a build onto live cores.
            self.core_selection.clear();
            // Inside the change gate on purpose: re-selecting the current mode writes nothing and
            // cannot arm a layout flush, matching the width and sort maps beside it.
            crate::persistence::table_persist::set_core_status_mode(
                &self.backend,
                &super::mode_ctx_id(self.detached),
                mode.code(),
                cx,
            );
            cx.notify();
        }
    }

    /// Every selectable core this panel is drawing RIGHT NOW, in on-screen order.
    ///
    /// The one place a scope comes from. Both presentations map their rows onto cores here, and
    /// both report the same shape -- `None` for a line that is not a selectable core (a Flat
    /// exchange heading, a By-IP server row), which the click algorithm ignores. A mode that draws
    /// no core rows at all reports NOTHING, which is what stops a selection made in By-IP from
    /// being acted on while the Problems list is on screen.
    ///
    /// A COLLAPSED server group contributes nothing either. That is deliberate and it is the
    /// safety property: a bulk update must never reach a core the user cannot see, so a selection
    /// resolved through this list shrinks visibly (the footer's own count says so) rather than
    /// silently keeping targets off screen.
    ///
    /// Args:
    ///     cx: Application context used to read the sort, the venues and the tree expansion.
    ///
    /// Returns:
    ///     One entry per rendered row in the current presentation.
    pub(super) fn visible_order(&self, cx: &App) -> Vec<Option<CoreId>> {
        match self.mode {
            CoreStatusMode::Flat => {
                let (rows, lines) = self.flat_view(cx);
                ordering::flat_order(&lines, &rows)
            }
            CoreStatusMode::ByIp => {
                let expanded = self.tree_state.read(cx).expanded_ids();
                server_view::visible_tree_order(&self.cached_groups, &expanded)
            }
            CoreStatusMode::Problems | CoreStatusMode::Warnings | CoreStatusMode::Updates => {
                Vec::new()
            }
        }
    }

    /// Apply one core-row click to the panel's controlled selection.
    ///
    /// A PLAIN click routes through `select_only`, not through `click`: in this panel a plain
    /// click means "this core", and it has to keep meaning that when the same row is clicked
    /// twice. Report's shared algorithm deliberately clears a sole selection on the second plain
    /// click, which is right for a report row and wrong here -- it would leave an ordinary
    /// double-click with nothing selected, in a panel whose next gesture enqueues a build. Ctrl
    /// and Shift go to `click` unchanged, so toggling and ranges are the one shared algorithm.
    ///
    /// Args:
    ///     clicked: The clicked core, or `None` for a line that is not a core row.
    ///     order: Rendered row order from [`Self::visible_order`].
    ///     modifiers: Native modifier snapshot from the owning window.
    ///     cx: View context used to repaint.
    pub(super) fn select_core_row(
        &mut self,
        clicked: Option<CoreId>,
        order: &[Option<CoreId>],
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        if modifiers.shift || modifiers.secondary() {
            self.core_selection
                .click(clicked, order, modifiers.shift, modifiers.secondary());
        } else {
            self.core_selection.select_only(clicked);
        }
        cx.notify();
    }

    /// Select every core the current presentation is drawing.
    ///
    /// Args:
    ///     order: Rendered row order from [`Self::visible_order`].
    ///     cx: View context used to repaint.
    pub(super) fn select_all_visible_cores(
        &mut self,
        order: &[Option<CoreId>],
        cx: &mut Context<Self>,
    ) {
        self.core_selection.select_all(order);
        cx.notify();
    }

    /// Drop the whole core-row selection.
    ///
    /// Args:
    ///     cx: View context used to repaint.
    pub(super) fn clear_core_selection(&mut self, cx: &mut Context<Self>) {
        self.core_selection.clear();
        cx.notify();
    }

    /// Panel-level keyboard route for the row selection.
    ///
    /// Ctrl/Cmd+A selects every core the current presentation draws, Escape drops the selection.
    /// The Flat table intercepts the same select-all chord itself when the GRID holds focus and
    /// calls the same handler, so the two routes converge; this one is what gives the By-IP tree,
    /// which is not a `MoonDataTable`, the identical gesture.
    ///
    /// Args:
    ///     event: The key press.
    ///     window: Host window, used to confirm this panel actually holds focus.
    ///     cx: View context used to repaint.
    pub(super) fn on_selection_key(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.contains_focused(window, cx) {
            return;
        }
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if key == "a" && modifiers.secondary() && !modifiers.shift && !modifiers.alt {
            let order = self.visible_order(cx);
            self.select_all_visible_cores(&order, cx);
        } else if key == "escape" && self.core_selection.len() > 0 {
            self.clear_core_selection(cx);
        }
    }
    /// Resolve the cores a bulk action should command, intersected with what is on screen.
    ///
    /// The SELECTION when the user has made one, the whole displayed scope otherwise -- and either
    /// way filtered through [`Self::visible_order`], so no caller can enqueue a core the panel is
    /// not currently drawing.
    ///
    /// Args:
    ///     visible: Cores the current presentation draws, in on-screen order.
    ///
    /// Returns:
    ///     Cores in on-screen order.
    pub(super) fn selected_or_visible(&self, visible: &[CoreId]) -> Vec<CoreId> {
        let selecting = self.core_selection.len() > 0;
        visible
            .iter()
            .copied()
            .filter(|core| !selecting || self.core_selection.contains(Some(*core)))
            .collect()
    }

    /// The selectable cores the current presentation is drawing, without the heading rows.
    ///
    /// Resolved ONCE per frame by the caller and threaded on, because `visible_order` re-sorts
    /// the flat rows and re-groups them into exchange sections on every call. The footer alone
    /// used to ask for it twice, on top of the one `render` already makes for the table, so a
    /// panel repainting at telemetry rate paid three full sorts of the whole fleet per frame.
    ///
    /// Args:
    ///     cx: Application context used to read the sort, the venues and the tree expansion.
    ///
    /// Returns:
    ///     Cores in on-screen order.
    pub(super) fn visible_cores(&self, cx: &App) -> Vec<CoreId> {
        self.visible_order(cx).into_iter().flatten().collect()
    }

    /// How many SELECTED cores the current presentation is actually drawing.
    ///
    /// Not `core_selection.len()`: collapsing a By-IP group hides its cores from the rendered
    /// order without rebuilding the cache, so the raw set can outnumber what is on screen. The
    /// footer labels its button with this, so the count it shows is the count its own confirm
    /// will name and its own press will enqueue.
    ///
    /// Args:
    ///     visible: Cores the presentation is drawing, from [`Self::visible_cores`].
    ///
    /// Returns:
    ///     Size of the visible part of the selection.
    pub(super) fn visible_selected(&self, visible: &[CoreId]) -> usize {
        if self.core_selection.len() == 0 {
            return 0;
        }
        visible
            .iter()
            .filter(|core| self.core_selection.contains(Some(**core)))
            .count()
    }
    /// What a footer bulk update would actually enqueue, and across how many server lanes.
    ///
    /// Built from the cores this panel SHOWS -- its selection when the operator has made one, its
    /// whole displayed scope otherwise ([`Self::selected_or_visible`]) -- and NOT from the store.
    /// It used to be fleet-wide: `update_fleet` walked `session.sessions()` and this preview walked
    /// the whole store, so a panel scoped to one group offered a button that quietly reached all 56
    /// cores. A control has to act on what it is drawn beside.
    ///
    /// Eligibility is read from public session data rather than the private
    /// `SessionManager::eligible` gate it mirrors; an exact match to that gate is not the point,
    /// wording the question honestly before the press is. `cores_behind` stays STORE-WIDE and is
    /// intersected here, so two differently scoped panels still agree about which cores are stale.
    ///
    /// Args:
    ///     only_behind: Whether to keep only the cores behind the fleet's newest build.
    ///     visible: Cores the presentation is drawing, resolved once per frame by the caller.
    ///     cx: Application context used to read the backend snapshot.
    ///
    /// Returns:
    ///     The cores to enqueue, in on-screen order, and how many distinct server lanes they span.
    pub(super) fn fleet_update_plan(
        &self,
        only_behind: bool,
        visible: &[CoreId],
        cx: &App,
    ) -> FleetUpdatePlan {
        let scope = self.selected_or_visible(visible);
        let b = self.backend.read(cx);
        let store = b.session.store();
        let behind: Option<HashSet<CoreId>> =
            only_behind.then(|| b.session.cores_behind().into_iter().collect());
        let mut lanes: HashSet<IpAddr> = HashSet::new();
        let mut cores: Vec<CoreId> = Vec::new();
        for id in scope {
            if behind.as_ref().is_some_and(|behind| !behind.contains(&id)) {
                continue;
            }
            let Some(data) = store.core(id) else {
                continue;
            };
            let Some(endpoint) = data.endpoint else {
                continue;
            };
            if !matches!(
                crate::controls::core_update::offer_state(
                    &data.status,
                    data.server_version,
                    true,
                    b.session.core_update_phase(id),
                ),
                crate::controls::core_update::OfferState::Offerable
            ) {
                continue;
            }
            cores.push(id);
            lanes.insert(endpoint.address);
        }
        FleetUpdatePlan {
            cores: Rc::from(cores),
            lanes: lanes.len(),
        }
    }
    /// Whether a core is connected right now.
    ///
    /// Asked again at the moment of sending, not only when the button was drawn: the command
    /// channel outlives the connection, so a command queued while a core is down waits there and
    /// fires on the next reconnect. For the clear that means destroying findings gathered during
    /// the very outage the operator was looking at.
    ///
    /// Args:
    ///     core: Core to check.
    ///     cx: App context.
    ///
    /// Returns:
    ///     `true` only for a core whose session reports `Ready`.
    fn core_is_ready(&self, core: CoreId, cx: &App) -> bool {
        matches!(
            self.backend
                .read(cx)
                .session
                .store()
                .core(core)
                .map(|data| data.status.clone()),
            Some(ConnStatus::Ready)
        )
    }

    /// A core's configured display name, or its id when the config no longer holds it.
    fn core_display_name(&self, core: CoreId, cx: &App) -> String {
        self.backend
            .read(cx)
            .config
            .servers
            .iter()
            .find(|server| server.id == core)
            .map(|server| server.name.clone())
            .unwrap_or_else(|| core.to_string())
    }

    /// Open the confirm a TEST diagnostic gets.
    ///
    /// Confirmed even though it looks harmless, and that is exactly why: the `test` fact it
    /// publishes stays on the core until something clears it, and the only thing that does is the
    /// irreversible, fleet-visible clear. An unconfirmed press can therefore force an operator to
    /// destroy a core's real findings just to tidy up after a test. The dialog says so.
    ///
    /// Args:
    ///     core: Core to test.
    ///     window: Window that owns the unique dialog.
    ///     cx: View context used to build the dialog.
    ///
    /// Returns:
    ///     Nothing; only Yes sends anything, and it closes the dialog either way.
    pub(super) fn confirm_problem_test(
        &mut self,
        core: CoreId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let question = t!(
            "core_status.problems_test_q",
            core = self.core_display_name(core, cx)
        )
        .to_string();
        let view = cx.entity().downgrade();
        window.open_unique_moon_dialog(
            "core-status-problem-test-confirm",
            cx,
            move |dialog, _window, cx| {
                let view = view.clone();
                problem_confirm_dialog(
                    dialog,
                    t!("core_status.problems_test_title").to_string(),
                    question.clone(),
                    "core-status-problem-test",
                    MoonButtonVariant::Blue,
                    cx,
                    move |window, cx| {
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |this, cx| this.send_problem_test(core, window, cx));
                        }
                    },
                )
            },
        );
    }

    /// Open the ONE confirm clearing a core's diagnostics gets.
    ///
    /// The core drops every confirmed finding AND every pending hypothesis, for every terminal
    /// watching it, with no way back, and it fixes nothing — a cause that persists produces a new
    /// fact later.
    ///
    /// Local rows are deliberately NOT cleared on confirmation: the core's next full list is the
    /// answer, and clearing optimistically would show a clean bill for a core that may have
    /// rejected the command.
    ///
    /// Args:
    ///     core: Core whose diagnostics would be dropped.
    ///     window: Window that owns the unique dialog.
    ///     cx: View context used to build the dialog.
    ///
    /// Returns:
    ///     Nothing; only Yes sends anything, and it closes the dialog either way.
    pub(super) fn confirm_clear_problems(
        &mut self,
        core: CoreId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let question = t!(
            "core_status.problems_clear_q",
            core = self.core_display_name(core, cx)
        )
        .to_string();
        let view = cx.entity().downgrade();
        window.open_unique_moon_dialog(
            "core-status-clear-problems-confirm",
            cx,
            move |dialog, _window, cx| {
                let view = view.clone();
                problem_confirm_dialog(
                    dialog,
                    t!("core_status.problems_clear_title").to_string(),
                    question.clone(),
                    "core-status-clear-problems",
                    MoonButtonVariant::Danger,
                    cx,
                    move |window, cx| {
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |this, cx| this.send_clear_problems(core, window, cx));
                        }
                    },
                )
            },
        );
    }

    /// Send the test, or say why it did not go.
    fn send_problem_test(&mut self, core: CoreId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.core_display_name(core, cx);
        if !self.core_is_ready(core, cx) {
            window.push_notification(
                MoonNotification::warning(t!("core_status.problems_not_sent_offline").to_string()),
                cx,
            );
            return;
        }
        match self
            .backend
            .read(cx)
            .session
            .test_core_problem(core, "MoonTerminal channel check")
        {
            Ok(()) => window.push_notification(
                MoonNotification::success(
                    t!("core_status.problems_test_sent", core = name).to_string(),
                ),
                cx,
            ),
            Err(error) => {
                log::warn!("core status: test problem for core {core} not sent: {error:#}");
                window.push_notification(
                    MoonNotification::warning(t!("core_status.problems_not_sent").to_string()),
                    cx,
                );
            }
        }
    }

    /// Send the clear, or say why it did not go.
    ///
    /// The readiness check is repeated HERE rather than trusted from the button that opened the
    /// dialog: that dialog can sit open while the core drops, and a queued clear would then fire on
    /// reconnect against findings the operator never saw.
    fn send_clear_problems(&mut self, core: CoreId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.core_display_name(core, cx);
        if !self.core_is_ready(core, cx) {
            window.push_notification(
                MoonNotification::warning(t!("core_status.problems_not_sent_offline").to_string()),
                cx,
            );
            return;
        }
        match self.backend.read(cx).session.clear_core_problems(core) {
            Ok(()) => window.push_notification(
                MoonNotification::success(
                    t!("core_status.problems_clear_sent", core = name).to_string(),
                ),
                cx,
            ),
            Err(error) => {
                log::warn!("core status: clear problems for core {core} not sent: {error:#}");
                window.push_notification(
                    MoonNotification::warning(t!("core_status.problems_not_sent").to_string()),
                    cx,
                );
            }
        }
    }

    /// Open the ONE confirm every footer bulk update gets, naming the core and lane counts
    /// before the press that fills the per-IP queue.
    ///
    /// ONE dialog for all three buttons, and for the named build the PROMPT LIVES INSIDE IT -- a
    /// prompt followed by a confirm would be two gates on one action, and the damage here is real
    /// exactly once. The row and server menus keep no confirm at all, as before: those are aimed
    /// at something the operator pointed at, while this reaches everything the panel shows.
    ///
    /// Args:
    ///     kind: Which footer button opened this.
    ///     window: Window that owns the unique dialog.
    ///     cx: View context used to resolve the plan and build the dialog.
    ///
    /// Returns:
    ///     Nothing; only Yes enqueues, and it closes the dialog either way.
    pub(super) fn confirm_fleet_update(
        &mut self,
        kind: FleetUpdateKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let behind = kind == FleetUpdateKind::Behind;
        let plan = self.fleet_update_plan(behind, &self.visible_cores(cx), cx);
        if plan.cores.is_empty() {
            return;
        }
        let names: Rc<[String]> = plan
            .cores
            .iter()
            .map(|core| self.core_display_name(*core, cx))
            .collect();
        let core_count = plan.cores.len();
        let lane_count = plan.lanes;
        let confirmed: Rc<[CoreId]> = plan.cores.clone();
        let backend = self.backend.clone();
        let view = cx.entity();
        let input = (kind == FleetUpdateKind::Named).then(|| {
            let input = cx.new(|cx| {
                MoonInputState::new(window, cx).placeholder(
                    t!(
                        "core_update.menu.named_ph",
                        cmd = moon_core::feed::CORE_UPDATE_COMMAND_WORD
                    )
                    .to_string(),
                )
            });
            input
                .clone()
                .update(cx, |input, cx| input.focus(window, cx));
            input
        });
        window.open_unique_moon_dialog(
            "core-status-fleet-update-confirm",
            cx,
            move |dialog, _window, cx| {
                let p = MoonPalette::active(cx);
                let confirm_backend = backend.clone();
                let confirm_view = view.clone();
                let confirm_cores = confirmed.clone();
                let confirm_input = input.clone();
                let field = input.clone();
                let names = names.clone();
                let question = t!(
                    "core_update.confirm.q",
                    cores = core_count,
                    servers = lane_count
                )
                .to_string();
                dialog
                    .w(px(360.0))
                    .close_button(true)
                    .overlay(true)
                    .overlay_closable(true)
                    .bg(rgb(p.shell_high))
                    .border_color(rgb(p.border))
                    .rounded(design::r_container(cx))
                    .text_color(rgb(p.text))
                    .header(
                        div()
                            .w_full()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("core_update.confirm.title").to_string()),
                    )
                    .content(move |content, _window, cx| {
                        let p = MoonPalette::active(cx);
                        content.child(
                            v_flex()
                                .w_full()
                                .gap_2()
                                .child(
                                    div()
                                        // MIXED NODE: `core_update.confirm.q` welds the core and
                                        // server COUNTS into the question. Half a node cannot be
                                        // styled.
                                        .font_family(design::mono())
                                        .text_size(design::t_body(cx))
                                        .text_color(rgb(p.text))
                                        .child(question.clone()),
                                )
                                // The same non-truncating list the row menu draws, from the same
                                // helper: two dialogs describing one scope must not be able to
                                // word it differently, and a core name is never shortened.
                                .children(update_menu::scope_name_list(&names, p, cx))
                                .children(field.clone().map(|field| {
                                    v_flex()
                                        .w_full()
                                        .gap_1()
                                        .child(div().text_color(rgb(p.text_muted)).child(
                                            t!("core_update.confirm.named_prompt").to_string(),
                                        ))
                                        .child(
                                            MoonInput::new("core-status-fleet-named-input")
                                                .state(&field)
                                                .small(),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(p.text_muted))
                                                .text_size(design::t_caption(cx))
                                                .child(
                                                    t!("core_update.menu.named_hint").to_string(),
                                                ),
                                        )
                                })),
                        )
                    })
                    .footer(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .justify_end()
                            .child(
                                MoonButton::new("core-status-fleet-confirm-no")
                                    .outline()
                                    .size(MoonButtonSize::Action)
                                    .label(format!("  {}  ", t!("dialogs.no")))
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                    })
                                    .render(),
                            )
                            .child(
                                MoonButton::new("core-status-fleet-confirm-yes")
                                    .size(MoonButtonSize::Action)
                                    .variant(MoonButtonVariant::Danger)
                                    .label(format!("  {}  ", t!("dialogs.yes")))
                                    .on_click(move |_, window, cx| {
                                        let target = match &confirm_input {
                                            Some(input) => {
                                                let typed = update_menu::typed_build_name(
                                                    &input.read(cx).value(),
                                                );
                                                let Some(typed) = typed else {
                                                    // An empty field means do nothing, exactly as
                                                    // it does in the row menu prompt.
                                                    window.close_dialog(cx);
                                                    return;
                                                };
                                                UpdateTarget::Named(typed)
                                            }
                                            None => UpdateTarget::Release,
                                        };
                                        // RE-RESOLVED at the press, then intersected with what
                                        // the operator confirmed. A dialog can sit open while a
                                        // preset change or a group switch rebuilds the panel, and
                                        // `update_scope` re-checks only enqueue ELIGIBILITY, never
                                        // scope -- so without this a still-ready core that left
                                        // the shown scope would take the build anyway. The
                                        // intersection can only ever SHRINK what was confirmed.
                                        let still: Vec<CoreId> = confirm_view
                                            .read(cx)
                                            .fleet_update_plan(
                                                behind,
                                                &confirm_view.read(cx).visible_cores(cx),
                                                cx,
                                            )
                                            .cores
                                            .iter()
                                            .copied()
                                            .filter(|core| confirm_cores.contains(core))
                                            .collect();
                                        window.close_dialog(cx);
                                        if still.len() < confirm_cores.len() {
                                            log::info!(
                                                "core status: footer update scope shrank {} -> {} while the confirm was open",
                                                confirm_cores.len(),
                                                still.len(),
                                            );
                                        }
                                        if still.is_empty() {
                                            return;
                                        }
                                        crate::controls::core_update::update_scope(
                                            &confirm_backend,
                                            &Rc::from(still),
                                            target,
                                            cx,
                                        );
                                    })
                                    .render(),
                            ),
                    )
            },
        );
    }
}

/// The chrome both diagnostic confirms share: title, question, No, and a coloured Yes.
///
/// One builder rather than two copies of forty lines, because the two dialogs differ in exactly
/// three things — their words, their button colour, and what Yes does — and a second copy is how
/// only one of them gets a fix.
///
/// Args:
///     dialog: The dialog under construction, from `open_unique_moon_dialog`.
///     title: Header text.
///     question: Body text, which must already name the core it addresses.
///     id_prefix: Element-id stem for the two footer buttons.
///     confirm: Variant for the Yes button — `Danger` for anything destructive.
///     cx: App context, for the palette and type scale.
///     on_yes: Runs before the dialog closes; closing is handled here so no caller can forget it.
///
/// Returns:
///     The built dialog.
#[allow(clippy::too_many_arguments)]
fn problem_confirm_dialog(
    dialog: moon_ui::MoonDialog,
    title: String,
    question: String,
    id_prefix: &'static str,
    confirm: MoonButtonVariant,
    cx: &App,
    on_yes: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> moon_ui::MoonDialog {
    let p = MoonPalette::active(cx);
    dialog
        .w(px(380.0))
        .close_button(true)
        .overlay(true)
        .overlay_closable(true)
        .bg(rgb(p.shell_high))
        .border_color(rgb(p.border))
        .rounded(design::r_container(cx))
        .text_color(rgb(p.text))
        .header(
            div()
                .w_full()
                .py_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .content(move |content, _window, cx| {
            let p = MoonPalette::active(cx);
            content.child(
                div()
                    // MIXED NODE: both questions that reach this dialog weld a CORE NAME into the
                    // sentence, and a core name is shown verbatim and identically everywhere.
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text))
                    .child(question.clone()),
            )
        })
        .footer(
            h_flex()
                .w_full()
                .gap_2()
                .justify_end()
                .child(
                    MoonButton::new(SharedString::from(format!("{id_prefix}-no")))
                        .outline()
                        .size(MoonButtonSize::Action)
                        .label(format!("  {}  ", t!("dialogs.no")))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                        })
                        .render(),
                )
                .child(
                    MoonButton::new(SharedString::from(format!("{id_prefix}-yes")))
                        .size(MoonButtonSize::Action)
                        .variant(confirm)
                        .label(format!("  {}  ", t!("dialogs.yes")))
                        .on_click(move |_, window, cx| {
                            on_yes(window, cx);
                            window.close_dialog(cx);
                        })
                        .render(),
                ),
        )
}
