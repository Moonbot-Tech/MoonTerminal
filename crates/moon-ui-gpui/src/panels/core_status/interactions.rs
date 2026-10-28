//! Core Status user-interaction handlers: chart-span and server selection, tree expansion, the core
//! multi-select filter, inline server rename, sort, and presentation mode. Split out of `mod.rs` as
//! the mutation half of the panel, distinct from its render-cache pipeline and its rendering.

use std::collections::HashSet;
use std::net::IpAddr;
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonInput, MoonInputEvent, MoonInputState, MoonNotification,
    MoonPalette, MoonWindowExt as _, h_flex, v_flex,
};

use super::health::HealthFilter;
use super::model::{CoreStatusRow, ServerKey};
use super::{ChartWindow, CoreStatusMode, CoreStatusView, ordering};
use crate::controls::core_update;
use crate::design;
use crate::display_text::plural_key;
use moon_core::feed::UpdateTarget;
use moon_core::session::CoreId;
use rust_i18n::t;

/// Which of the three bulk buttons opened the confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FleetUpdateKind {
    /// Every offerable core in the panel's scope, or in its selection.
    All,
    /// Only those behind the fleet's newest build.
    Behind,
    /// Every offerable core, to a build name the operator types inside the confirm.
    Named,
}

/// The footer confirm's question: what is updated, on how many servers, and to what.
///
/// Composed from two counted phrases rather than one string with two numbers, because a noun
/// agrees with its own count -- one string could only ever be right for one of them, which is how
/// a single core read "1 ядер на 1 серверах".
///
/// Args:
///     named: Whether the target is the build typed below the question rather than the release.
///     cores: How many cores the press enqueues.
///     servers: How many server lanes they span.
///
/// Returns:
///     The localized question.
fn fleet_question(named: bool, cores: usize, servers: usize) -> String {
    let cores = t!(
        plural_key("core_update.confirm.cores", cores as u64),
        n = cores
    )
    .to_string();
    let servers = t!(
        plural_key("core_update.confirm.servers", servers as u64),
        n = servers
    )
    .to_string();
    let key = if named {
        "core_update.confirm.q_named"
    } else {
        "core_update.confirm.q_release"
    };
    t!(key, cores = cores, servers = servers).to_string()
}

/// Exactly what a bulk update would enqueue, resolved before the confirm opens.
pub(super) struct FleetUpdatePlan {
    /// Cores to enqueue, in on-screen order.
    pub(super) cores: Rc<[CoreId]>,
    /// Distinct server lanes they span -- the queue runs one core at a time per lane, and the
    /// confirm says so.
    pub(super) lanes: usize,
}

/// Cores a bulk update may command when the mode is Updates: every core in the panel scope.
///
/// Other modes return `None` and the caller keeps the on-screen list. Selection and Ctrl+A stay
/// on [`CoreStatusView::visible_order`], which is empty in Updates, so this scope never becomes
/// a selection.
///
/// Args:
///     mode: The panel's current presentation.
///     rows: Unfiltered cores in the panel scope.
///
/// Returns:
///     Every scoped core id in row order, or `None` when the mode is not Updates.
pub(super) fn updates_scope_ids(
    mode: CoreStatusMode,
    rows: &[CoreStatusRow],
) -> Option<Vec<CoreId>> {
    if mode == CoreStatusMode::Updates {
        Some(rows.iter().map(|row| row.id).collect())
    } else {
        None
    }
}

/// How many scoped cores reported a build behind the fleet's newest.
///
/// The same predicate as the version health chip: `version_behind` is set only for a reported
/// build that is behind.
///
/// Args:
///     rows: Scoped core snapshots.
///
/// Returns:
///     The count.
pub(super) fn behind_in_scope(rows: &[CoreStatusRow]) -> usize {
    rows.iter()
        .filter(|row| row.version_behind.is_some())
        .count()
}

