//! [ReportPanel] table element, sort handling, and the panel and render trait impls.

use super::*;

impl ReportPanel {
    /// Render the report table from currently renderable data.
    ///
    /// `vis` indexes [`Self::cols`]. Empty rows keep the header and show the
    /// panel overlay. Loading with stale data may reach this function; loading
    /// without data, `NotReady`, and `Failed` are replaced by `note_el`.
    ///
    /// # Arguments
    ///
    /// * `data` - Current report snapshot and stable row identities.
    /// * `vis` - Source-column indices selected for display.
    /// * `p` - Active MoonUI palette.
    /// * `cx` - Panel context used by row and sort callbacks.
    ///
    /// # Returns
    ///
    /// The table element with caller-controlled multi-row selection.
    pub(super) fn table_el(
        &mut self,
        data: Arc<ReportData>,
        vis: &[usize],
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible = Rc::new(vis.to_vec());
        let row_count = data.rows.len();
        let view = cx.entity();
        let view_sort = view.clone();
        let view_row = view.clone();
        let view_click = view.clone();
        let view_double = view.clone();
        let view_select_all = view.clone();
        let backend = self.backend.clone();
        let table_state = self.table_state.clone();
        let row_cols = self.cols.clone();
        self.natural_widths
            .refresh(&self.cols, &data.rows, vis, p, cx);
        let cols = columns::report_columns(&self.cols, vis, &self.natural_widths.widths);
        let selection = self.selection.clone();
        // With no explicit filter, the coin menu may act on every core currently
        // known to the report selector.
        let selected_cores: Rc<Vec<u64>> = Rc::new(if self.sel_cores.is_empty() {
            self.cores.iter().map(|(u, _)| *u).collect()
        } else {
            self.sel_cores.iter().copied().collect()
        });
        // Clip resized columns at the shared table host; an empty successful
        // result keeps the header and uses the panel overlay.
        crate::panels::common::data_table_host(
            "rep-table-host",
            row_count == 0,
            t!("report.empty").to_string(),
            p,
            cx,
            MoonDataTable::new("report-table", row_count, move |ri, _window, _app| {
                columns::report_data_row(
                    ri,
                    &row_cols,
                    &data,
                    &visible,
                    &selected_cores,
                    &backend,
                    &view_row,
                    selection.contains(data.row_keys.get(ri).copied().flatten()),
                    p,
                )
            })
            .state(&table_state)
            .columns(cols)
            .width_policy(MoonDataTableWidthPolicy::Preserve)
            .horizontal_scrollbar_visibility(MoonScrollbarVisibility::Always)
            .controlled_row_selection(true)
            .header_height(design::TABLE_HEAD_H)
            .row_height(design::TABLE_ROW_H)
            .on_select_row(move |row, window, app| {
                let modifiers = window.modifiers();
                view_click.update(app, |this, cx| this.select_report_row(row, modifiers, cx));
            })
            .on_double_click_row(move |row, window, app| {
                // Only a PLAIN double-click needs the compensation. With Shift or Ctrl held the two
                // clicks are a range or toggle gesture whose result must stand — forcing the row to
                // be the whole selection there would silently collapse a range the user built.
                let modifiers = window.modifiers();
                if modifiers.shift || modifiers.secondary() {
                    return;
                }
                view_double.update(app, |this, cx| this.keep_report_row_selected(row, cx));
            })
            .on_select_all_rows(move |_window, app| {
                view_select_all.update(app, |this, cx| this.select_all_report_rows(cx));
            })
            .on_sort(move |key, ascending, _window, app| {
                let key = key.to_string();
                view_sort.update(app, |t, cx| t.set_report_sort(&key, !ascending, cx));
            }),
        )
        .into_any_element()
    }

    pub(super) fn set_report_sort(&mut self, col: &str, sort_desc: bool, cx: &mut Context<Self>) {
        if self.sort_key == col && self.sort_desc == sort_desc {
            return;
        }
        self.sort_key = col.to_string();
        self.sort_desc = sort_desc;
        self.table_state.update(cx, |state, _| {
            state.set_sort(col.to_string(), !sort_desc);
        });
        if let Some(conn) = &self.conn {
            db::save_sort(conn, &self.sort_key, self.sort_desc);
        }
        self.request_requery(cx);
    }
}

impl EventEmitter<PanelEvent> for ReportPanel {}
impl Focusable for ReportPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ReportPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn panel_name(&self) -> &'static str {
        "Report"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("Report", &self.group)
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
        Some(vec![
            crate::persistence::table_persist::reset_button(
                "report-reset-widths",
                &self.table_state,
            ),
            crate::panels::detach_button(
                "Report",
                self.group.clone(),
                self.backend.clone(),
                self.dock.clone(),
            ),
        ])
    }
}

impl Render for ReportPanel {
    /// Render the Report controls, table, totals, and optional standalone window chrome.
    ///
    /// Args:
    ///     window: Owning window used to flush retained strategy-combobox updates.
    ///     cx: Panel render context.
    ///
    /// Returns:
    ///     The complete Report surface.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.flush_strategy_select_sync(window, cx);
        let p = MoonPalette::active(cx);
        let border = rgb(p.border);

