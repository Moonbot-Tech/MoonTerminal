//! Core Status panel: connection state and typed protocol-v4 resource telemetry
//! (`Event::KernelHealth`) for every core in scope.
//!
//! `CoreData::sys` holds the latest sample, while `sys_rev` invalidates the table
//! only when metric values change.
//!
//! Like the Assets panel, it is scoped to a window group and can live in a dock
//! tab or a detached window. [`crate::persistence::table_persist`] stores separate column
//! widths for `:dock` and `:win`. This module owns data and lifecycle; [`table`]
//! owns table rendering.

mod table;

use std::collections::HashSet;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonDataTableState, MoonPalette, Panel, PanelEvent, PanelState, h_flex, v_flex,
};

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use crate::panels::RenderGate;
use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreSysStatus};
use rust_i18n::t;

/// Cached table row containing a core name, connection state, and latest system telemetry sample.
#[derive(Clone)]
pub(super) struct CoreStatusRow {
    pub(super) name: String,
    pub(super) status: ConnStatus,
    pub(super) sys: CoreSysStatus,
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
    table_state: Entity<MoonDataTableState>,
    /// Context-qualified column-width persistence ID (`core-status-table:dock` or `:win`).
    widths_id: String,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl CoreStatusView {
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
            let b = backend.read(cx);
            let sig = this.sys_sig(b);
            let changed = this.cache_sig != Some(sig);
            let due = this.gate.should_notify(sig, now);
            if changed || due {
                this.rebuild_cache(b);
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

        let mut this = Self {
            backend,
            group,
            sel_cores: HashSet::new(),
            gate: RenderGate::default(),
            cache_sig: None,
            cached_rows: Rc::new(Vec::new()),
            table_state,
            widths_id,
            dock: None,
            focus: cx.focus_handle(),
        };
        let b = this.backend.clone();
        this.rebuild_cache(b.read(cx));
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

    /// Fold CoreId, system revision, and status into the ordered-cache signature.
    ///
    /// Include CoreId so canonical reordering invalidates the cache when state is unchanged.
    fn sys_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        self.scope_cores(b).iter().fold(0u64, |a, (id, _)| {
            let (sys_rev, st) = store
                .core(*id)
                .map(|c| (c.sys_rev, status_ord(&c.status)))
                .unwrap_or((0, 0));
            a.wrapping_mul(31)
                .wrapping_add(*id)
                .wrapping_mul(31)
                .wrapping_add(sys_rev)
                .wrapping_mul(31)
                .wrapping_add(st)
        })
    }

    fn collect(&self, b: &Backend) -> Vec<CoreStatusRow> {
        let store = b.session.store();
        let mut out = Vec::new();
        for (id, name) in self.scope_cores(b) {
            if !self.sel_cores.is_empty() && !self.sel_cores.contains(&id) {
                continue;
            }
            let (status, sys) = store
                .core(id)
                .map(|c| (c.status.clone(), c.sys))
                .unwrap_or((ConnStatus::Disconnected, CoreSysStatus::default()));
            out.push(CoreStatusRow { name, status, sys });
        }
        out
    }

    fn rebuild_cache(&mut self, b: &Backend) {
        self.cache_sig = Some(self.sys_sig(b));
        self.cached_rows = Rc::new(self.collect(b));
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
        let b = self.backend.clone();
        self.rebuild_cache(b.read(cx));
        cx.notify();
    }
}

/// Return a compact connection-status contribution to the cache signature.
///
/// `Stage` and `Failed` incorporate only their text length, so different messages
/// of the same length deliberately share this coarse invalidation value.
fn status_ord(s: &ConnStatus) -> u64 {
    match s {
        ConnStatus::Connecting => 1,
        ConnStatus::Stage(t) => 100 + t.len() as u64,
        ConnStatus::Ready => 2,
        ConnStatus::Failed(e) => 1000 + e.len() as u64,
        ConnStatus::Disconnected => 3,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cores = self.scope_cores(self.backend.read(cx));
        let rows = self.cached_rows.clone();
        let p = MoonPalette::active(cx);
        let count = rows.len();
        let now = moon_chart::paint::now_unix_ms() as i64;

        let core_bar = self.core_bar(&cores, cx);
        let footer = self.footer(count, cx);
        let table = table::core_status_table("core-status-table", rows, now, &self.table_state, cx);

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
                    .child(table),
            )
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}

impl CoreStatusView {
    /// Render the core multi-selector in the top bar.
    ///
    /// Args:
    ///     cores: Scoped cores in canonical display order.
    ///     cx: View context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     The top-bar row containing the fixed-trigger dropdown.
    fn core_bar(&self, cores: &OrderedCores, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let exchange_names = self
            .backend
            .read(cx)
            .session
            .market_source()
            .core_exchange_names();
        let combo = crate::controls::core_combo(
            cx,
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
        );
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(combo)
    }

    /// Render the core-count footer with the same visual treatment as Assets and Report.
    fn footer(&self, count: usize, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("core_status.cores").to_string()),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("{count}")),
            )
    }
}
