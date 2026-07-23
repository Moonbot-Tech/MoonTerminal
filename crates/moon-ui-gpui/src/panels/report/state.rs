//! [ReportPanel] construction, retained table state, and per-context column layout.

use super::*;

impl ReportPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let generation = backend
            .read(cx)
            .reports
            .as_ref()
            .map(|h| h.generation.clone());
        // Keep this connection for panel metadata. Startup core/schema probes
        // are deliberately lossy because the fallible background batch below
        // owns user-visible read errors.
        let conn = db::open_reader().ok();
        let cores = conn
            .as_ref()
            .and_then(|c| db::distinct_cores(c).ok())
            .unwrap_or_default();
        let last_gen = generation
            .as_ref()
            .map(|g| g.load(Ordering::Relaxed))
            .unwrap_or(0);
        // Restore visible column names from `app_meta`, or use the defaults when never saved.
        let visible: HashSet<String> = conn
            .as_ref()
            .and_then(db::load_visible)
            .map(|saved| saved.into_iter().collect())
            .unwrap_or_else(|| DEFAULT_VISIBLE.iter().map(|c| c.to_string()).collect());
        // Probe the initial database schema so the first render and column menu are complete.
        let init_cols = conn
            .as_ref()
            .and_then(|c| db::display_columns(c).ok())
            .unwrap_or_default();
        let (sort_key, sort_desc) = conn
            .as_ref()
            .and_then(db::load_sort)
            .unwrap_or_else(|| ("buydate".to_string(), true));
        let widths_id = crate::persistence::table_persist::ctx_id("report-table", false);
        let mut saved_widths =
            crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        complete_widths(&mut saved_widths, &init_cols);
        let table_state = cx.new(|_| MoonDataTableState::new());
        table_state.update(cx, |state, _| {
            state.set_sort(sort_key.clone(), !sort_desc);
            state.column_widths = saved_widths;
        });
        // A column resize mutates table state; clamp visible widths to the viewport budget, then
        // persist them through the shared storage.
        cx.observe(&table_state, |this, state, cx| {
            this.clamp_table_widths(&state, cx);
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();

        let coin = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.coin_ph").to_string())
        });
        let from = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.date_ph").to_string())
        });
        let to = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.date_ph").to_string())
        });
        // Any coin-field change requests a query. Manual input also opens the match popup. An
        // `on_pick` substitution updates `coin_query` first, so either a synchronous or deferred
        // Change event sees the mirror already matched and does not reopen the popup.
        cx.subscribe(&coin, |t, e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = e.read(cx).value().to_string();
                if t.coin_query != value {
                    t.coin_query = value;
                    t.coin_popup_open = !t.coin_query.trim().is_empty();
                }
                t.request_requery(cx);
            }
        })
        .detach();
        // A non-empty manual date switches to All so a preset cannot silently take precedence.
        for st in [&from, &to] {
            cx.subscribe(st, |t, e, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    if !e.read(cx).value().trim().is_empty() {
                        t.period = Period::All;
                    }
                    t.request_requery(cx);
                }
            })
            .detach();
        }
        // Backend observation requests a refresh only when the report writer's generation changes.
        // Filter edits notify and query immediately; generation-driven refreshes use the throttle.
        cx.observe(&backend, |this, _b, cx| {
            if let Some(g) = &this.generation {
                let v = g.load(Ordering::Relaxed);
                if v != this.last_gen {
                    this.last_gen = v;
                    this.requery_on_generation(cx);
                }
            }
        })
        .detach();

        let mut this = Self {
            backend,
            group,
            generation,
            last_gen,
            conn,
            cores,
            cols: Rc::new(init_cols),
            data: LoadState::default(),
            sort_key,
            sort_desc,
            sel_cores: HashSet::new(),
            coin,
            coin_query: String::new(),
            coin_popup_open: false,
            from,
            to,
            side: SideFilter::All,
            period: Period::Today,
            kind: ReportKind::Real,
            deleted_only: false,
            delete_mode: false,
            pending_deleted: std::collections::HashMap::new(),
            needs_query: true,
            query_inflight: false,
            query_seq: 0,
            last_query_start: None,
            throttle_armed: false,
            last_cores_at: None,
            visible,
            table_state,
            widths_id,
            detached: false,
            last_widths: std::collections::HashMap::new(),
            dock: None,
            focus: cx.focus_handle(),
        };
        // A per-context shared-storage column set overrides the one loaded from `app_meta`. Without
        // one for this mode, the `app_meta` set remains as a migration seed.
        this.apply_ctx_columns(cx);
        this.schedule_requery(cx);
        this
    }

    /// Return retained table state for the detached window's automatic-width reset button.
    pub(crate) fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Switch a newly created detached panel to the `:win` column-storage context.
    ///
    /// Reload widths and visible columns for detached mode so docked tabs and windows keep separate
    /// layouts, and enable the detached-only manual date controls.
    pub(crate) fn mark_table_detached(&mut self, cx: &mut Context<Self>) {
        self.detached = true;
        self.widths_id = crate::persistence::table_persist::ctx_id("report-table", true);
        let mut saved =
            crate::persistence::table_persist::saved(self.backend.read(cx), &self.widths_id);
        complete_widths(&mut saved, &self.cols);
        self.table_state.update(cx, |s, c| {
            s.column_widths = saved;
            c.notify();
        });
        self.apply_ctx_columns(cx);
        cx.notify();
    }

    /// Apply a saved per-context visible-column set for `widths_id`, when non-empty.
    ///
    /// Docked `:dock` and detached `:win` modes have distinct shared-storage entries. An empty set
    /// leaves the current selection intact rather than producing an empty table.
    pub(super) fn apply_ctx_columns(&mut self, cx: &App) {
        if let Some(keys) =
            crate::persistence::table_persist::visible(self.backend.read(cx), &self.widths_id)
        {
            let set: HashSet<String> = keys.into_iter().collect();
            if !set.is_empty() {
                self.visible = set;
            }
        }
    }

    /// Save visible columns in runtime table order to per-context storage under `widths_id`.
    ///
    /// Called through [`Self::persist_visible`] after column-menu changes. An empty set is not
    /// written, so an older non-empty per-context entry remains available and can override the empty
    /// `app_meta` set when the panel is recreated.
    pub(super) fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .map(|c| c.to_string())
            .collect();
        if !keys.is_empty() {
            crate::persistence::table_persist::set_visible(
                &self.backend,
                &self.widths_id,
                keys,
                cx,
            );
        }
    }

    /// Return visible columns in runtime-schema order for stable rendering and persistence.
    pub(super) fn visible_cols(&self) -> Vec<&str> {
        self.cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .map(|c| c.as_str())
            .collect()
    }

    /// Return whether every runtime column is enabled in the Columns menu.
    pub(super) fn all_columns_on(&self) -> bool {
        !self.cols.is_empty() && self.cols.iter().all(|c| self.visible.contains(c.as_str()))
    }

    /// Persist visible columns to `app_meta` and, when non-empty, the dock/window table descriptor.
    ///
    /// `app_meta` records an empty set, but [`Self::save_ctx_columns`] deliberately leaves any older
    /// per-context descriptor unchanged in that case.
    pub(super) fn persist_visible(&self, cx: &mut App) {
        if let Some(conn) = &self.conn {
            db::save_visible(conn, &self.visible_cols());
        }
        self.save_ctx_columns(cx);
    }
}
