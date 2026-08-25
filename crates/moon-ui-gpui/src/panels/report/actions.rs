//! [ReportPanel] filters, controlled row selection, row actions, column toggles, and export.

use super::state::{ReportPreferenceWrite, schedule_report_preference};
use super::*;

/// Workspace authority captured while an export destination picker is open.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ReportExportScopeIdentity {
    /// Group-workspace generation, or `None` for an explicit standalone Report scope.
    workspace_generation: Option<u64>,
    /// Deterministic effective core ids represented by the export request.
    core_ids: Vec<CoreId>,
}

/// Return whether a post-picker export still belongs to its originating scope.
///
/// Args:
///     requested: Scope captured before opening the native path picker.
///     current: Scope rebuilt from the live panel and Backend after the picker returns.
///
/// Returns:
///     `true` only when neither group generation nor effective core membership changed.
fn report_export_scope_is_current(
    requested: &ReportExportScopeIdentity,
    current: &ReportExportScopeIdentity,
) -> bool {
    requested == current
}

impl ReportPanel {
    /// Toggle a core in the retained Classic or standalone multi-selection.
    ///
    /// `None` is the All toggle and clears the explicit selection back to the empty-means-all
    /// state. `Some(uid)` toggles one core. Group Auto mode owns the effective scope and leaves this
    /// retained selection unchanged.
    ///
    /// Args:
    ///     uid: Core to toggle, or `None` for the All row.
    ///     cx: Panel context used to request a filtered database query.
    ///
    /// Returns:
    ///     Nothing; Classic or standalone mode schedules a requery, while group Auto is a no-op.
    pub(super) fn toggle_core(&mut self, uid: Option<u64>, cx: &mut Context<Self>) {
        if self
            .workspace_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_workspace_owned())
        {
            return;
        }
        if !crate::controls::toggle_core_selection(&mut self.sel_cores, uid) {
            return;
        }
        self.reconcile_strategy_core(cx);
        self.request_requery(cx);
    }

    /// Toggle one exchange section in the retained Classic or standalone core filter.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Database cores that disappeared after rendering are ignored. Group Auto mode leaves the
    /// retained selection unchanged.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: Panel context used to request a filtered database query.
    ///
    /// Returns:
    ///     Nothing; a retained-scope change schedules one requery, while stale-only and group Auto
    ///     calls are no-ops.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<u64>,
        cx: &mut Context<Self>,
    ) {
        if self
            .workspace_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_workspace_owned())
        {
            return;
        }
        let available = self.cores.iter().map(|(core, _)| *core).collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.reconcile_strategy_core(cx);
            self.request_requery(cx);
        }
    }

    /// Handle a Core-cell click under standalone, Classic, or Auto authority.
    ///
    /// Standalone and Classic group Reports set the retained filter to the clicked core, or clear
    /// it when that core is already the sole selection. Group Auto ignores this shortcut because
    /// only the Shell rail owns selection.
    ///
    /// Args:
    ///     uid: Core identity from the clicked report row.
    ///     cx: Panel context used to request the replacement query.
    ///
    /// Returns:
    ///     Nothing; only a retained-filter change requests a local requery.
    pub(super) fn filter_to_core(&mut self, uid: u64, cx: &mut Context<Self>) {
        if self
            .workspace_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_workspace_owned())
        {
            return;
        }
        self.sel_cores = crate::controls::next_core_filter(&self.sel_cores, &[uid], false);
        self.reconcile_strategy_core(cx);
        self.request_requery(cx);
    }

    /// Replace the retained filter with the one the Profit Monitor broadcast.
    ///
    /// A STANDALONE Report never adopts. That window is opened from Analytics already seeded with
    /// an exact core and strategy, and `reconcile_strategy_core` would drop the pinned strategy
    /// with no way back — the same reason every other revision observer here excludes it.
    ///
    /// For a group Report, `apply_core_broadcast` owns the release / ignore / intersect rule. The
    /// retained set is written even under Auto, where it is dormant; only the requery is skipped.
    ///
    /// Args:
    ///     cx: Panel context used to request the replacement query.
    ///
    /// Returns:
    ///     Nothing; standalone, a broadcast about other groups, and an unchanged selection all
    ///     requery nothing.
    pub(super) fn adopt_broadcast_core_filter(&mut self, cx: &mut Context<Self>) {
        let broadcast = self.backend.read(cx).core_filter().clone();
        // Nothing published and nothing retained: leave before resolving any core list.
        if broadcast.is_empty() && self.sel_cores.is_empty() {
            return;
        }
        // An absent workspace scope IS the standalone window, the same authority every other
        // revision observer here reads. Resolved before the write: `is_workspace_owned` depends on
        // the group's mode, not on the retained set.
        let Some(scope) = self.workspace_scope(self.backend.read(cx)) else {
            return;
        };
        // Both universes, because neither alone is complete: `self.cores` comes from the report
        // database and is still EMPTY until the first metadata batch lands — a Report created while
        // a filter is on air would otherwise never join it — while the live group holds cores that
        // have traded nothing yet and so appear in no report row.
        let available: Vec<CoreId> = self
            .cores
            .iter()
            .map(|(core, _)| *core)
            .chain(
                self.backend
                    .read(cx)
                    .group_cores(&self.group)
                    .into_iter()
                    .map(|(core, _)| core),
            )
            .collect();
        if !crate::controls::apply_core_broadcast(&mut self.sel_cores, &broadcast, available) {
            return;
        }
        if scope.is_workspace_owned() {
            return;
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
    pub(super) fn reconcile_strategy_core(&mut self, cx: &mut Context<Self>) {
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

    /// Toggle all contextually available runtime columns.
    ///
    /// Keeping one available column prevents the table from becoming entirely empty. Columns
    /// hidden only by the current display lens retain their raw saved preference.
    ///
    /// Args:
    ///     cx: Panel context used to resolve workspace scope, persist, and repaint.
    ///
    /// Returns:
    ///     Nothing; an empty runtime schema leaves the saved set untouched.
    pub(super) fn toggle_all_columns(&mut self, cx: &mut Context<Self>) {
        let hide_core_name = self.hide_core_name_column(self.backend.read(cx));
        let visible = columns::toggled_all_columns(&self.cols, &self.visible, hide_core_name);
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        self.persist_visible(cx);
        cx.notify();
    }

    /// Store this host context's six toolbar filters.
    ///
    /// Replaces one complete map entry per mutation because the six values form one filter set. The
    /// replacement still merges one value deliberately: unless this call represents an explicit
    /// period-menu pick, it carries the already-stored period forward. An Analytics-scoped panel
    /// writes nothing — see [`ReportPanel::scoped`].
    ///
    /// The period is the ONE member that is not read off the live panel. `self.period` also shows
    /// the implicit "all" that typing a manual date produces, which is not a preset anybody chose
    /// and must not replace the stored one — so every caller but the period menu passes `None` and
    /// the stored value is carried through untouched.
    ///
    /// Args:
    ///     picked_period: The preset the user just chose from the menu, or `None` to keep whatever
    ///         is already stored.
    ///     cx: Panel context used to reach the backend layout.
    ///
    /// Returns:
    ///     Nothing; the marked layout is flushed by the shared debounced and quit save path.
    pub(super) fn persist_filters(
        &mut self,
        picked_period: Option<Period>,
        cx: &mut Context<Self>,
    ) {
        if self.scoped {
            return;
        }
        let id = filters_ctx_id(self.detached);
        let period_bucket =
            period_bucket_for_scope(self.workspace_scope(self.backend.read(cx)).as_ref());
        let prefs = {
            let backend = self.backend.read(cx);
            next_prefs_for_period_pick(
                crate::persistence::table_persist::report_filters(&backend, &id),
                period_bucket,
                picked_period,
                &super::state::ReportFilterSet {
                    side: self.side,
                    kind: self.kind,
                    deleted_only: self.deleted_only,
                    show_open: self.show_open,
                    period: self.period,
                    strategy_name_mask: self.strategy_name_mask.clone(),
                },
            )
        };
        crate::persistence::table_persist::set_report_filters(&self.backend, &id, prefs, cx);
    }

    /// Select a direction filter, persist the changed set, and request fresh rows.
    ///
    /// Args:
    ///     s: Direction selected from the toolbar menu.
    ///     cx: Panel context used to persist and requery.
    ///
    /// Returns:
    ///     Nothing; selecting the current direction is a no-op.
    pub(super) fn set_side(&mut self, s: SideFilter, cx: &mut Context<Self>) {
        if self.side != s {
            self.side = s;
            self.persist_filters(None, cx);
            self.request_requery(cx);
        }
    }

    /// Record an explicit period-menu pick and apply a changed preset to the query.
    ///
    /// Persistence precedes the value guard because an explicit pick may match the implicit
    /// `Period::All` shown after a manual date while still replacing an older stored preset.
    ///
    /// Args:
    ///     p: Period preset explicitly selected from the toolbar menu.
    ///     cx: Panel context used to persist and requery.
    ///
    /// Returns:
    ///     Nothing; an unchanged displayed period is stored without issuing another query.
    pub(super) fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {
        // Stored OUTSIDE the changed-value guard, because a menu pick that changes nothing on
        // screen can still change what is stored: the visible preset may be the implicit "all" a
        // typed date produced, in which case this click is the user replacing an older stored pick
        // with one that merely happens to match what is already displayed.
        self.persist_filters(Some(p), cx);
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

    /// Select an order-kind filter, persist the changed set, and request fresh rows.
    ///
    /// Args:
    ///     k: Order kind selected from the toolbar menu.
    ///     cx: Panel context used to persist and requery.
    ///
    /// Returns:
    ///     Nothing; selecting the current kind is a no-op.
    pub(super) fn set_kind(&mut self, k: ReportKind, cx: &mut Context<Self>) {
        if self.kind != k {
            self.kind = k;
            self.persist_filters(None, cx);
            self.request_requery(cx);
        }
    }
    /// Show or hide the full-width comment pane and remember the choice.
    ///
    /// Args:
    ///     cx: Panel context used to persist the flag and repaint.
    ///
    /// Returns:
    ///     Nothing. This is a display toggle: it changes no filter and triggers no query.
    pub(super) fn toggle_comment_pane(&mut self, cx: &mut Context<Self>) {
        self.show_comment = !self.show_comment;
        self.preference_revisions.comment = self.preference_revisions.comment.wrapping_add(1);
        let detached = self.detached;
        let show_comment = self.show_comment;
        schedule_report_preference(
            cx,
            ReportPreferenceWrite::Comment {
                detached,
                visible: show_comment,
            },
        );
        cx.notify();
    }

    /// Toggle between active and deleted-only report rows.
    ///
    /// Args:
    ///     on: Whether the next query should show only soft-deleted trades.
    ///     cx: Panel context used to clear stale selection and request the query.
    ///
    /// Returns:
    ///     Nothing. Selection is cleared before the totals-row commands can flip to the opposite one.
    pub(super) fn set_deleted_only(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.deleted_only != on {
            self.deleted_only = on;
            self.selection.clear();
            self.persist_filters(None, cx);
            self.request_requery(cx);
        }
    }

    /// Include or exclude still-running positions in the rows, the totals, and the export.
    ///
    /// One switch reaches all three because they share ONE `ReportFilter` — see
    /// [`ReportPanel::filter`] — so a footer that totals trades the table is not showing is not
    /// merely avoided here but unrepresentable. `closed_only` still wins over this value; the
    /// precedence lives in [`super::row_scope_for`].
    ///
    /// Unlike [`Self::set_deleted_only`] this does NOT clear the selection. That one clears
    /// because the totals-row command flips between Delete and Restore and would otherwise act on
    /// rows from the opposite universe; nothing analogous exists on the lifecycle axis, and
    /// `retain_visible` already drops the vanished open rows when the next result publishes.
    /// Clearing here would discard a user's selection of CLOSED rows for nothing.
    ///
    /// Args:
    ///     on: Whether the next query includes still-running positions.
    ///     cx: Panel context used to persist and request the query.
    ///
    /// Returns:
    ///     Nothing; re-selecting the current value is a no-op.
    pub(super) fn set_show_open(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.show_open != on {
            self.show_open = on;
            self.persist_filters(None, cx);
            self.request_requery(cx);
        }
    }

    /// Apply one table-row click to the controlled Report selection.
    ///
    /// Args:
    ///     row: Current visible row index.
    ///     modifiers: Native modifier snapshot from the owning window.
    ///     cx: Panel context used to repaint the selection and its totals-row commands.
    ///
    /// Returns:
    ///     Nothing. Shift takes precedence over Ctrl/Command, and a plain click on the row that is
    ///     already the whole selection clears it — see [`ReportSelection::click`].
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

    /// Keep the double-clicked row selected.
    ///
    /// The table calls the row-select handler on both clicks of a double-click, so the second one
    /// hits the "click the sole selected row to deselect it" path and would leave a double-click
    /// with nothing selected. This runs after it, from the table's own double-click callback, and
    /// only for an unmodified double-click — the caller filters Shift and Ctrl out.
    ///
    /// Args:
    ///     row: Current visible row index.
    ///     cx: Panel context used to repaint the selection and its totals-row commands.
    pub(super) fn keep_report_row_selected(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(data) = self.data.data() else {
            return;
        };
        self.selection
            .select_only(data.row_keys.get(row).copied().flatten());
        cx.notify();
    }

    /// Select every stable row in the current filtered and sorted Report table.
    ///
    /// Args:
    ///     cx: Panel context used to read the event-time result and repaint the totals-row commands.
    ///
    /// Returns:
    ///     Nothing. The current table is capped by the report query's top-500 contract.
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

    /// Open the row's right-click menu at the cursor: the shared token menu, plus this trade's log.
    ///
    /// ONE menu for the whole row — every cell opens it, not just the coin. It is the same menu the
    /// Orders table, Assets and the chart's order lines use, so a token action reads the same
    /// everywhere; what only the Report can add rides at the end, behind its own separator.
    ///
    /// That trailing entry is the trade's core log. A row whose core recorded no task number cannot
    /// produce one — a log line carries no other identity — so it is shown DISABLED and says why,
    /// instead of opening a window that could only ever be empty.
    ///
    /// Args:
    ///     row: Index into the current snapshot's rows.
    ///     window: Window hosting the menu and the resulting dialog.
    ///     cx: Panel context used to read the row, the core, and the panel's core scope.
    ///
    /// Returns:
    ///     Nothing; a right-click landing past the last row opens nothing.
    pub(super) fn open_row_menu(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        let trade_log = vec![match self.trade_log_request(row, cx) {
            Ok(request) => {
                MoonMenuItem::with_key("rep-row-trade-log", t!("report.trade_log.open").to_string())
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        trade_log::open_trade_log(request.clone(), backend.clone(), window, app);
                    })
            }
            Err(reason) => {
                MoonMenuItem::with_key("rep-row-trade-log", t!(reason).to_string()).disabled(true)
            }
        }];
        // The DISCOVERABLE path to the trade window: a double-click is invisible, and it is also
        // the only place a "why not" can be shown at all, since a double-click has nowhere to put
        // one.
        let detail_view = cx.entity();
        let mut trade_log = trade_log;
        // Resolved HERE, not in the callback. A Report refresh republishes the rows while the menu
        // is open, so a retained row INDEX would let the action land on a different trade; the
        // resolved target is carried instead, exactly as the trade-log entry above carries its
        // request.
        trade_log.push(match self.trade_detail_target(row, cx) {
            Some(target) => {
                MoonMenuItem::with_key("rep-row-trade-window", super::trade_detail::menu_label())
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        let target = target.clone();
                        detail_view
                            .update(app, |this, cx| this.open_trade_detail_target(target, cx));
                    })
            }
            None => MoonMenuItem::with_key(
                "rep-row-trade-window",
                t!("trade_window.blocked.unresolved").to_string(),
            )
            .disabled(true),
        });
        // With no explicit filter the coin actions may act on every core the selector currently
        // knows, exactly as the coin cell's menu did.
        let selected_cores = self.effective_core_ids(self.backend.read(cx));
        let workspace_group = (!self.standalone).then(|| self.group.clone());
        let Some(data) = self.data.data().cloned() else {
            return;
        };
        let Some(values) = data.rows.get(row) else {
            return;
        };
        let ctx = columns::row_coin_menu_ctx(
            values,
            &self.cols,
            columns::ReportCoinMenuScope {
                target: columns::ReportCoinTarget {
                    core_uid: data.core_uids.get(row).copied().unwrap_or(0),
                    published_filter: data.filter.clone(),
                    focus_record_id: data.row_keys.get(row).and_then(|key| match key {
                        Some(selection::ReportRowKey::Replicated { rec_id, .. })
                        | Some(selection::ReportRowKey::Legacy { db_id: rec_id, .. }) => {
                            Some(*rec_id)
                        }
                        None => None,
                    }),
                },
                selected_cores,
                workspace_group,
            },
            trade_log,
            &self.backend,
            cx,
        );
        crate::controls::open_coin_menu(
            ctx,
            self.backend.clone(),
            window.mouse_position(),
            window,
            cx,
        );
    }

    /// Resolve one report row into everything the log scan needs, or say why it cannot be done.
    ///
    /// Args:
    ///     row: Index into the current snapshot's rows.
    ///     cx: Panel context used to read the row and the core's configured name.
    ///
    /// Returns:
    ///     The scan request, or the locale key naming the reason this row has no log to show — the
    ///     two reasons are different facts about the row and the menu says which one it is.
    fn trade_log_request(
        &self,
        row: usize,
        cx: &App,
    ) -> Result<trade_log::TradeLogRequest, &'static str> {
        let (data, values) = self
            .data
            .data()
            .and_then(|data| data.rows.get(row).map(|values| (data, values)))
            .ok_or("report.trade_log.no_task")?;
        let column = |name: &str| {
            self.cols
                .iter()
                .position(|col| col == name)
                .and_then(|ix| values.get(ix))
        };
        // A `taskid` stored as text is still a task number; `as_i64` deliberately leaves text alone
        // for the cells that must render it verbatim, so this call site parses it itself.
        let task_id = column("taskid")
            .and_then(|value| match value {
                Value::Text(text) => text.trim().parse().ok(),
                other => as_i64(other),
            })
            .unwrap_or(0);
        if task_id == 0 {
            return Err("report.trade_log.no_task");
        }
        let core_uid = data.core_uids.get(row).copied().unwrap_or(0);
        let (config_name, workspace) = {
            let backend = self.backend.read(cx);
            let config_name = backend
                .config
                .servers
                .iter()
                .find(|server| server.id == core_uid)
                .map(|server| server.name.clone());
            let workspace = if self.standalone {
                None
            } else {
                let revision = backend.workspace_revision();
                Some(trade_log::TradeLogWorkspaceIdentity::new(
                    self.group.clone(),
                    core_uid,
                    revision.read(cx).generation(),
                ))
            };
            (config_name, workspace)
        };
        // Report dates are unix seconds; the log files are named from milliseconds.
        let secs_to_ms = |v: Option<i64>| v.unwrap_or(0).saturating_mul(1000);
        // With no name for the core there is no file to open at all, and the dialog could only ever
        // report "nothing found" for a reason it does not know — so that is refused below, named.
        let mut request = trade_log::trade_log_request(
            &column("core_name").map(value_to_string).unwrap_or_default(),
            config_name.as_deref(),
            &column("coin").map(value_to_string).unwrap_or_default(),
            task_id,
            secs_to_ms(column("buydate").and_then(as_i64)),
            secs_to_ms(column("closedate").and_then(as_i64)),
            moon_core::util::now_unix_ms_i64(),
        );
        request.workspace = workspace;
        if request.labels.is_empty() {
            return Err("report.trade_log.no_core");
        }
        Ok(request)
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
        let hide_core_name = self.hide_core_name_column(self.backend.read(cx));
        let source_indices =
            columns::effective_visible_columns(&self.cols, &self.visible, hide_core_name)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
        let indices = selection::ordered_source_indices(
            &self.cols,
            &source_indices,
            &self.table_state.read(cx),
        );
        if self.selection.len() == 0 || indices.is_empty() {
            return;
        }
        let text = selection::selected_tsv(
            data,
            &self.cols,
            &indices,
            &self.selection,
            // Phase 1: replicated timestamps stay on the core's own clock, exactly as the grid
            // renders them, so a copied row and a read row cannot disagree.
            &moon_core::db::ReportAxis::identity_core_local(),
            self.display_zone,
        );
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(
            MoonNotification::success(
                t!("report.selection.copied", n = self.selection.len()).to_string(),
            ),
            cx,
        );
    }

    /// Queue soft-delete or restore after revalidating every selected row against live scope.
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
        let workspace_group = (!self.standalone).then(|| self.group.clone());
        let mut queued = 0usize;
        let mut failed = false;
        self.backend.update(cx, |backend, _| {
            if targets.iter().any(|(core_uid, _)| {
                !backend.workspace_action_allows_core(workspace_group.as_deref(), *core_uid)
            }) {
                failed = true;
                return;
            }
            for (core_uid, rec_ids) in targets {
                let count = rec_ids.len();
                if backend
                    .session
                    // Folded into ranges by the session: a large filtered selection can have the
                    // same mostly-consecutive id shape as a strategy purge.
                    .set_report_rows_deleted_ids(core_uid, deleted, rec_ids)
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
    ///
    /// # Returns
    ///
    /// Nothing; callbacks for a column unavailable in the current display context are ignored.
    pub(super) fn toggle_column(&mut self, name: String, cx: &mut Context<Self>) {
        let hide_core_name = self.hide_core_name_column(self.backend.read(cx));
        if !self
            .cols
            .iter()
            .any(|column| column == &name && columns::column_is_available(column, hide_core_name))
        {
            return;
        }
        if self.visible.contains(name.as_str()) {
            self.visible.remove(&name);
        } else {
            self.visible.insert(name);
        }
        if columns::effective_visible_columns(&self.cols, &self.visible, hide_core_name)
            .next()
            .is_none()
        {
            self.selection.clear();
        }
        self.persist_visible(cx);
        cx.notify();
    }

    /// Return export columns in runtime order for the selected visible-or-complete mode.
    ///
    /// Args:
    ///     all_cols: Whether to ignore the visible-column selection.
    ///     cx: Application context used to resolve the current workspace display lens.
    ///
    /// Returns:
    ///     Runtime column names in source order.
    fn export_columns(&self, all_cols: bool, cx: &App) -> Vec<String> {
        if all_cols {
            (*self.cols).clone()
        } else {
            let hide_core_name = self.hide_core_name_column(self.backend.read(cx));
            columns::effective_visible_columns(&self.cols, &self.visible, hide_core_name)
                .map(|(_, column)| column.clone())
                .collect()
        }
    }

    /// Capture the current effective export authority without applying workspace state to a
    /// standalone Analytics-owned Report.
    ///
    /// Args:
    ///     cx: Application context used to read Backend and workspace revision state.
    ///
    /// Returns:
    ///     Group generation plus effective ids, or explicit standalone ids without a generation.
    fn export_scope_identity(&self, cx: &App) -> ReportExportScopeIdentity {
        let (workspace_revision, core_ids) = {
            let backend = self.backend.read(cx);
            (
                (!self.standalone).then(|| backend.workspace_revision()),
                self.effective_core_ids(backend),
            )
        };
        ReportExportScopeIdentity {
            workspace_generation: workspace_revision.map(|revision| revision.read(cx).generation()),
            core_ids,
        }
    }

    /// Prompt for a destination, then export the current filter and sort order.
    ///
    /// `all_cols` selects the full runtime schema instead of visible columns. A
    /// `NotReady` or `Failed` read aborts before any file write; completion or
    /// failure is reported in the originating window. Group scope is captured before the picker
    /// and revalidated with a freshly rebuilt filter afterward; standalone explicit scope does not
    /// inherit workspace generation changes.
    ///
    /// Args:
    ///     fmt: CSV or XLSX destination format.
    ///     all_cols: Whether to export the complete runtime schema instead of visible columns.
    ///     window: Originating window for the path picker and completion notification.
    ///     cx: Panel context used to capture and later rebuild live export state.
    ///
    /// Returns:
    ///     Nothing; cancellation, released panels, empty columns, or stale scope write no file.
    pub(super) fn export_report(
        &mut self,
        fmt: export::Format,
        all_cols: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.export_columns(all_cols, cx).is_empty() {
            log::warn!("report export has no columns (table empty?)");
            return;
        }
        let requested_scope = self.export_scope_identity(cx);
        let suggested_filter = self.filter(cx);
        let suggested = export::suggested_name(
            &suggested_filter,
            fmt,
            all_cols,
            &moon_core::db::ReportAxis::identity_core_local(),
            self.display_zone,
        );
        let handle = window.window_handle();
        let rx = cx.prompt_for_new_path(&export::default_dir(), Some(&suggested));
        cx.spawn(async move |this, cx| {
            // A cancelled or closed destination dialog requires no notification.
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let Ok(Some((cols, filter, sort_key, sort_desc, zone))) = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    let current_scope = this.export_scope_identity(cx);
                    if !report_export_scope_is_current(&requested_scope, &current_scope) {
                        log::warn!(
                            "report export cancelled because its scope changed while choosing a destination"
                        );
                        return None;
                    }
                    let cols = this.export_columns(all_cols, cx);
                    if cols.is_empty() {
                        log::warn!("report export has no columns after destination selection");
                        return None;
                    }
                    Some((
                        cols,
                        this.filter(cx),
                        this.sort_key.clone(),
                        this.sort_desc,
                        this.display_zone,
                    ))
                })
            }) else {
                return;
            };
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    export::run(
                        &path,
                        fmt,
                        &cols,
                        &filter,
                        &sort_key,
                        sort_desc,
                        &moon_core::db::ReportAxis::identity_core_local(),
                        zone,
                    )
                        .map(|n| (n, path))
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

#[cfg(test)]
mod tests;
