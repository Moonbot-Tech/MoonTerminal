//! Core Status panel: connection state and typed protocol-v4 resource telemetry
//! (`Event::KernelHealth`) for every core in scope.
//!
//! `CoreData::sys` holds the latest sample, while `sys_rev` invalidates the panel
//! when metric values or the decoded endpoint change.
//!
//! Like the Assets panel, it is scoped to a window group and can live in a dock
//! tab or a detached window. [`crate::persistence::table_persist`] stores separate column
//! widths for `:dock` and `:win`. This module owns data and lifecycle; [`server_view`]
//! and [`table`] own the two presentations.

mod model;
mod presentation;
mod server_view;
mod table;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonDataTableState, MoonPalette, MoonSegmentItem, MoonSegmentedControl,
    MoonTreeState, Panel, PanelEvent, PanelState, h_flex, v_flex,
};

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use crate::panels::RenderGate;
use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreSysStatus};
use rust_i18n::t;

use model::{CoreStatusRow, ServerKey, ServerStatusGroup, aggregate_servers};

/// Available Core Status presentations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreStatusMode {
    /// Expandable server rows grouped by endpoint address.
    ByIp,
    /// Existing one-row-per-core telemetry table.
    Flat,
}

impl Default for CoreStatusMode {
    /// Return the server-by-IP presentation used on every new panel instance.
    fn default() -> Self {
        Self::ByIp
    }
}