/// Server lane a bulk update would run one core on, or `None` when the core is not offerable.
///
/// The one eligibility rule [`CoreStatusView::fleet_update_plan`] and the Updates footer counts
/// share: a stored core with a known endpoint whose update offer state is `Offerable`.
///
/// Args:
///     b: Backend snapshot.
///     id: Core to test.
///
/// Returns:
///     The core's endpoint address when it may be enqueued.
pub(super) fn update_lane(b: &crate::Backend, id: CoreId) -> Option<IpAddr> {
    let data = b.session.store().core(id)?;
    let endpoint = data.endpoint?;
    matches!(
        crate::controls::core_update::offer_state(
            &data.status,
            data.server_version,
            true,
            b.session.core_update_phase(id),
        ),
        crate::controls::core_update::OfferState::Offerable
    )
    .then_some(endpoint.address)
}

impl CoreStatusView {
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

    /// Apply a Problems header sort from a column click and remember it the way Flat does.
    ///
    /// Args:
    ///     key: Column key from the table header.
    ///     ascending: Whether the click requested ascending order.
    ///     cx: View context used to persist the choice and repaint.
    ///
    /// Returns:
    ///     Nothing. An unknown key clears the saved sort back to newest-confirmation-first.
    ///     A repeat of the current choice does not arm a layout flush.
    pub(super) fn set_problems_sort(&mut self, key: &str, ascending: bool, cx: &mut Context<Self>) {
        let next =
            super::problems::restore_problems_sort(Some(moon_core::config::TableSortPreference {
                column: key.to_string(),
                ascending,
            }));
        if self.problems_sort != next {
            self.problems_sort = next;
            let preference = self.problems_sort.as_ref().map(|(column, ascending)| {
                moon_core::config::TableSortPreference {
                    column: column.clone(),
                    ascending: *ascending,
                }
            });
            let id =
                crate::persistence::table_persist::ctx_id("core-status-problems", self.detached);
            crate::persistence::table_persist::set_sort(&self.backend, &id, preference, cx);
            cx.notify();
        }
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
    ///     Nothing. A real mode change is persisted. A health chip is cleared even when the mode
    ///     does not change, and that clear does not write the mode.
    pub(super) fn set_mode(&mut self, mode: CoreStatusMode, cx: &mut Context<Self>) {
        let mode_changed = self.mode != mode;
        if !mode_changed && self.health_filter.is_none() {
            return;
        }
        if mode_changed {
            self.mode = mode;
            // The selection belongs to the rows that were on screen when it was made. Problems,
            // Warnings and Updates draw no core rows at all, and Servers and Flat draw different
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
        }
        self.health_filter = None;
        cx.notify();
    }

    /// Apply one health-chip click.
    ///
    /// The same chip a second time clears the filter. A click from Warnings or Updates moves
    /// the panel to Flat first, which clears any chip, and the chosen chip is then restored.
    /// Flat and Problems keep their mode.
    ///
    /// Args:
    ///     filter: The chip the operator clicked.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing. The next Flat projection reads the chip.
    pub(super) fn toggle_health_filter(&mut self, filter: HealthFilter, cx: &mut Context<Self>) {
        let next = health_filter_click(self.mode, self.health_filter, filter);
        if next.mode != self.mode {
            self.set_mode(next.mode, cx);
        }
        if self.health_filter != next.filter {
            self.health_filter = next.filter;
            cx.notify();
        }
    }

    /// Open the alarms list and drop any health chip.
    ///
    /// Args:
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing. The mode is Warnings and the chip is clear.
    pub(super) fn open_alarms(&mut self, cx: &mut Context<Self>) {
        let (mode, filter) = open_alarms_state();
        if self.mode != mode {
            self.set_mode(mode, cx);
        }
        if self.health_filter != filter {
            self.health_filter = filter;
            cx.notify();
        }
    }

    /// Store the toolbar search in the form the projection compares.
    ///
    /// Args:
    ///     text: Raw field text, including a still-uncommitted edit.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing. An unchanged comparison form does not repaint.
    pub(super) fn set_search(&mut self, text: String, cx: &mut Context<Self>) {
        let query = super::cache::normalize_query(&text);
        if self.search_query == query {
            return;
        }
        self.search_query = query;
        cx.notify();
    }

    /// Check or uncheck one Flat column and store the columns that stay visible.
    ///
    /// The stored list is the canonical order, never empty. No stored entry means every column
    /// is user-visible, so the first uncheck starts from that full list. The last checked
    /// column cannot be removed.
    ///
    /// Args:
    ///     key: Flat column id from [`super::table::order_keys`].
    ///     cx: View context used to persist the layout and repaint.
    ///
    /// Returns:
    ///     Nothing. An unknown key, or the last visible column, is left as it was.
    pub(super) fn toggle_flat_column(&mut self, key: &str, cx: &mut Context<Self>) {
        let order = super::table::order_keys();
        if !order.contains(&key) {
            return;
        }
        let widths_id = self.widths_id.clone();
        let stored = crate::persistence::table_persist::visible(self.backend.read(cx), &widths_id);
        let mut set: HashSet<String> = match stored.as_ref() {
            None => order.iter().copied().map(str::to_string).collect(),
            Some(keys) => {
                let have: HashSet<&str> = keys.iter().map(String::as_str).collect();
                order
                    .iter()
                    .copied()
                    .filter(|column| have.contains(*column))
                    .map(str::to_string)
                    .collect()
            }
        };
        if set.contains(key) {
            if set.len() == 1 {
                return;
            }
            set.remove(key);
        } else {
            set.insert(key.to_string());
        }
        let keys: Vec<String> = order
            .iter()
            .copied()
            .filter(|column| set.contains(*column))
            .map(str::to_string)
            .collect();
        crate::persistence::table_persist::set_visible(&self.backend, &widths_id, keys, cx);
        cx.notify();
    }

    /// Every selectable core this panel is drawing RIGHT NOW, in on-screen order.
    ///
    /// Flat and Servers map their rows onto cores. `None` is a line that is not a selectable core
    /// (a Flat exchange heading), which the click algorithm ignores. Problems and Warnings draw no
    /// core rows, so they report nothing: a selection made on Servers is not acted on while those
    /// lists are showing.
    ///
    /// Updates also reports nothing here, so Ctrl+A does not select the fleet. The update buttons
    /// do not read this list. They read [`Self::visible_cores`], which in Updates mode is every
    /// core in the panel scope.
    ///
    /// Args:
    ///     cx: Application context used to read the sort and the venues.
    ///
    /// Returns:
    ///     One entry per rendered row in the current presentation.
    pub(super) fn visible_order(&self, cx: &App) -> Vec<Option<CoreId>> {
        match self.mode {
            CoreStatusMode::Flat => {
                let hidden = self.user_hidden_columns(cx);
                let (rows, lines, _) = self.flat_view(&hidden, cx);
                ordering::flat_order(&lines, &rows)
            }
            CoreStatusMode::Servers => {
                super::servers_view::card_core_order(&super::servers_view::shown_server_groups(
                    &self.cached_groups,
                    &self.search_query,
                    self.ip_masked,
                ))
            }
            CoreStatusMode::Problems | CoreStatusMode::Warnings | CoreStatusMode::Updates => {
                Vec::new()
            }
        }
    }

    /// Apply one core-row click to the panel's controlled selection.
    ///
    /// Through `RowSelection::press`: a plain click means "this core" and keeps meaning it on a
    /// second click, Ctrl and Shift toggle and range — the reading this panel and the expert
    /// settings window share, and the one written up on that method.
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
        self.core_selection
            .press(clicked, order, modifiers.shift, modifiers.secondary());
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
            // Deliberately NOT extended to the Problems row selection. Escape already carries a
            // window-level meaning here (it closes an open chart), and adding a second one to the
            // same key risks swallowing that. The Problems selection is cleared by clicking its
            // row again, which is the gesture that set it.
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
        if let Some(scope) = updates_scope_ids(self.mode, &self.cached_rows) {
            return scope;
        }
        self.visible_order(cx).into_iter().flatten().collect()
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
        let behind: Option<HashSet<CoreId>> =
            only_behind.then(|| b.session.cores_behind().into_iter().collect());
        let mut lanes: HashSet<IpAddr> = HashSet::new();
        let mut cores: Vec<CoreId> = Vec::new();
        for id in scope {
            if behind.as_ref().is_some_and(|behind| !behind.contains(&id)) {
                continue;
            }
            let Some(lane) = update_lane(b, id) else {
                continue;
            };
            cores.push(id);
            lanes.insert(lane);
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
        // The app's own spelling of "reachable", shared with every other fleet action rather than
        // hand-written a third time in this panel.
        self.backend.read(cx).session.core_run_state(core).online
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
                    // The test names its one core inside the question, like the one-core reset.
                    Rc::from([]),
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

    /// Open the ONE confirm a diagnostics reset gets, for one core or for a whole scope.
    ///
    /// Every core listed drops every confirmed finding AND every pending hypothesis, for every
    /// terminal watching it, with no way back, and it fixes nothing — a cause that persists produces
    /// a new fact later.
    ///
    /// The question names the BLAST RADIUS, which is the only thing that changed when this action
    /// stopped needing a hand-picked core: one core is named, and a scope states how many cores and
    /// which. An operator who never opened the panel's selector is the normal case, and their press
    /// must not be quieter than a deliberate one.
    ///
    /// Local rows are deliberately NOT cleared on confirmation: each core's next full list is the
    /// answer, and clearing optimistically would show a clean bill for a core that may have rejected
    /// the command.
    ///
    /// Args:
    ///     window: Window that owns the unique dialog.
    ///     cx: View context used to build the dialog.
    ///
    /// Returns:
    ///     Nothing; only Yes sends anything, and it closes the dialog either way.
    pub(super) fn confirm_clear_problems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cores = self.connected_in_scope(cx);
        let names: Rc<[String]> = cores
            .iter()
            .map(|core| self.core_display_name(*core, cx))
            .collect();
        let (title, question) = match cores.as_slice() {
            // Both fleet buttons are greyed with no targets, so this is the race where the last
            // core dropped between the repaint and the press. It says so rather than swallowing
            // the press, which is what the re-read does in the identical state.
            [] => return self.no_fleet_targets(window, cx),
            [core] => (
                t!("core_status.problems_clear_title").to_string(),
                t!(
                    "core_status.problems_clear_q",
                    core = self.core_display_name(*core, cx)
                )
                .to_string(),
            ),
            many => (
                t!("core_status.problems_clear_title_many").to_string(),
                t!("core_status.problems_clear_q_many", cores = many.len()).to_string(),
            ),
        };
        let cores: Rc<[CoreId]> = cores.into();
        let view = cx.entity().downgrade();
        window.open_unique_moon_dialog(
            "core-status-clear-problems-confirm",
            cx,
            move |dialog, _window, cx| {
                let view = view.clone();
                // Refcount bumps, not copies: this builder is an `Fn` re-run on every frame the
                // dialog is drawn, and the confirmed scope can be the whole fleet.
                let cores = cores.clone();
                problem_confirm_dialog(
                    dialog,
                    title.clone(),
                    question.clone(),
                    names.clone(),
                    "core-status-clear-problems",
                    MoonButtonVariant::Danger,
                    cx,
                    move |window, cx| {
                        if let Some(view) = view.upgrade() {
                            let cores = cores.clone();
                            view.update(cx, |this, cx| {
                                this.send_clear_problems(&cores, window, cx)
                            });
                        }
                    },
                )
            },
        );
    }

    /// Re-read the retained diagnostics of every connected core in scope.
    ///
    /// Unconfirmed on purpose, and it is the only one of the three that can be: nothing leaves this
    /// terminal and nothing on any core changes. See `CoreCmd::RefreshProblems` — the protocol has
    /// no request for a diagnostics list, so this republishes what the library already holds.
    ///
    /// Args:
    ///     window: Window that shows the outcome.
    ///     cx: View context.
    ///
    /// Returns:
    ///     Nothing; the outcome is a notification.
    pub(super) fn refresh_problems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cores = self.connected_in_scope(cx);
        self.fleet_send(
            &cores,
            window,
            cx,
            |session, live| session.refresh_core_problems_many(live),
            |sent| {
                MoonNotification::success(
                    t!("core_status.problems_refreshed", cores = sent).to_string(),
                )
            },
        );
    }

