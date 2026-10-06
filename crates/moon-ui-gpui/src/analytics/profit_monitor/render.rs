//! Profit Monitor render.

use super::*;

impl ProfitMonitorView {
    /// Render the period and grouping selectors.
    ///
    /// Args:
    ///     width: Current window width used to select the complete responsive presentation.
    ///     palette: Active MoonUI palette.
    ///     cx: Monitor render context.
    ///
    /// Returns:
    ///     One inline selector row or a narrow two-row control surface.
    pub(super) fn controls(
        &self,
        width: f32,
        palette: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = MonitorLayout::for_width(width, design::ui_value(cx, 1.0));
        let selected_group = self.group;
        let view = cx.entity();
        let groups = [GroupMode::Core, GroupMode::Exchange];
        let group_control = MoonSegmentedControl::new("profit-monitor-groups")
            .items(groups.map(|group| {
                // No tooltip: the segment already shows its own title in full.
                MoonSegmentItem::new("", group_title(group))
                    .fit_width(cx, 58.0, 110.0)
                    .selected(group == selected_group)
            }))
            .on_click(move |index, _, _, app| {
                let Some(group) = groups.get(index).copied() else {
                    return;
                };
                view.update(app, |this, cx| this.set_group(group, cx));
            })
            .render();
        let period = period_dropdown(self.period, cx.entity(), cx);
        let settings = self.settings_popover(settings_trigger(self.settings_open), palette, cx);
        let status_clock = h_flex()
            .flex_none()
            .gap(design::ui_px(cx, 10.0))
            .child(auto_status(
                self.db_active,
                self.refresh_error.is_some(),
                layout.status_label,
                palette,
                cx,
            ))
            .child(self.clock.clone())
            .child(settings);
        let content = if layout.inline_controls {
            h_flex()
                .justify_between()
                .gap(design::ui_px(cx, 10.0))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap(design::ui_px(cx, 8.0))
                        .child(period)
                        .child(group_control),
                )
                .child(status_clock)
        } else {
            v_flex()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap(design::ui_px(cx, 10.0))
                        .child(period)
                        .child(status_clock),
                )
                .child(h_flex().w_full().justify_center().child(group_control))
        };

        content
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .bg(moon(palette.shell_high))
            .border_b(px(1.0))
            .border_color(moon(palette.border))
            .into_any_element()
    }

    /// Size the profit column from this snapshot, never letting it shrink inside one period.
    ///
    /// Args:
    ///     entries: Display lines the table is about to draw.
    ///     total: The window fold drawn in the footer.
    ///     unit: Exact comparable unit shared by every value in the column.
    ///     layout: Responsive column selection the width has to be shared with.
    ///     width: Current window width.
    ///     scale: Active UI geometry scale, already resolved for `layout`.
    ///     cx: Application context used to measure glyph advances.
    ///
    /// Returns:
    ///     Chosen print form and the width the column claims this frame.
    pub(super) fn profit_column(
        &self,
        entries: &[MonitorEntry],
        total: &MonitorRow,
        unit: Option<ProfitUnit>,
        layout: MonitorLayout,
        width: f32,
        scale: f32,
        cx: &App,
    ) -> ProfitColumn {
        // The floor releases itself whenever the measurement it was taken under moved — another
        // currency, another window width, another font size — so the ratchet cannot outlive the
        // question it answered.
        let (column, floor) = table::profit_column(
            table::ColumnRequest {
                entries,
                total,
                unit,
                prefs: self.prefs,
                layout,
                width,
                scale,
                floor: self.profit_width.get(),
            },
            cx,
        );
        self.profit_width.set(floor);
        column
    }

    /// Release the profit column's ratchet, so the next snapshot sizes it from scratch.
    pub(super) fn release_profit_width(&self) {
        self.profit_width.set(ColumnFloor::default());
    }

    /// Render the current typed load state.
    ///
    /// Args:
    ///     width: Current window width used for deterministic column degradation.
    ///     palette: Active MoonUI palette.
    ///     view: Owning monitor entity receiving sortable-header actions.
    ///     cx: Application context used for rendering.
    ///
    /// Returns:
    ///     Table, split-currency warning, loading placeholder, or error state.
    pub(super) fn body(
        &self,
        width: f32,
        palette: MoonPalette,
        view: Entity<Self>,
        cx: &App,
    ) -> AnyElement {
        // Hoisted above the match so every arm draws the SAME marker, built from the same facts
        // the scoped query used. `ProfitLoadState::Split` is matched here BEFORE where the marker
        // used to be constructed inline (inside the `Ready` arm below) — giving only `Ready` a
        // marker would compile and render nothing for `Split` (§4.3's cross-model finding). The
        // Profit Monitor is `Singleton`: an Analytics surface that lists cores and folds them into
        // money, so it inherits the focused Auto workspace's preset like every other aggregate.
        // `self.live.preset` is `None`, and this marker stays empty, only while no group is
        // focused; once one is, a hidden core's exclusion shows here same as it does on Assets and
        // Core Status.
        let scope_marker = self.live.scope_marker();
        match &self.data {
            ProfitLoadState::Loading => {
                centered_message(t!("common.loading").to_string(), palette, cx)
            }
            ProfitLoadState::NotReady => {
                centered_message(t!("profit_monitor.not_ready").to_string(), palette, cx)
            }
            ProfitLoadState::Failed(error) => centered_alert(
                t!("profit_monitor.read_failed").to_string(),
                error.to_string(),
                cx,
            ),
            ProfitLoadState::Split(totals) => {
                let core_label = t!("profit_monitor.core_fallback").to_string();
                let unknown = t!("profit_monitor.currency_unknown").to_string();
                let ungrouped = t!("profit_monitor.group.ungrouped").to_string();
                let subtotal =
                    |name: &str| t!("profit_monitor.group.subtotal", name = name).to_string();
                let group_labels = SectionLabels {
                    ungrouped: &ungrouped,
                    subtotal: &subtotal,
                };
                let idle_label = t!("profit_monitor.idle_section").to_string();
                let entries = sections::currencies(
                    &self.currencies,
                    &self.live,
                    self.group,
                    self.sort,
                    RowLabels { core: &core_label },
                    &unknown,
                    sections::CurrencyOptions {
                        group_labels: self.prefs.group_sections.then_some(&group_labels),
                        include_idle: self.prefs.idle_cores,
                        idle_label: &idle_label,
                    },
                );
                let scale = design::ui_value(cx, 1.0);
                let layout = MonitorLayout::for_width(width, scale);
                let total = MonitorRow {
                    trades: totals.orders,
                    ..MonitorRow::default()
                };
                let column = self.profit_column(&entries, &total, None, layout, width, scale, cx);
                table::table(
                    entries,
                    total,
                    None,
                    Some(totals),
                    column,
                    layout,
                    self.sort,
                    self.prefs,
                    &self.flash,
                    self.backend.read(cx).core_filter(),
                    &self.scroll,
                    scope_marker,
                    &self.live.action_core_ids,
                    palette,
                    view,
                    self.backend.clone(),
                    cx,
                )
            }
            ProfitLoadState::Ready { unit, data } => {
                let core_label = t!("profit_monitor.core_fallback").to_string();
                let rows = grouped_rows(
                    data,
                    &self.live,
                    self.group,
                    self.prefs.idle_cores,
                    RowLabels { core: &core_label },
                );
                // Folded BEFORE the rows are split into groups, so the window's own total counts
                // every core exactly once. A core saved into two groups appears in both sections
                // on purpose; summing the sections instead would silently double its money here.
                let total = fold_total(&rows);
                let entries = if self.prefs.group_sections && self.group == GroupMode::Core {
                    let ungrouped = t!("profit_monitor.group.ungrouped").to_string();
                    sections::sectioned(
                        rows,
                        &self.live,
                        self.sort,
                        SectionLabels {
                            ungrouped: &ungrouped,
                            // Keep the user-authored name first: at the minimum width, ellipsis may
                            // hide the generic suffix but must not hide which group this closes.
                            subtotal: &|name| {
                                t!("profit_monitor.group.subtotal", name = name).to_string()
                            },
                        },
                    )
                } else {
                    sections::flat(rows, self.sort)
                };
                // Resolved once here because both the responsive tiers and the measured
                // column are stated against it.
                let scale = design::ui_value(cx, 1.0);
                let layout = MonitorLayout::for_width(width, scale);
                let column = self.profit_column(&entries, &total, *unit, layout, width, scale, cx);
                table::table(
                    entries,
                    total,
                    *unit,
                    None,
                    column,
                    layout,
                    self.sort,
                    self.prefs,
                    &self.flash,
                    self.backend.read(cx).core_filter(),
                    &self.scroll,
                    scope_marker,
                    &self.live.action_core_ids,
                    palette,
                    view,
                    self.backend.clone(),
                    cx,
                )
            }
        }
    }
}

impl Focusable for ProfitMonitorView {
    /// Return the monitor root focus handle.
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ProfitMonitorView {
    /// Render the complete independent monitor surface.
    ///
    /// Args:
    ///     window: Owning window used for responsive column selection.
    ///     cx: Monitor render context.
    ///
    /// Returns:
    ///     Window chrome, controls, and current report state.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::hotkeys::restore_root_focus(&self.focus, window, cx);
        let palette = MoonPalette::active(cx);
        let width = window_width(window);
        v_flex()
            .size_full()
            .relative()
            .bg(moon(palette.shell))
            .text_color(moon(palette.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .track_focus(&self.focus)
            .child(window_header(palette, cx))
            .child(self.controls(width, palette, cx))
            .child(
                AnyView::from(self.content.clone())
                    .cached(StyleRefinement::default().flex_1().min_h(px(0.0)).w_full()),
            )
            .child(
                MoonWindowFrame::tool("profit-monitor-window-hit", width)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(HEADER_HEIGHT)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}