        // Coin field using the shared chart-style search widget, such as `BTC - Bybit1`.
        // Unlike chart search, selecting a result does not open that market. It inserts the base
        // token into the textual report filter, backed by SQL `coin LIKE`, and requests a refresh.
        // Partial manual input still works; the popup is only a suggestion layer.
        let coin_popup = self.coin_popup_open.then(|| {
            let results = {
                let b = self.backend.read(cx);
                crate::controls::coin_search::search(
                    b,
                    &self.group,
                    None,
                    &self.coin.read(cx).value(),
                )
            };
            let view = cx.entity();
            let coin_input = self.coin.clone();
            let backend_pick = self.backend.clone();
            crate::controls::coin_search::render_popup(
                "rep-coin-search",
                results,
                &HashSet::new(),
                false,
                p,
                cx,
                move |core, market, window, app| {
                    // The filter is textual and runs as SQL `coin LIKE` against the token the CORE
                    // wrote into the report, so the coin has to be the core's own spelling —
                    // `SOL_RP`, not a reading of the market name. Core is selected separately.
                    // Update the mirror before `set_value` so the resulting Change event does not
                    // reopen the popup.
                    // The DISPLAY coin, without a contract tail: the filter is a substring match,
                    // so `SOL` still finds the core's `SOL_RP` and `SOL_0925` rows while the
                    // qualified token would exclude every other expiry — and `_` is a
                    // single-character wildcard in the `coin LIKE` this feeds.
                    let base = backend_pick
                        .read(app)
                        .session
                        .market_source()
                        .market_label(core, &market)
                        .display_coin()
                        .to_string();
                    view.update(app, |this, cx| {
                        this.coin_query = base.clone();
                        this.coin_popup_open = false;
                        cx.notify();
                    });
                    coin_input.update(app, |inp, c| inp.set_value(base.clone(), window, c));
                    view.update(app, |this, cx| this.request_requery(cx));
                },
                |_core, _market, _app| {},
                |_app| {},
            )
            .absolute()
            .top_full()
            .left_0()
            .mt(px(2.0))
        });
        // Catch clicks outside the popup to dismiss it. This layer sits below the filters in z-order,
        // so result rows handle their own clicks while clicks elsewhere close the list.
        let coin_dismiss = self.coin_popup_open.then(|| {
            div()
                .id("rep-coin-dismiss")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                        this.close_coin_popup(cx);
                        cx.stop_propagation();
                    }),
                )
        });
        let coin_field = div()
            .relative()
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("rep-coin")
                        .state(&self.coin)
                        .small()
                        .cleanable(true),
                ),
            )
            .children(coin_popup);

        // Filters.
        let filters = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.core_combo(cx))
            .child(self.strategy_combo(cx))
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.filter.coin").to_string()),
            )
            .child(coin_field)
            .child(self.scope_combo(cx))
            .child(self.period_combo(cx))
            // Show manual From/To dates only in detached windows. Docked tabs rely on period presets
            // to keep the core, coin, scope (side/kind/deleted), period and column controls compact.
            .when(self.detached, |f| {
                f.child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("report.filter.from").to_string()),
                )
                .child(
                    div()
                        .w(px(110.0))
                        .child(MoonInput::new("rep-from").state(&self.from).small()),
                )
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("report.filter.to").to_string()),
                )
                .child(
                    div()
                        .w(px(110.0))
                        .child(MoonInput::new("rep-to").state(&self.to).small()),
                )
            })
            // Export and the field selector sit at the RIGHT edge, away from the filters. The row
            // wraps, so they ride in their own `ml_auto` group instead of behind a `flex_1` spacer:
            // a flexible spacer inside a wrapping row would claim a whole line of its own once the
            // filters wrap, pushing both buttons onto a second row.
            .child(
                h_flex()
                    .ml_auto()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(self.export_menu(cx))
                    .child(self.columns_menu(cx)),
            );

        // Table columns follow the user's persisted visibility set in every host.
        let vis: Vec<usize> = self
            .cols
            .iter()
            .enumerate()
            .filter(|(_, c)| self.visible.contains(c.as_str()))
            .map(|(i, _)| i)
            .collect();
        // Resolve `LoadState` before inspecting schema or visibility. A failed
        // or not-ready read can coexist with an empty cached schema and must show
        // its database note, not the user-preference message for hidden columns.
        // Loaded empty rows keep the table's header and overlay, so this view
        // deliberately never classifies `Note::Empty`.
        let table_el: AnyElement = match self.data.view(|_| false) {
            Err(note) => note_el("rep-table-note", note, 12.0, p, cx),
            // A successful read with no report columns means the core schema is
            // not available yet.
            Ok(_) if self.cols.is_empty() => note_el("rep-table-note", Note::NotReady, 12.0, p, cx),
            // Columns exist, so an empty visible set is a user preference.
            Ok(_) if vis.is_empty() => div()
                .p_3()
                .text_color(rgb(p.text_soft))
                .child(t!("report.all_cols_hidden").to_string())
                .into_any_element(),
            Ok(data) => self.table_el(data.clone(), &vis, p, cx),
        };

        // Exact totals over the full filtered period.
        // Without current data, never render +0.00 / 0 orders: those values are
        // indistinguishable from a genuinely empty period.
        // The totals line doubles as the selection bar: while rows are selected it carries their
        // commands on the right and takes the same accent tint the separate bar used to have.
        // Wrapping keeps it usable in a narrow dock, where totals plus commands outgrow one line.
        let selection_actions = self.selection_actions(p, cx);
        let mut totals = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .when(selection_actions.is_some(), |row| {
                row.bg(rgba_from(p.accent, 0.08))
            })
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.totals").to_string()),
            );
        totals = match self.data.data() {
            Some(d) => {
                for total in &d.totals.totals {
                    let sum_col = if total.profit > 0.0 {
                        p.green
                    } else if total.profit < 0.0 {
                        p.red
                    } else {
                        p.text_soft
                    };
                    totals = totals.child(
                        div()
                            .font_bold()
                            .text_color(rgb(sum_col))
                            .child(report_quote_total_text(*total)),
                    );
                }
                if d.totals.unknown_orders > 0 {
                    totals = totals.child(div().font_bold().text_color(rgb(p.orange)).child(
                        t!("report.unknown_quote_orders", n = d.totals.unknown_orders).to_string(),
                    ));
                }
                totals
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_soft))
                            .child(t!("report.orders_count", count = d.totals.orders).to_string()),
                    )
                    // The shown-rows note yields its right slot to the selection commands, which
                    // ride their own `ml_auto` group instead: a flexible spacer would claim a whole
                    // line of the wrapping row once the commands wrap onto one.
                    .when(selection_actions.is_none(), |row| {
                        row.child(
                            div()
                                .flex_1()
                                .flex()
                                .justify_end()
                                .text_size(design::t_body(cx))
                                .text_color(rgb(p.text_soft))
                                .child(t!("report.shown_top", n = d.rows.len()).to_string()),
                        )
                    })
                    .children(selection_actions)
            }
            None => {
                let failed = matches!(self.data, LoadState::Failed(_));
                let (text, color) = if failed {
                    (t!("common.db_read_failed_short").to_string(), p.orange)
                } else {
                    ("—".to_string(), p.text_soft)
                };
                // No commands here: a read that fails or has not landed clears the selection first
                // (`query.rs`, the `Err` arm), so `selection_actions` is always `None` in this arm.
                totals.child(div().font_bold().text_color(rgb(color)).child(text))
            }
        };

        let root = v_flex()
            .id("report-panel")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            // Set monospace on the panel root, as in Orders, Assets, Analytics, and Screener.
            // Detached panels do not inherit it from the shell header; without this, Report would
            // render in Inter and diverge from the docked view and monospace-based core-selector
            // width measurement.
            .font_family(design::mono())
            .bg(rgb(p.table_body))
            .when(self.standalone, |this| {
                this.child(window::report_header(
                    p,
                    &self.table_state,
                    window::report_title(&self.sel_cores, &self.cores),
                    cx,
                ))
            })
            // Keep the popup dismiss layer below the filters in z-order; see `coin_dismiss`.
            .children(coin_dismiss)
            .child(filters)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(table_el)
            // The comment of the current row, directly above the totals: full width, so a long
            // comment wraps to as many lines as it needs instead of being truncated in its cell.
            // It sits ABOVE the rule that closes the table, not below it, so showing it adds
            // exactly its own height — the pane sizes itself in whole table rows, and a stray
            // pixel of rule would put the table's unused tail back out of step.
            // Only where a table exists to click: on a failed or not-ready read, or with every
            // column hidden, the panel already carries its own note and the pane would be a strip
            // that can never fill. A core whose schema has no `comment` column gets no pane at all
            // rather than one that can only ever say "no comment".
            .when(
                self.show_comment
                    && self.selection.current().is_some()
                    && self.data.data().is_some()
                    && !vis.is_empty()
                    && self.cols.iter().any(|column| column == "comment"),
                |this| this.child(self.comment_pane(p, cx)),
            )
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(totals);
        if self.standalone {
            root.child(
                MoonWindowFrame::tool("report-window-frame-hit", 0.0)
                    .header_height(window::REPORT_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
        } else {
            root
        }
    }
}

/// Format one Report footer total with precision appropriate to its quote currency.
///
/// Args:
///     total: Known-currency aggregate to display.
///
/// Returns:
///     Signed compact amount followed by the exact quote ticker.
fn report_quote_total_text(total: db::QuoteTotal) -> String {
    let amount =
        moon_core::util::fmt::compact(total.profit.abs(), total.currency.display_decimals());
    let sign = if total.profit >= 0.0 { "+" } else { "-" };
    format!("{sign}{amount} {}", total.currency.ticker())
}
