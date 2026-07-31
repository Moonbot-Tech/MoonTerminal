//! [ReportPanel] user actions: core/side/period/kind filters, deletion mode, column toggles, and export.

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

    /// Apply a core-wide, exact-strategy, or global strategy-filter choice.
    ///
    /// Args:
    ///     choice: Grouped combobox choice, or `None` for every core and strategy.
    ///     cx: Panel context used to request the replacement query.
    ///
    /// Returns:
    ///     Nothing; unchanged selections are ignored. The emitting combobox already owns the
    ///     matching widget-state mutation.
    pub(super) fn set_strategy_choice(
        &mut self,
        choice: Option<ReportStrategyChoice>,
        cx: &mut Context<Self>,
    ) {
        let (sel_cores, strategy) = filters_for_strategy_choice(choice);
        if self.sel_cores == sel_cores && self.strategy == strategy {
            return;
        }
        self.sel_cores = sel_cores;
        self.strategy = strategy;
        self.request_requery(cx);
    }

    /// Clear a strategy whose core is excluded by an explicit core selection.
    ///
    /// Args:
    ///     cx: Panel context used to synchronize the retained selector.
    ///
    /// Returns:
    ///     Nothing; an implicit All selection retains the strategy.
    fn reconcile_strategy_core(&mut self, cx: &mut Context<Self>) {
        if let Some(strategy) = self.strategy {
            if !self.sel_cores.is_empty() && !self.sel_cores.contains(&strategy.core_uid) {
                self.strategy = None;
            }
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
    /// Toggle the deleted-trades checkbox: off hides soft-deleted trades, on shows only them.
    pub(super) fn set_deleted_only(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.deleted_only != on {
            self.deleted_only = on;
            self.request_requery(cx);
        }
    }

    /// Whether the deletion-mode Save is armed: in mode with at least one uncommitted edit.
    pub(super) fn delete_dirty(&self) -> bool {
        self.delete_mode && !self.pending_deleted.is_empty()
    }

    /// The trash button: enter deletion mode, or — once in it — Save the edits when any exist, else
    /// leave the mode. This one control is both the toggle and the commit, per the panel spec. The
    /// `deleted` checkbox column is force-shown while the mode is on (see the render), so entering
    /// needs no persisted column change.
    pub(super) fn on_delete_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.delete_mode {
            self.delete_mode = true;
            cx.notify();
        } else if self.pending_deleted.is_empty() {
            self.delete_mode = false;
            cx.notify();
        } else {
            self.commit_deletions(window, cx);
        }
    }

    /// Record one checkbox edit. Stores the desired flag only when it DIFFERS from the database
    /// value `orig`, so toggling a row back to its original state disarms it. Legacy rows
    /// (`rec == 0`) have no `newrecid` and cannot be soft-deleted, so they are ignored.
    pub(super) fn set_row_deleted(
        &mut self,
        core: u64,
        rec: i64,
        want: bool,
        orig: bool,
        cx: &mut Context<Self>,
    ) {
        if rec == 0 {
            return;
        }
        if want == orig {
            self.pending_deleted.remove(&(core, rec));
        } else {
            self.pending_deleted.insert((core, rec), want);
        }
        cx.notify();
    }

    /// Send the pending edits to their cores, grouped into one soft-delete and one restore batch
    /// per core. Edits whose core accepted the send are dropped and the mode is left; edits whose
    /// core rejected it (offline / removed from the session set) are KEPT so the amber Save stays
    /// armed and the intent is not silently lost, with a toast reporting the failure. The rows
    /// change on screen only when each core's `ReportEvent::RowsDeleted` echo lands and the replica
    /// refreshes — not optimistically.
    pub(super) fn commit_deletions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // (to_delete, to_restore) newrecid lists per core.
        let mut per_core: std::collections::HashMap<u64, (Vec<i64>, Vec<i64>)> =
            std::collections::HashMap::new();
        for (&(core, rec), &want) in self.pending_deleted.iter() {
            let entry = per_core.entry(core).or_default();
            if want {
                entry.0.push(rec);
            } else {
                entry.1.push(rec);
            }
        }
        let mut failed: std::collections::HashSet<u64> = std::collections::HashSet::new();
        self.backend.update(cx, |b, _| {
            for (core, (del, res)) in per_core {
                for (deleted, recs) in [(true, del), (false, res)] {
                    if !recs.is_empty()
                        && b.session
                            .set_report_rows_deleted(core, deleted, Vec::new(), recs)
                            .is_err()
                    {
                        failed.insert(core);
                    }
                }
            }
        });
        // Keep only the edits whose core rejected the send; the rest reached their core.
        self.pending_deleted
            .retain(|(core, _), _| failed.contains(core));
        if failed.is_empty() {
            self.delete_mode = false;
        } else {
            window.push_notification(
                MoonNotification::error(t!("report.delete_send_failed").to_string()),
                cx,
            );
        }
        cx.notify();
    }
    /// Toggle a column by name and persist it using [`Self::persist_visible`].
    ///
    /// Hiding the last column writes an empty `app_meta` set but does not erase a prior per-context
    /// entry, which may restore that older non-empty set when the panel is recreated.
    pub(super) fn toggle_column(&mut self, name: String, cx: &mut Context<Self>) {
        if self.visible.contains(name.as_str()) {
            self.visible.remove(&name);
        } else {
            self.visible.insert(name);
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
