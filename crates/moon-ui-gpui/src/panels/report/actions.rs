//! [ReportPanel] filters, controlled row selection, row actions, column toggles, and export.

use super::*;

impl ReportPanel {
    /// Toggle a core in the multi-selection.
    ///
    /// `None` is the All toggle: it builds the current core set, then clears the selection when
    /// every current core is selected. Otherwise it replaces the selection with the current set;
    /// stale ids never stand in for current cores.
    ///
    /// Args:
    ///     uid: Core to toggle, or `None` for the All row.
    ///     cx: Panel context used to request a filtered database query.
    ///
    /// Returns:
    ///     Nothing; the in-memory filter is updated and a requery is scheduled.
    pub(super) fn toggle_core(&mut self, uid: Option<u64>, cx: &mut Context<Self>) {
        match uid {
            None => {
                let all: HashSet<u64> = self.cores.iter().map(|(u, _)| *u).collect();
                crate::controls::toggle_all_core_selection(&mut self.sel_cores, all);
            }
            Some(uid) => {
                if !self.sel_cores.remove(&uid) {
                    self.sel_cores.insert(uid);
                }
            }
        }
        self.reconcile_strategy_core(cx);
        self.request_requery(cx);
    }

    /// Toggle every still-available core from one clicked exchange section.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Database cores that disappeared after rendering are ignored.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: Panel context used to request a filtered database query.
    ///
    /// Returns:
    ///     Nothing; a changed selection schedules one requery, while a stale-only batch is a no-op.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<u64>,
        cx: &mut Context<Self>,
    ) {
        let available = self.cores.iter().map(|(core, _)| *core).collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.reconcile_strategy_core(cx);
            self.request_requery(cx);
        }
    }

    /// Set the filter to only the clicked Core-cell UID, or clear it when already the sole selection.
    ///
    /// This matches Core-cell filtering in Orders and Assets.
    ///
    /// Args:
    ///     uid: Core identity from the clicked report row.
    ///     cx: Panel context used to request the replacement query.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn filter_to_core(&mut self, uid: u64, cx: &mut Context<Self>) {
        if self.sel_cores.len() == 1 && self.sel_cores.contains(&uid) {
            self.sel_cores.clear();
        } else {
            self.sel_cores = HashSet::from([uid]);
        }
        self.reconcile_strategy_core(cx);
        self.request_requery(cx);
    }

    /// Apply the exact set emitted by the grouped multi-strategy combobox.
    ///
    /// Strategy selection stays independent of the core selector. Exact keys already carry their
    /// core identity, while rewriting `sel_cores` here would shrink the available strategy scope
    /// after the first checkbox and prevent adding a strategy from another core.
    ///
    /// Args:
    ///     choices: Exact strategy values after one checkbox or core-group toggle.
    ///     cx: Panel context used to request the replacement query.
    ///
    /// Returns:
    ///     Nothing; unchanged selections are ignored. The emitting combobox already owns the
    ///     matching widget-state mutation.
    pub(super) fn set_strategy_choices(
        &mut self,
        choices: &[ReportStrategyChoice],
        cx: &mut Context<Self>,
    ) {
        let selected_strategies = exact_strategy_selection(choices);
        if self.selected_strategies == selected_strategies {
            return;
        }
        self.selected_strategies = selected_strategies;
        self.request_requery(cx);
    }

    /// Remove every exact strategy whose core is excluded by an explicit core selection.
    ///
    /// Args:
    ///     cx: Panel context used to synchronize the retained selector.
    ///
    /// Returns:
    ///     Nothing; an implicit All selection remains implicit All. Removing the final exact key
    ///     returns to All, matching the shared core selector's empty-selection convention.
    fn reconcile_strategy_core(&mut self, cx: &mut Context<Self>) {
        let became_empty = if let Some(strategies) = &mut self.selected_strategies {
            if !self.sel_cores.is_empty() {
                strategies.retain(|strategy| self.sel_cores.contains(&strategy.core_uid));
            }
            strategies.is_empty()
        } else {
            false
        };
        if became_empty {
            self.selected_strategies = None;
        }
        self.queue_strategy_select_sync(false, cx);
    }

    /// Toggle all runtime columns on, or retain only the first when all are already visible.
    ///
    /// Keeping one column prevents the table from becoming entirely empty.
    pub(super) fn toggle_all_columns(&mut self, cx: &mut Context<Self>) {
        if self.all_columns_on() {
            self.visible = self.cols.first().cloned().into_iter().collect();
        } else {
            self.visible = self.cols.iter().cloned().collect();
        }
        self.persist_visible(cx);
        cx.notify();
    }
    pub(super) fn set_side(&mut self, s: SideFilter, cx: &mut Context<Self>) {
        if self.side != s {
            self.side = s;
            self.request_requery(cx);
        }
    }
    pub(super) fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {
        if self.period != p {
            self.period = p;
            self.request_requery(cx);
        }
    }
    /// Close the coin-match popup without changing the typed report filter.
    ///
    /// Unlike chart tabs, dismissing this popup leaves its text intact.
    pub(super) fn close_coin_popup(&mut self, cx: &mut Context<Self>) {
        if self.coin_popup_open {
            self.coin_popup_open = false;
            cx.notify();
        }
    }

    pub(super) fn set_kind(&mut self, k: ReportKind, cx: &mut Context<Self>) {
        if self.kind != k {
            self.kind = k;
            self.request_requery(cx);
        }
    }
    /// Toggle between active and deleted-only report rows.
    ///
    /// Args:
    ///     on: Whether the next query should show only soft-deleted trades.
    ///     cx: Panel context used to clear stale selection and request the query.
    ///
    /// Returns:
    ///     Nothing. Selection is cleared before the action bar can change to the opposite command.
    pub(super) fn set_deleted_only(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.deleted_only != on {
            self.deleted_only = on;
            self.selection.clear();
            self.request_requery(cx);
        }
    }

    /// Apply one table-row click to the controlled Report selection.
    ///
    /// Args:
    ///     row: Current visible row index.
    ///     modifiers: Native modifier snapshot from the owning window.
    ///     cx: Panel context used to repaint the selection and action bar.
    ///
    /// Returns:
    ///     Nothing. Shift takes precedence over Ctrl/Command.
    pub(super) fn select_report_row(
        &mut self,
        row: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some(data) = self.data.data() else {
            return;
        };
        self.selection.click(
            data.row_keys.get(row).copied().flatten(),
            &data.row_keys,
            modifiers.shift,
            modifiers.secondary(),
        );
        cx.notify();
    }

    /// Select every stable row in the current filtered and sorted Report table.
    ///
    /// Args:
    ///     cx: Panel context used to read the event-time result and repaint the action bar.
    ///
    /// Returns:
    ///     Nothing. The current table is capped by the report query's top-100 contract.
    pub(super) fn select_all_report_rows(&mut self, cx: &mut Context<Self>) {
        let Some(data) = self.data.data() else {
            return;
        };
        self.selection.select_all(&data.row_keys);
        cx.notify();
    }

    /// Clear the controlled Report row selection.
    ///
    /// Args:
    ///     cx: Panel context used to repaint the panel.
    ///
    /// Returns:
    ///     Nothing after the selection and Shift anchor have been cleared.
    pub(super) fn clear_report_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.clear();
        cx.notify();
    }

    /// Copy selected rows as spreadsheet-friendly TSV in current visual order.
    ///
    /// Args:
    ///     window: Owning window used to show the completion notification.
    ///     cx: Panel context used for table state and clipboard access.
    ///
    /// Returns:
    ///     Nothing when data, selection, or visible columns are empty.
    pub(super) fn copy_report_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.data.data() else {
            return;
        };
        let indices = selection::ordered_visible_indices(
            &self.cols,
            &self.visible,
            &self.table_state.read(cx),
        );
        if self.selection.len() == 0 || indices.is_empty() {
            return;
        }
        let text = selection::selected_tsv(data, &self.cols, &indices, &self.selection);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(
            MoonNotification::success(
                t!("report.selection.copied", n = self.selection.len()).to_string(),
            ),
            cx,
        );
    }

    /// Queue soft-delete or restore for every selected replicated row.
    ///
    /// Args:
    ///     deleted: `true` sends the protocol's delete flag; `false` sends restore.
    ///     window: Owning window used for queued/failure notifications.
    ///     cx: Panel context used to access sessions and repaint retained selection.
    ///
    /// Returns:
    ///     Nothing. Local queue acceptance is not treated as a core acknowledgement, so selected
    ///     rows remain selected until `ReportEvent::RowsDeleted` refreshes them out of this view.
    pub(super) fn mutate_report_selection(
        &mut self,
        deleted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(data) = self.data.data() else {
            return;
        };
        let targets = self.selection.mutation_targets(data);
        if targets.is_empty() {
            return;
        }
        let mut queued = 0usize;
        let mut failed = false;
        self.backend.update(cx, |backend, _| {
            for (core_uid, rec_ids) in targets {
                let count = rec_ids.len();
                if backend
                    .session
                    .set_report_rows_deleted(core_uid, deleted, Vec::new(), rec_ids)
                    .is_ok()
                {
                    queued += count;
                } else {
                    failed = true;
                }
            }
        });
        if queued > 0 {
            let note = if deleted {
                t!("report.selection.delete_queued", n = queued).to_string()
            } else {
                t!("report.selection.restore_queued", n = queued).to_string()
            };
            window.push_notification(MoonNotification::success(note), cx);
        }
        if failed {
            window.push_notification(
                MoonNotification::error(t!("report.delete_send_failed").to_string()),
                cx,
            );
        }
    }

    /// Toggle a column by name, clear invisible selection when needed, and persist the result.
    ///
    /// Hiding the last column writes an empty `app_meta` set but does not erase a prior per-context
    /// entry, which may restore that older non-empty set when the panel is recreated.
    ///
    /// # Arguments
    ///
    /// * `name` - Runtime report column name to show or hide.
    /// * `cx` - Panel context used to persist and redraw the new state.
    pub(super) fn toggle_column(&mut self, name: String, cx: &mut Context<Self>) {
        if self.visible.contains(name.as_str()) {
            self.visible.remove(&name);
        } else {
            self.visible.insert(name);
        }
        if self.visible.is_empty() {
            self.selection.clear();
        }
        self.persist_visible(cx);
        cx.notify();
    }
    /// Prompt for a destination, then export the current filter and sort order.
    ///
    /// `all_cols` selects the full runtime schema instead of visible columns. A
    /// `NotReady` or `Failed` read aborts before any file write; completion or
    /// failure is reported in the originating window.
    pub(super) fn export_report(
        &mut self,
        fmt: export::Format,
        all_cols: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cols: Vec<String> = if all_cols {
            (*self.cols).clone()
        } else {
            self.cols
                .iter()
                .filter(|c| self.visible.contains(c.as_str()))
                .cloned()
                .collect()
        };
        if cols.is_empty() {
            log::warn!("report export has no columns (table empty?)");
            return;
        }
        let filter = self.filter(cx);
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;
        let suggested = export::suggested_name(&filter, fmt, all_cols);
        let handle = window.window_handle();
        let rx = cx.prompt_for_new_path(&export::default_dir(), Some(&suggested));
        cx.spawn(async move |_this, cx| {
            // A cancelled or closed destination dialog requires no notification.
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    export::run(&path, fmt, &cols, &filter, &sort_key, sort_desc).map(|n| (n, path))
                })
                .await;
            let note = match result {
                Ok((n, path)) => {
                    log::info!("report exported {n} rows -> {}", path.display());
                    MoonNotification::success(t!("report.export.ok", n = n).to_string())
                }
                Err(e) => {
                    log::error!("report export failed: {e:#}");
                    // Do not auto-hide: the user must notice that the requested
                    // export did not complete.
                    MoonNotification::error(format!("{e}"))
                        .title(t!("report.export.fail").to_string())
                        .autohide(false)
                }
            };
            let _ = cx.update(|app| {
                let _ = handle.update(app, |_, window, app| {
                    use moon_ui::MoonWindowExt as _;
                    window.push_notification(note, app);
                });
            });
        })
        .detach();
    }
}