/// Group-scoped Core Status panel for a dock tab or detached window.
pub struct CoreStatusView {
    pub(super) backend: Entity<Backend>,
    /// Window group whose cores define this panel's scope, matching the Assets panel.
    group: String,
    /// Multi-select core filter; an empty set means every core in the group.
    pub(super) sel_cores: HashSet<CoreId>,
    /// Redraw gate driven by the system/status signature or a 1 Hz sample-age refresh.
    gate: RenderGate,
    cache_sig: Option<u64>,
    cached_rows: Rc<Vec<CoreStatusRow>>,
    cached_groups: Rc<Vec<ServerStatusGroup>>,
    /// Last endpoint-derived key for every group-scoped core, including filtered-out cores.
    server_keys: HashMap<CoreId, ServerKey>,
    hidden_servers: HashSet<ServerKey>,
    mode: CoreStatusMode,
    tree_state: Entity<MoonTreeState>,
    tree_initialized: bool,
    table_state: Entity<MoonDataTableState>,
    /// Context-qualified column-width persistence ID (`core-status-table:dock` or `:win`).
    widths_id: String,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl CoreStatusView {
    /// Construct a group-scoped Core Status panel and its table/tree state.
    ///
    /// Args:
    ///     backend: Shared terminal backend.
    ///     group: Window group that defines the core scope.
    ///     detached: Whether column widths use the detached-window persistence key.
    ///     _window: Host window reserved for panel construction symmetry.
    ///     cx: View context used for observers and child entities.
    ///
    /// Returns:
    ///     A panel whose default presentation is server-by-IP.
    fn new(
        backend: Entity<Backend>,
        group: String,
        detached: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Backend drains repaint on metric/status changes or once per second so
        // the sample-age column advances even when no new telemetry arrives.
        cx.observe(&backend, |this, backend, cx| {
            let now = moon_chart::paint::now_unix_ms();
            let sig = {
                let b = backend.read(cx);
                this.sys_sig(b)
            };
            let changed = this.cache_sig != Some(sig);
            let due = this.gate.should_notify(sig, now);
            if changed {
                this.rebuild_cache_with_signature(sig, cx);
            }
            if changed || (due && this.mode == CoreStatusMode::Flat) {
                cx.notify();
            }
        })
        .detach();

        let widths_id = crate::persistence::table_persist::ctx_id("core-status-table", detached);
        let saved_widths = crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();
        let tree_state = cx.new(|cx| MoonTreeState::new(cx));

        let mut this = Self {
            backend,
            group,
            sel_cores: HashSet::new(),
            gate: RenderGate::default(),
            cache_sig: None,
            cached_rows: Rc::new(Vec::new()),
            cached_groups: Rc::new(Vec::new()),
            server_keys: HashMap::new(),
            hidden_servers: HashSet::new(),
            mode: CoreStatusMode::default(),
            tree_state,
            tree_initialized: false,
            table_state,
            widths_id,
            dock: None,
            focus: cx.focus_handle(),
        };
        this.rebuild_cache(cx);
        this
    }

    /// Reconstruct a dock tab from `docks.json` using the `:dock` width context.
    pub fn restored_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, group, false, window, cx)
    }

    /// Build detached-window content, framed by `DetachedWindow`, using the `:win` width context.
    pub fn detached_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, group, true, window, cx)
    }

    /// Return the table state used by the detached window's auto-width reset button.
    pub fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Return this panel group's cores in canonical order.
    pub(super) fn scope_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }

    /// Fold CoreId, system revision, status, and exchange into the ordered-cache signature.
    ///
    /// Include CoreId so canonical reordering invalidates the cache when state is unchanged.
    ///
    /// Args:
    ///     b: Backend snapshot whose scoped cores contribute to the signature.
    ///
    /// Returns:
    ///     Deterministic ordered signature for all visible row inputs.
    fn sys_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        let exchange_names = b.session.market_source().core_exchange_names();
        self.scope_cores(b).iter().fold(0u64, |a, (id, name)| {
            let (sys_rev, st) = store
                .core(*id)
                .map(|c| (c.sys_rev, status_ord(&c.status)))
                .unwrap_or((0, 0));
            let exchange = exchange_names
                .get(id)
                .map(|name| text_sig(name))
                .unwrap_or(0);
            a.wrapping_mul(31)
                .wrapping_add(*id)
                .wrapping_mul(31)
                .wrapping_add(sys_rev)
                .wrapping_mul(31)
                .wrapping_add(st)
                .wrapping_mul(31)
                .wrapping_add(exchange)
                .wrapping_mul(31)
                .wrapping_add(text_sig(name))
        })
    }

    /// Collect filtered rows plus endpoint keys for every core in the group scope.
    ///
    /// Args:
    ///     b: Backend snapshot containing config, sessions, and market identity.
    ///
    /// Returns:
    ///     Canonically ordered visible rows and unfiltered server identities for state migration.
    fn collect(&self, b: &Backend) -> (Vec<CoreStatusRow>, HashMap<CoreId, ServerKey>) {
        let store = b.session.store();
        let exchange_names = b.session.market_source().core_exchange_names();
        let mut out = Vec::new();
        let mut server_keys = HashMap::new();
        for (id, name) in self.scope_cores(b) {
            let endpoint = store.core(id).and_then(|core| core.endpoint);
            server_keys.insert(
                id,
                endpoint
                    .map(|endpoint| ServerKey::Address(endpoint.address))
                    .unwrap_or(ServerKey::Unknown(id)),
            );
            if !self.sel_cores.is_empty() && !self.sel_cores.contains(&id) {
                continue;
            }
            let (status, sys) = store
                .core(id)
                .map(|c| (c.status.clone(), c.sys))
                .unwrap_or((ConnStatus::Disconnected, CoreSysStatus::default()));
            out.push(CoreStatusRow {
                id,
                name,
                status,
                sys,
                endpoint,
                exchange: exchange_names.get(&id).cloned(),
            });
        }
        (out, server_keys)
    }

    /// Rebuild flat and grouped snapshots, then reconcile MoonTree items.
    ///
    /// Args:
    ///     cx: View context used to read the backend and update tree state.
    ///
    /// Returns:
    ///     Nothing; all render caches are replaced atomically.
    fn rebuild_cache(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let signature = {
            let b = backend.read(cx);
            self.sys_sig(b)
        };
        self.rebuild_cache_with_signature(signature, cx);
    }

    /// Rebuild presentation caches without repeating an already-computed invalidation pass.
    ///
    /// Args:
    ///     signature: Ordered input signature computed by the backend observer or wrapper.
    ///     cx: View context used to read the backend and update tree state.
    ///
    /// Returns:
    ///     Nothing; flat rows, server groups, and tree items are replaced together.
    fn rebuild_cache_with_signature(&mut self, signature: u64, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let (rows, server_keys) = {
            let b = backend.read(cx);
            self.collect(b)
        };
        self.cache_sig = Some(signature);
        self.migrate_server_state(server_keys, cx);
        self.cached_groups = Rc::new(aggregate_servers(&rows));
        self.cached_rows = Rc::new(rows);
        self.rebuild_tree(cx);
    }

    /// Carry visibility and expansion across endpoint-key transitions for the same core.
    ///
    /// Args:
    ///     current: Current keys for every group-scoped core, including filtered-out cores.
    ///     cx: View context used to update MoonTree expansion ids.
    ///
    /// Returns:
    ///     Nothing; hidden keys and expanded ids migrate in place.
    fn migrate_server_state(
        &mut self,
        current: HashMap<CoreId, ServerKey>,
        cx: &mut Context<Self>,
    ) {
        let current_keys = current.values().copied().collect::<HashSet<_>>();
        let changes = current
            .iter()
            .filter_map(|(id, new)| {
                self.server_keys
                    .get(id)
                    .copied()
                    .filter(|old| old != new)
                    .map(|old| (old, *new))
            })
            .collect::<Vec<_>>();
        self.server_keys = current;
        if changes.is_empty() {
            return;
        }

        migrate_keyed_server_state(&mut self.hidden_servers, &changes, &current_keys, |key| key);

        let mut expanded = self
            .tree_state
            .read(cx)
            .expanded_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        migrate_keyed_server_state(&mut expanded, &changes, &current_keys, |key| {
            SharedString::from(key.tree_id())
        });
        self.tree_state
            .update(cx, |state, cx| state.set_expanded(expanded, cx));
    }

    /// Reconcile hierarchical items while preserving expansion ids.
    ///
    /// Args:
    ///     cx: View context used to update the MoonTree state entity.
    ///
    /// Returns:
    ///     Nothing; only tree items and initial-expansion bookkeeping change.
    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let items = server_view::tree_items(
            &self.cached_groups,
            &self.hidden_servers,
            !self.tree_initialized,
        );
        self.tree_initialized = true;
        self.tree_state
            .update(cx, |state, cx| state.set_items(items, cx));
    }

    /// Toggle one core in the multi-select filter, or toggle the All item.
    ///
    /// `Some(id)` toggles one core. `None` clears a selection containing every non-empty scoped
    /// core, otherwise replacing it with that full set. Stale ids do not stand in for current cores.
    ///
    /// Args:
    ///     id: Core to toggle, or `None` for the All row.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the in-memory filter and cached rows are updated in place.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        match id {
            None => crate::controls::toggle_all_core_selection(&mut self.sel_cores, all),
            Some(id) => {
                if !self.sel_cores.remove(&id) {
                    self.sel_cores.insert(id);
                }
            }
        }
        self.rebuild_cache(cx);
        cx.notify();
    }

    /// Toggle every still-available core from one clicked exchange section.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Rendered ids that left this panel's group are ignored.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a changed selection rebuilds the cache once, while a stale-only batch is a
    ///     no-op.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<CoreId>,
        cx: &mut Context<Self>,
    ) {
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

    /// Toggle local visibility for one server without changing feed subscriptions.
    ///
    /// Args:
    ///     key: Stable server identity whose process rows should be hidden or restored.
    ///     cx: View context used to rebuild tree items and repaint.
    ///
    /// Returns:
    ///     Nothing; visibility remains local to this panel instance.
    fn toggle_server(&mut self, key: ServerKey, cx: &mut Context<Self>) {
        if !self.hidden_servers.remove(&key) {
            self.hidden_servers.insert(key);
        }
        self.rebuild_tree(cx);
        cx.notify();
    }

    /// Switch between grouped and flat presentations.
    ///
    /// Args:
    ///     mode: Requested presentation.
    ///     cx: View context used to repaint.
    ///
    /// Returns:
    ///     Nothing; data caches and visibility state are retained.
    fn set_mode(&mut self, mode: CoreStatusMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            cx.notify();
        }
    }
}