    /// Send the reset to every core that is still up, or say why it did not go.
    fn send_clear_problems(
        &mut self,
        cores: &[CoreId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The one-core success still NAMES its core: that message is what an operator reads back
        // when checking which core they just emptied, and a count cannot answer it.
        let single = match cores {
            [core] => Some(self.core_display_name(*core, cx)),
            _ => None,
        };
        self.fleet_send(
            cores,
            window,
            cx,
            |session, live| session.clear_core_problems_many(live),
            move |sent| match single {
                Some(name) => MoonNotification::success(
                    t!("core_status.problems_clear_sent", core = name).to_string(),
                ),
                None => MoonNotification::success(
                    t!("core_status.problems_clear_sent_many", cores = sent).to_string(),
                ),
            },
        );
    }

    /// Run one fleet diagnostics command and report what it actually reached.
    ///
    /// Both actions are the same four steps — drop the cores that went down, send one command per
    /// core, count the acceptances, tell the operator — so they share them rather than each
    /// spelling them out and drifting.
    ///
    /// Readiness is re-read HERE rather than trusted from the button or the confirm that started
    /// the action: either can sit open while a core drops, and a command queued for a core that is
    /// down waits on its channel and fires on reconnect.
    ///
    /// The shortfall is the point. A core that went down between the press and the send, and one
    /// whose command channel is already gone, mean the same thing to the operator: the action did
    /// NOT reach everything it named. Which of the two it was reaches the log. A bare success toast
    /// over a partial result is precisely the "failure nobody sees" this surface's rule forbids.
    ///
    /// Args:
    ///     cores: Cores the press addressed.
    ///     window: Window that shows the outcome.
    ///     cx: View context.
    ///     send: Issues the command to the still-connected cores, returning those that accepted.
    ///     whole: Builds this action's own success message when every named core accepted.
    ///
    /// Returns:
    ///     Nothing; the outcome is a notification.
    fn fleet_send(
        &mut self,
        cores: &[CoreId],
        window: &mut Window,
        cx: &mut Context<Self>,
        send: impl FnOnce(&moon_core::session::SessionManager, &[CoreId]) -> Vec<CoreId>,
        whole: impl FnOnce(usize) -> MoonNotification,
    ) {
        let asked = cores.len();
        if asked == 0 {
            return self.no_fleet_targets(window, cx);
        }
        let live = self.still_connected(cores, cx);
        let sent = send(&self.backend.read(cx).session, &live).len();
        // The two failure messages are shared and the success is the caller's: "it did not reach
        // everything" means the same for either action, while "the reset went" and "the list was
        // re-read" are different facts an operator acts on differently.
        let note = if sent == 0 {
            log::warn!("core status: a fleet diagnostics action reached none of {asked} cores");
            MoonNotification::warning(
                t!("core_status.problems_fleet_none", cores = asked).to_string(),
            )
        } else if sent < asked {
            log::warn!("core status: a fleet diagnostics action reached {sent} of {asked} cores");
            MoonNotification::warning(
                t!(
                    "core_status.problems_fleet_partial",
                    sent = sent,
                    cores = asked
                )
                .to_string(),
            )
        } else {
            whole(sent)
        };
        window.push_notification(note, cx);
    }

    /// Say that a fleet action found nothing to act on, in the one wording both of them use.
    ///
    /// The greyed button already carries this reason as its tooltip; this is the same sentence for
    /// the race where the last core drops between the repaint that enabled the button and the press.
    fn no_fleet_targets(&self, window: &mut Window, cx: &mut Context<Self>) {
        // Which emptiness it was: a scope full of live cores must not be reported as offline just
        // because the ONE core the operator clicked is down. Same split as `fleet_refusal`'s, and
        // for the same reason — the two have different remedies.
        let key = match self.problems_picked {
            Some(_) => "core_status.problems_picked_offline",
            None => "core_status.problems_no_online",
        };
        window.push_notification(MoonNotification::warning(t!(key).to_string()), cx);
    }

    /// Narrow the three Problems actions to one core, or widen them back to the whole scope.
    ///
    /// A TOGGLE because the narrowing has to be reversible: a click that can reach "just this core"
    /// and never get back would strand the operator on one core, and the table's own handling only
    /// ever sets a row. Clicking any row of the already-picked core clears the pick — including a
    /// different finding of the same core, which is the same answer to the same question.
    ///
    /// Stored as the CORE, never as the clicked row index. The finding list is rebuilt from live
    /// core data on every repaint, so an index outlives the row it named: one finding appearing or
    /// clearing above it re-points it at another core, and the action it narrows cannot be undone.
    ///
    /// Args:
    ///     core: Core owning the clicked finding.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; the next frame draws that core's rows selected.
    pub(super) fn pick_problem_core(&mut self, core: CoreId, cx: &mut Context<Self>) {
        self.problems_picked = (self.problems_picked != Some(core)).then_some(core);
        // The table's own row and cell cursors are dropped on EVERY pick change, set or cleared, so
        // the surface carries exactly one highlight. Left behind they draw a second one from a raw
        // row INDEX (`data_table.rs:1231` ORs it into the row, and the cell background at :1262 is
        // painted whether or not cells are selectable) — and an index is precisely what this pick
        // exists to avoid, because the finding list is rebuilt every repaint and the index then
        // marks a different core's row. `select_row(None)` alone is not enough: it leaves
        // `selected_cell` behind (`data_table.rs:345`), which paints a ghost on the last clicked
        // cell after the pick is gone.
        self.problems_table_state.update(cx, |state, cx| {
            state.selected_row = None;
            state.selected_column = None;
            state.selected_cell = None;
            cx.notify();
        });
        cx.notify();
    }

    /// What a Problems-mode fleet action acts on: the clicked finding's core, else the whole scope.
    ///
    /// The panel's own rule for a bulk command — the selection when the operator has made one, the
    /// displayed scope otherwise (`selected_or_visible`) — applied to the one gesture this mode
    /// offers. Problems draws findings rather than cores, so `core_selection` can never be set
    /// here; the table's own selected row is its equivalent, and the render arm resolves it to a
    /// core in [`CoreStatusView::problems_picked`].
    ///
    /// The list is derived at PRESS time rather than carried on the rendered scope: the render arm
    /// would otherwise build it on every repaint for something only a click reads, and the click
    /// has to re-check reachability at send time regardless.
    fn connected_in_scope(&self, cx: &App) -> Vec<CoreId> {
        let scope = self.effective_scope(self.backend.read(cx)).ids().to_vec();
        // The pick is intersected with the scope rather than trusted: the panel's selector or its
        // workspace can drop a core out from under a pick that was made before either moved.
        let ids = match self.problems_picked {
            Some(core) if scope.contains(&core) => vec![core],
            Some(_) => Vec::new(),
            None => scope,
        };
        self.still_connected(&ids, cx)
    }

    /// Keep only the cores that are connected right now.
    fn still_connected(&self, cores: &[CoreId], cx: &App) -> Vec<CoreId> {
        let session = &self.backend.read(cx).session;
        cores
            .iter()
            .copied()
            .filter(|core| session.core_run_state(*core).online)
            .collect()
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
        let named = kind == FleetUpdateKind::Named;
        // A release press replaces whatever lettered build a core runs; say so before Yes.
        let replaces_test = if named {
            None
        } else {
            let b = self.backend.read(cx);
            let letters: Vec<String> = plan
                .cores
                .iter()
                .filter_map(|core| b.session.store().core(*core)?.server_version_suffix.clone())
                .collect();
            core_update::replaces_test_note(letters.iter().map(String::as_str))
        };
        let input = named.then(|| core_update::build_name_input(None, window, cx));
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
                let replaces_test = replaces_test.clone();
                let question = fleet_question(named, core_count, lane_count);
                dialog
                    .w(design::ui_px(cx, 360.0))
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
                                        // MIXED NODE: `fleet_question` welds the core and server
                                        // COUNTS into the question. Half a node cannot be styled.
                                        .font_family(design::mono())
                                        .text_size(design::t_body(cx))
                                        .text_color(rgb(p.text))
                                        .child(question.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(design::t_caption(cx))
                                        .text_color(rgb(p.text_muted))
                                        .child(t!("core_update.confirm.lanes").to_string()),
                                )
                                // What a release press installs: the page has no release number
                                // to show, so it at least says which package it asks the core for.
                                .when(!named, |col| {
                                    col.child(
                                        div()
                                            .text_size(design::t_caption(cx))
                                            .text_color(rgb(p.text_muted))
                                            .child(
                                                t!("core_update.confirm.release_what").to_string(),
                                            ),
                                    )
                                })
                                .children(replaces_test.clone().map(|note| {
                                    div()
                                        .text_size(design::t_caption(cx))
                                        .text_color(rgb(p.amber))
                                        .child(note)
                                }))
                                // The same non-truncating list the row menu draws, from the same
                                // helper: two dialogs describing one scope must not be able to
                                // word it differently, and a core name is never shortened.
                                .children(core_update::scope_name_list(&names, p, cx))
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
                                                .size(design::INPUT_SIZE),
                                        )
                                        .child(core_update::build_name_hint(p, cx))
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
                                    .label(format!("  {}  ", t!("dialogs.no")))
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                    })
                                    .render(),
                            )
                            .child(
                                MoonButton::new("core-status-fleet-confirm-yes")
                                    .variant(MoonButtonVariant::Danger)
                                    .label(format!("  {}  ", t!("dialogs.yes")))
                                    .on_click(move |_, window, cx| {
                                        let target = match &confirm_input {
                                            Some(input) => {
                                                let typed = core_update::typed_build_name(
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
                                        core_update::update_scope(
                                            &confirm_backend,
                                            &still,
                                            target,
                                            window,
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
    names: Rc<[String]>,
    id_prefix: &'static str,
    confirm: MoonButtonVariant,
    cx: &App,
    on_yes: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> moon_ui::MoonDialog {
    let p = MoonPalette::active(cx);
    dialog
        .w(design::ui_px(cx, 380.0))
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
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            // MIXED NODE: the one-core questions weld a CORE NAME into the
                            // sentence, and a core name is shown verbatim and identically
                            // everywhere.
                            .font_family(design::mono())
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text))
                            .child(question.clone()),
                    )
                    // The same non-truncating list the footer's fleet confirm and the row menu
                    // draw, from the same helper: dialogs describing one scope must not be able to
                    // word it differently, and a core name is never shortened. It renders nothing
                    // for a single core, which the question above has already named.
                    .children(core_update::scope_name_list(&names, p, cx)),
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
                        .label(format!("  {}  ", t!("dialogs.no")))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                        })
                        .render(),
                )
                .child(
                    MoonButton::new(SharedString::from(format!("{id_prefix}-yes")))
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

/// The mode and chip a health-chip click leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HealthChipClick {
    /// Presentation after the click. Only Flat stays; every other mode moves to Flat.
    pub(super) mode: CoreStatusMode,
    /// Chip after the click. The same chip a second time is `None`.
    pub(super) filter: Option<HealthFilter>,
}

/// Resolve one health-chip click without a view.
///
/// Args:
///     mode: Presentation that is showing.
///     current: Chip that is already on, if any.
///     clicked: Chip the operator clicked.
///
/// Returns:
///     The mode and chip to store. Flat keeps `mode`. Any other mode, including Problems,
///     becomes Flat so the chip actually filters the table.
pub(super) fn health_filter_click(
    mode: CoreStatusMode,
    current: Option<HealthFilter>,
    clicked: HealthFilter,
) -> HealthChipClick {
    let filter = if current == Some(clicked) {
        None
    } else {
        Some(clicked)
    };
    let mode = if matches!(mode, CoreStatusMode::Flat) {
        mode
    } else {
        CoreStatusMode::Flat
    };
    HealthChipClick { mode, filter }
}

/// Mode and chip the alarms control selects.
///
/// Returns:
///     Warnings, and no health chip.
pub(super) fn open_alarms_state() -> (CoreStatusMode, Option<HealthFilter>) {
    (CoreStatusMode::Warnings, None)
}

#[cfg(test)]
mod tests;
