//! [ReportPanel] table element, sort handling, and the panel and render trait impls.

use super::state::{ReportPreferenceWrite, schedule_report_preference};
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
        let view_menu = view.clone();
        let backend = self.backend.clone();
        let table_state = self.table_state.clone();
        let row_cols = self.cols.clone();
        let display_zone = self.display_zone;
        let axis = self.report_axis();
        self.natural_widths
            .refresh(&self.cols, &data.rows, vis, p, &axis, self.display_zone, cx);
        let cols = columns::report_columns(&self.cols, vis, &self.natural_widths.widths);
        let selection = self.selection.clone();
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
                    &backend,
                    &view_row,
                    selection.contains(data.row_keys.get(ri).copied().flatten()),
                    p,
                    &axis,
                    display_zone,
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
                view_double.update(app, |this, cx| {
                    // The selection quirk does not go away, so the compensation stays and the new
                    // action is ADDED to the gesture that already fires rather than replacing it.
                    this.keep_report_row_selected(row, cx);
                    this.open_trade_detail(row, cx);
                });
            })
            .on_select_all_rows(move |_window, app| {
                view_select_all.update(app, |this, cx| this.select_all_report_rows(cx));
            })
            // The coin cell keeps its own right-click menu and stops propagation; anywhere else on
            // the row opens the row menu.
            .on_right_click_row(move |row, window, app| {
                view_menu.update(app, |this, cx| this.open_row_menu(row, window, cx));
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
        self.preference_revisions.sort = self.preference_revisions.sort.wrapping_add(1);
        self.table_state.update(cx, |state, _| {
            state.set_sort(col.to_string(), !sort_desc);
        });
        let sort_key = self.sort_key.clone();
        schedule_report_preference(
            cx,
            ReportPreferenceWrite::Sort {
                key: sort_key,
                desc: sort_desc,
            },
        );
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
        cx: &mut Context<Self>,
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
                cx,
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
        self.sync_display_zone_fields(window, cx);
        // MoonUI renders only the selected tab or a visible tile, so reaching this method is the
        // visibility boundary. Hidden Report tabs retain their due edge, while a visible Report in
        // an unfocused OS window still catches up without waiting for a click.
        if self.generation_refresh.take_due() {
            self.needs_query = true;
        }
        self.schedule_requery(cx);
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
            // Always a query list: this field filters a report COLUMN, not a set of charts.
            crate::controls::coin_search::render_popup(
                "rep-coin-search",
                crate::controls::coin_search::CoinResults::Query(results),
                &HashSet::new(),
                false,
                None,
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

        // Each divider travels with the section AFTER it, so wrapping can never strand a rule at
        // the right edge. Sections stay atomic and move to the next line instead of introducing a
        // horizontal scrollbar or splitting controls that describe one filter concept.
        let separated = |section: Div| {
            h_flex()
                .flex_none()
                .items_center()
                .gap(design::ui_px(cx, design::CHROME_GAP))
                .child(design::chrome_divider(cx, p))
                .child(section)
        };
        let core_filters = design::chrome_section(cx).child(self.core_combo(cx));
        let strategy_filter = design::chrome_section(cx).child(self.strategy_combo(cx));
        let strategy_mask = self
            .strategy_name_mask_field(cx)
            .map(|field| design::chrome_section(cx).child(field));
        // No caption beside the coin field: its placeholder names it without consuming row width.
        let trade_filters = design::chrome_section(cx)
            .child(coin_field)
            .child(self.scope_control.clone());
        let period_filters = design::chrome_section(cx).child(self.period_combo(cx));
        // Manual bounds exist only in detached windows. Each caption stays attached to its field,
        // while From and To remain independent sections so a narrow host can wrap between them.
        let date_filters = self.detached.then(|| {
            let from = design::chrome_section(cx)
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("report.filter.from").to_string()),
                )
                .child(
                    div()
                        .w(px(date_range::field_width(cx)))
                        .whitespace_nowrap()
                        .child(
                            MoonDateTimePicker::new("rep-from", &self.from)
                                .placeholder(t!("report.filter.date_ph").to_string())
                                .cleanable(true)
                                .render(),
                        ),
                );
            let to = design::chrome_section(cx)
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("report.filter.to").to_string()),
                )
                .child(
                    div()
                        .w(px(date_range::field_width(cx)))
                        .whitespace_nowrap()
                        .child(
                            MoonDateTimePicker::new("rep-to", &self.to)
                                .placeholder(t!("report.filter.date_ph").to_string())
                                .cleanable(true)
                                .render(),
                        ),
                );
            [from, to]
        });
        let actions = design::chrome_section(cx)
            .child(self.export_menu(cx))
            .child(self.columns_menu(cx));
        let filters = h_flex()
            .w_full()
            .flex_wrap()
            .gap(design::ui_px(cx, design::CHROME_GAP))
            .items_center()
            .px_2()
            .py_1()
            .child(core_filters)
            .child(separated(strategy_filter))
            .children(strategy_mask.map(separated))
            .child(separated(trade_filters))
            .child(separated(period_filters))
            .children(date_filters.into_iter().flatten().map(separated))
            // Export and Columns retain the trailing edge without a flexible wrapping spacer.
            .child(separated(actions).ml_auto());

        // Apply the host's display lens without changing the user's persisted visibility set.
        let hide_core_name = self.hide_core_name_column(self.backend.read(cx));
        let vis = columns::effective_visible_columns(&self.cols, &self.visible, hide_core_name)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
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

        // Exact totals over the full filtered period. WHY the row is split into a never-yielding
        // head and one clipping tail is stated once, in `totals.rs`'s module doc; what only this
        // file knows is how that split is expressed in flex, below.
        // The row doubles as the selection bar: while rows are selected it carries their commands
        // and uses an accent tint to distinguish the active selection state.
        let selection_actions = self.selection_actions(p, cx);
        let facts = totals::footer_facts(
            self.data.data().map(Arc::as_ref),
            matches!(self.data, LoadState::Failed(_)),
            &self.valuation_status,
            moon_core::util::now_unix_ms_i64(),
        );
        let tip: SharedString = totals::footer_tooltip(&facts).into();
        let body = design::t_body(cx);
        // Every fact is `flex_none`, which is load-bearing in the tail alone: earlier facts retain
        // their intrinsic widths while overflow clips the tail at its right edge.
        let fact_el = move |index: usize, fact: totals::FooterFact| {
            let color = match fact.tone {
                totals::FactTone::Soft | totals::FactTone::Untrusted => p.text_soft,
                // Through the accessors: the light theme needs its darker tokens to stay legible,
                // and this match is the one place the footer picks a money colour.
                totals::FactTone::Positive => design::positive_color(p),
                totals::FactTone::Negative | totals::FactTone::Alarm => design::danger_color(p),
                totals::FactTone::Warn => p.orange,
            };
            let el = div()
                .id(index)
                .flex_none()
                .text_size(body)
                .text_color(rgb(color))
                // A theme-token border creates hierarchy without becoming localized/spoken text.
                .when(fact.section_start, |chip| {
                    chip.pl_2().border_l_1().border_color(rgb(p.border))
                })
                .when(fact.bold, |chip| chip.font_bold())
                .child(fact.text);
            match fact.tip {
                // A fact carrying diagnostics keeps its own tooltip; hovering it wins over the
                // group's, so the codes stay one hover away without crowding the visible row.
                Some(text) => el
                    .tooltip(crate::panels::common::text_tooltip(text))
                    .into_any_element(),
                None => el.into_any_element(),
            }
        };
        let fact_group = |id: &'static str, tip: SharedString, group: Vec<totals::FooterFact>| {
            h_flex()
                .id(id)
                .items_center()
                .gap_2()
                .tooltip(crate::panels::common::text_tooltip(tip))
                .children(
                    group
                        .into_iter()
                        .enumerate()
                        .map(|(index, fact)| fact_el(index, fact)),
                )
        };
        // Head and tail are DIRECT children of the row, never nested in a shared box. Nested, the
        // wrapper would take a zero flex basis and be allocated only the width the commands leave,
        // while its `flex_none` head kept painting at full width — straight over the command
        // buttons. As siblings the row lays them out sequentially, so the worst case is a clean
        // overflow off the panel's clipped edge rather than overlapping glyphs. The tooltip rides
        // the two groups, not the row, which also keeps it from rising over a command button.
        let essential = fact_group("rep-totals-head", tip.clone(), facts.essential).flex_none();
        // Zero flex basis: the tail grows into whatever the commands leave and contributes nothing
        // to shrink, so a shortfall lands on the commands.
        let clipping = fact_group("rep-totals-tail", tip.clone(), facts.tail)
            .flex_1()
            .min_w_0()
            .overflow_hidden();
        // Pinned right: the shown-rows tally describes the grid, not the money, so it sits at the
        // far edge. `ml_auto` inside the row, not inside the tail — nested in the clipping box it
        // would be pushed out of view instead of holding the edge.
        //
        // It yields that edge entirely while rows are selected: the selection commands take it,
        // and the width the tally gives back goes to the clipping tail, which is where the money
        // that a selection is about to act on lives. The fact still reaches the row tooltip.
        let shown = selection_actions.is_none().then(|| {
            fact_group("rep-totals-shown", tip, facts.trailing)
                .flex_none()
                .ml_auto()
        });
        let totals = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .when(selection_actions.is_some(), |row| {
                row.bg(rgba_from(p.accent, 0.08))
            })
            .child(essential)
            .child(clipping)
            .children(shown)
            // No commands in the no-data arms: a read that fails or has not landed clears the
            // selection first (`query.rs`, the `Err` arm), so this is `None` there.
            .children(selection_actions);

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