/// Return a compact connection-status contribution to the cache signature.
///
/// Dynamic status text is folded exactly enough to repaint equal-length stage/error changes.
///
/// Args:
///     s: Current connection status.
///
/// Returns:
///     Stable numeric contribution including dynamic display text.
fn status_ord(s: &ConnStatus) -> u64 {
    match s {
        ConnStatus::Connecting => 1,
        ConnStatus::Stage(t) => 100u64.wrapping_add(text_sig(t)),
        ConnStatus::Ready => 2,
        ConnStatus::Failed(e) => 1000u64.wrapping_add(text_sig(e)),
        ConnStatus::Disconnected => 3,
    }
}

/// Fold display text into a deterministic cache-signature contribution.
///
/// Args:
///     text: Status or exchange label whose visible content matters.
///
/// Returns:
///     Stable FNV-1a value for this process and platform.
fn text_sig(text: &str) -> u64 {
    text.as_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

/// Move keyed UI state across endpoint changes while retaining still-live shared keys.
///
/// Args:
///     members: Hidden server keys or expanded tree ids to update in place.
///     changes: Old and new server keys for cores whose endpoint identity changed.
///     current_keys: Server keys that remain present after the cache rebuild.
///     identify: Projection from a server key to the stored membership type.
///
/// Returns:
///     Nothing; matching memberships are copied to new keys and obsolete old keys are removed.
fn migrate_keyed_server_state<T, F>(
    members: &mut HashSet<T>,
    changes: &[(ServerKey, ServerKey)],
    current_keys: &HashSet<ServerKey>,
    identify: F,
) where
    T: Clone + Eq + Hash,
    F: Fn(ServerKey) -> T,
{
    for (old, new) in changes {
        let old_member = identify(*old);
        if members.contains(&old_member) {
            members.insert(identify(*new));
            if !current_keys.contains(old) {
                members.remove(&old_member);
            }
        }
    }
}

impl EventEmitter<PanelEvent> for CoreStatusView {}
impl Focusable for CoreStatusView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for CoreStatusView {
    fn panel_name(&self) -> &'static str {
        "CoreStatus"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("CoreStatus", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![crate::persistence::table_persist::reset_button(
            "core-status-reset-widths",
            &self.table_state,
        )])
    }
}

