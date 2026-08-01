//! [ReportPanel] construction, retained table state, and per-context column layout.

use super::*;

/// Insert or refresh a scoped strategy before the periodic metadata snapshot contains it.
///
/// Args:
///     strategies: Current exact strategy choices.
///     key: Scoped strategy identity.
///     name: Scoped display name.
///
/// Returns:
///     Nothing; an existing exact key receives the latest display name.
fn upsert_strategy_choice(
    strategies: &mut Vec<ReportStrategy>,
    key: ReportStrategyKey,
    name: String,
) {
    if let Some(strategy) = strategies.iter_mut().find(|strategy| strategy.key == key) {
        strategy.name = name;
    } else {
        strategies.push(ReportStrategy { key, name });
    }
}

impl ReportPanel {
    /// Create a regular docked or detachable Report panel with its default filters.
    ///
    /// Args:
    ///     backend: Shared application backend.
    ///     group: Group used by report coin search.
    ///     window: Owning GPUI window.
    ///     cx: Construction context.
    ///
    /// Returns:
    ///     A panel with the default Report filters and its first query scheduled.
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_scope(backend, group, None, window, cx)
    }

    /// Create a Report panel, seeding an optional Analytics scope before its first query.
    ///
    /// Args:
    ///     backend: Shared application backend.
    ///     group: Group used by report coin search.
    ///     scope: Optional exact Analytics filter to seed atomically.
    ///     window: Owning GPUI window.
    ///     cx: Construction context.
    ///
    /// Returns:
    ///     A scoped or default Report panel with one background query scheduled.
    pub(crate) fn new_with_scope(
        backend: Entity<Backend>,
        group: String,
        scope: Option<ReportScope>,
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
        // Strategy discovery can scan a large covering index, so the first background batch owns
        // it. A scoped window still seeds its selected label immediately.
        let mut strategies: Vec<ReportStrategy> = Vec::new();
        if let Some(scope) = &scope {
            upsert_strategy_choice(&mut strategies, scope.strategy, scope.strategy_name.clone());
        }
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
        let widths_id = crate::persistence::table_persist::ctx_id("report-table-v2", false);
        let mut saved_widths =
            crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        complete_widths(&mut saved_widths, &init_cols);
        let table_state = cx.new(|_| MoonDataTableState::new());
        table_state.update(cx, |state, _| {
            state.set_sort(sort_key.clone(), !sort_desc);
            state.column_widths = saved_widths;
        });
        // A column resize mutates table state; persist the exact user widths through shared storage.
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();

        let coin_query = String::new();
        let from_query = scope
            .as_ref()
            .and_then(|scope| scope.date_from)
            .map(db::fmt_unix_date)
            .unwrap_or_default();
        let to_query = scope
            .as_ref()
            .and_then(|scope| scope.date_to)
            .map(db::fmt_unix_date)
            .unwrap_or_default();
        let coin = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(coin_query.clone())
                .placeholder(t!("report.filter.coin_ph").to_string())
        });
        let from = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(from_query.clone())
                .placeholder(t!("report.filter.date_ph").to_string())
        });
        let to = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(to_query.clone())
                .placeholder(t!("report.filter.date_ph").to_string())
        });
        let selected_strategies = scope.as_ref().map(|scope| HashSet::from([scope.strategy]));
        // Scoped labels are seeded before the first metadata snapshot, so they are display choices
        // but not yet confirmed available choices.
        let available_strategy_keys = HashSet::new();
        let initial_selected_cores = scope
            .as_ref()
            .map(|scope| HashSet::from([scope.strategy.core_uid]))
            .unwrap_or_default();
        let ordered_cores = ordered_strategy_cores(&strategies, &cores, &backend.read(cx).config);
        let groups = strategy_groups(
            &strategies,
            &ordered_cores,
            &t!("report.all_strategies"),
            &t!("analytics.manual_orders"),
        );
        let strategy_select_indices =
            strategy_choice_indices(&groups, selected_strategies.as_ref());
        let strategy_search = ReportStrategyDelegate::search_state();
        let strategy_catalog =
            ReportStrategyDelegate::catalog(groups, available_strategy_keys.clone());
        let strategy_delegate = ReportStrategyDelegate::new(
            strategy_catalog.clone(),
            selected_strategies.as_ref(),
            strategy_search.clone(),
        );
        let strategy_select = cx.new(|cx| {
            MoonComboboxState::new(strategy_delegate, strategy_select_indices, window, cx)
                .multiple(true)
                .searchable(true)
        });
        cx.subscribe(
            &strategy_select,
            |panel, _, event: &MoonComboboxEvent<ReportStrategyDelegate>, cx| {
                let MoonComboboxEvent::Change(choices) = event else {
                    return;
                };
                // The widget already owns this selection mutation; only programmatic Report
                // mutations need to synchronize the widget back from panel state.
                panel.set_strategy_choices(choices, cx);
            },
        )
        .detach();
        // Any coin-field change requests a query. Manual input also opens the match popup. An
        // `on_pick` substitution updates `coin_query` first, so either a synchronous or deferred
        // Change event sees the mirror already matched and does not reopen the popup.
        cx.subscribe(&coin, |t, e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = e.read(cx).value().to_string();
                if t.coin_query != value {
                    t.coin_query = value;
                    t.coin_popup_open = !t.coin_query.trim().is_empty();
                    t.request_requery(cx);
                }
            }
        })
        .detach();
        // A non-empty manual date switches to All so a preset cannot silently take precedence.
        cx.subscribe(&from, |t, e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = e.read(cx).value().to_string();
                if t.from_query != value {
                    t.from_query = value;
                    if !t.from_query.trim().is_empty() {
                        t.period = Period::All;
                    }
                    t.request_requery(cx);
                }
            }
        })
        .detach();
        cx.subscribe(&to, |t, e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = e.read(cx).value().to_string();
                if t.to_query != value {
                    t.to_query = value;
                    if !t.to_query.trim().is_empty() {
                        t.period = Period::All;
                    }
                    t.request_requery(cx);
                }
            }
        })
        .detach();
        // The dedicated report-revision channel avoids repainting every shell on each commit.
        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _revision, cx| {
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
            strategies,
            available_strategy_keys,
            cols: Rc::new(init_cols),
            data: LoadState::default(),
            sort_key,
            sort_desc,
            sel_cores: initial_selected_cores,
            selected_strategies,
            strategy_select,
            strategy_catalog,
            strategy_search,
            strategy_select_items_dirty: false,
            strategy_select_selection_dirty: false,
            coin,
            coin_query,
            coin_popup_open: false,
            from,
            from_query,
            to,
            to_query,
            side: scope
                .as_ref()
                .map(|scope| scope.side)
                .unwrap_or(SideFilter::All),
            period: if scope.is_some() {
                Period::All
            } else {
                Period::Today
            },
            kind: scope
                .as_ref()
                .map(|scope| ReportKind::from_filter(scope.emulator))
                .unwrap_or(ReportKind::Real),
            deleted_only: false,
            closed_only: scope.is_some(),
            selection: ReportSelection::default(),
            needs_query: true,
            query_inflight: false,
            query_seq: 0,
            last_query_start: None,
            throttle_armed: false,
            last_metadata_at: None,
            last_strategy_scope: None,
            visible,
            table_state,
            widths_id,
            natural_widths: NaturalWidthsCache::default(),
            detached: false,
            standalone: false,
            dock: None,
            focus: cx.focus_handle(),
        };
        // A per-context shared-storage column set overrides the one loaded from `app_meta`. Without
        // one for this mode, the `app_meta` set remains as a migration seed.
        this.apply_ctx_columns(cx);
        this.schedule_requery(cx);
        this
    }

    /// Replace the standalone window's exact strategy and inherited Analytics filters.
    ///
    /// Args:
    ///     scope: Replacement exact strategy, date, side, and emulator filters.
    ///     window: Existing standalone Report window.
    ///     cx: Panel context used to update inputs and request one query.
    ///
    /// Returns:
    ///     Nothing; row selection and unrelated coin filters are cleared.
    pub(crate) fn apply_scope(
        &mut self,
        scope: ReportScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        upsert_strategy_choice(&mut self.strategies, scope.strategy, scope.strategy_name);
        self.group = report_group_for_core(&self.backend, scope.strategy.core_uid, cx);
        self.sel_cores = HashSet::from([scope.strategy.core_uid]);
        self.selected_strategies = Some(HashSet::from([scope.strategy]));
        self.queue_strategy_select_sync(true, cx);
        self.side = scope.side;
        self.kind = ReportKind::from_filter(scope.emulator);
        self.period = Period::All;
        self.deleted_only = false;
        self.closed_only = true;
        self.selection.clear();
        self.coin_query.clear();
        self.coin_popup_open = false;
        self.from_query = scope.date_from.map(db::fmt_unix_date).unwrap_or_default();
        self.to_query = scope.date_to.map(db::fmt_unix_date).unwrap_or_default();

        self.coin.update(cx, |input, input_cx| {
            input.set_value(String::new(), window, input_cx)
        });
        let from = self.from_query.clone();
        self.from.update(cx, |input, input_cx| {
            input.set_value(from, window, input_cx)
        });
        let to = self.to_query.clone();
        self.to
            .update(cx, |input, input_cx| input.set_value(to, window, input_cx));
        self.request_requery(cx);
    }

    /// Schedule synchronization of the retained strategy combobox on its next render.
    ///
    /// Args:
    ///     refresh_items: Whether core/strategy metadata changed and groups must be rebuilt.
    ///     cx: Panel context used to request a render with a live window handle.
    ///
    /// Returns:
    ///     Nothing; repeated requests coalesce before element construction.
    pub(super) fn queue_strategy_select_sync(
        &mut self,
        refresh_items: bool,
        cx: &mut Context<Self>,
    ) {
        self.strategy_select_items_dirty |= refresh_items;
        self.strategy_select_selection_dirty = true;
        cx.notify();
    }

    /// Apply queued strategy metadata and selection changes with the owning window available.
    ///
    /// Args:
    ///     window: Owning window required by the MoonUI combobox replacement API.
    ///     cx: Panel render context used to update the retained entity.
    ///
    /// Returns:
    ///     Nothing; selected values outside the retained query remain selected by exact identity.
    pub(super) fn flush_strategy_select_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.strategy_select_items_dirty && !self.strategy_select_selection_dirty {
            return;
        }
        if self.strategy_select_items_dirty {
            let ordered_cores = ordered_strategy_cores(
                &self.strategies,
                &self.cores,
                &self.backend.read(cx).config,
            );
            let groups = strategy_groups(
                &self.strategies,
                &ordered_cores,
                &t!("report.all_strategies"),
                &t!("analytics.manual_orders"),
            );
            self.strategy_catalog =
                ReportStrategyDelegate::catalog(groups, self.available_strategy_keys.clone());
        }
        let selected = self
            .strategy_catalog
            .selected_indices(self.selected_strategies.as_ref());
        let unfiltered = ReportStrategyDelegate::unfiltered(
            self.strategy_catalog.clone(),
            self.selected_strategies.as_ref(),
            self.strategy_search.clone(),
        );
        let filtered = ReportStrategyDelegate::new(
            self.strategy_catalog.clone(),
            self.selected_strategies.as_ref(),
            self.strategy_search.clone(),
        );
        self.strategy_select.update(cx, |select, select_cx| {
            // Synchronize against a full delegate, then restore the retained filtered view. The
            // selection snapshot keeps item values even when their rows are outside the query.
            select.set_items(unfiltered, window, select_cx);
            select.set_selected_indices(selected, window, select_cx);
            select.set_items(filtered, window, select_cx);
        });
        self.strategy_select_items_dirty = false;
        self.strategy_select_selection_dirty = false;
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
        self.widths_id = crate::persistence::table_persist::ctx_id("report-table-v2", true);
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

    /// Enable the dedicated Report title bar while retaining detached table controls.
    ///
    /// Args:
    ///     window: Standalone Report window whose geometry is persisted.
    ///     cx: Panel context used to restore detached widths and repaint.
    ///
    /// Returns:
    ///     Nothing.
    pub(crate) fn mark_standalone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mark_table_detached(cx);
        self.standalone = true;
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::window::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |backend, _| {
                if backend
                    .layout
                    .report_window
                    .map(|geometry| (geometry.x, geometry.y, geometry.w, geometry.h))
                    != Some((x, y, w, h))
                {
                    backend.layout.report_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    backend.layout_dirty = true;
                }
            });
        })
        .detach();
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

#[cfg(test)]
mod tests;