impl Render for CoreStatusView {
    /// Render the active presentation with shared filters and counters.
    ///
    /// Args:
    ///     _window: Host window reserved for renderer symmetry.
    ///     cx: View context used for backend reads, palette, and callbacks.
    ///
    /// Returns:
    ///     Full dock, detached-window, or group-host panel contents.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cores = self.scope_cores(self.backend.read(cx));
        let rows = self.cached_rows.clone();
        let groups = self.cached_groups.clone();
        let p = MoonPalette::active(cx);
        let total_cores = rows.len();
        let now = moon_chart::paint::now_unix_ms() as i64;

        let core_bar = self.core_bar(&cores, cx);
        let footer = self.footer(&groups, total_cores, cx);
        let content = match self.mode {
            CoreStatusMode::ByIp => server_view::grouped_server_view(
                groups.clone(),
                Rc::new(self.hidden_servers.clone()),
                &self.tree_state,
                cx,
            ),
            CoreStatusMode::Flat => table::core_status_table(
                "core-status-table",
                Rc::new(model::visible_flat_rows(&rows, &self.hidden_servers)),
                now,
                &self.table_state,
                cx,
            )
            .into_any_element(),
        };

        v_flex()
            .id("core-status-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(core_bar)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(
                v_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(content),
            )
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}

impl CoreStatusView {
    /// Render the core multi-selector in the top bar.
    ///
    /// Clicking a known exchange header batch-toggles its currently available group cores.
    ///
    /// Args:
    ///     cores: Scoped cores in canonical display order.
    ///     cx: View context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     The top-bar row containing the fixed-trigger dropdown.
    fn core_bar(&self, cores: &OrderedCores, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let exchange_view = view.clone();
        let exchange_names = self
            .backend
            .read(cx)
            .session
            .market_source()
            .core_exchange_names();
        let combo = crate::controls::core_combo(
            "core-status-core",
            cores,
            &exchange_names,
            &self.sel_cores,
            t!("core_status.all_cores").to_string(),
            |n| t!("core_status.cores_n", n = n).to_string(),
            170.0,
            move |id, app| {
                view.update(app, |t, c| t.toggle_core(id, c));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        );
        let weak_view = cx.entity().downgrade();
        let modes = MoonSegmentedControl::new("core-status-mode")
            .items([
                MoonSegmentItem::new("", t!("core_status.mode.by_ip").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::ByIp),
                MoonSegmentItem::new("", t!("core_status.mode.flat").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Flat),
            ])
            .on_click(move |index, _, _, app| {
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                let mode = if index == 0 {
                    CoreStatusMode::ByIp
                } else {
                    CoreStatusMode::Flat
                };
                view.update(app, |this, cx| this.set_mode(mode, cx));
            });
        // The core selector yields first in a narrow side dock, keeping both mode actions
        // reachable without introducing a horizontal scroll host.
        h_flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(div().flex_1().min_w_0().overflow_hidden().child(combo))
            .child(modes)
    }

    /// Render server, visible-core, readiness, and hidden counters.
    ///
    /// Args:
    ///     groups: Current server snapshots.
    ///     total_cores: Cores in the current selector scope, including locally hidden servers.
    ///     cx: View context used for palette and localization.
    ///
    /// Returns:
    ///     Compact footer shared by grouped and flat presentations.
    fn footer(
        &self,
        groups: &[ServerStatusGroup],
        total_cores: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let online = groups
            .iter()
            .filter(|group| group.ready_count == group.cores.len() && !group.cores.is_empty())
            .count();
        let hidden = groups
            .iter()
            .filter(|group| self.hidden_servers.contains(&group.key))
            .count();
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .text_size(design::t_body(cx))
            .text_color(rgb(p.text_muted))
            .child(t!(
                "core_status.footer",
                servers = groups.len(),
                cores = total_cores,
                online = online,
                hidden = hidden
            ))
    }
}
