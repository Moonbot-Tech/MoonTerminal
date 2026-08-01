//! Report panel dropdown filters and the visible-column selection menu.

use super::columns::header_for;
use super::*;
use rust_i18n::t;

/// Compact word for the trade-kind filter inside the scope trigger.
///
/// Abbreviated ("реал." for "Реальные") so the two-part summary fits the field width.
fn kind_short(kind: ReportKind) -> String {
    match kind {
        ReportKind::All => t!("report.kind.all_short").to_string(),
        ReportKind::Real => t!("report.kind.real_short").to_string(),
        ReportKind::Emu => t!("report.kind.emu_short").to_string(),
    }
}

impl ReportPanel {
    /// Render the shared multi-select core combo.
    ///
    /// An empty set means all cores; every partial selection shows the selected count. Clicking a
    /// known exchange header batch-toggles its currently available database cores.
    ///
    /// Args:
    ///     cx: Panel context used to order database cores, read exchanges, and wire callbacks.
    ///
    /// Returns:
    ///     The configured fixed-trigger dropdown.
    pub(super) fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let exchange_view = view.clone();
        // Rank the raw DB result at render time; the query has no config and may include
        // deleted cores with database-owned names.
        let (cores, exchange_names) = {
            let backend = self.backend.read(cx);
            (
                CoreOrder::new(&backend.config).from_db(self.cores.clone()),
                backend.session.market_source().core_exchange_names(),
            )
        };
        crate::controls::core_combo(
            "rep-core",
            &cores,
            &exchange_names,
            &self.sel_cores,
            t!("report.all_cores").to_string(),
            |n| t!("report.cores_n", n = n).to_string(),
            180.0,
            move |uid, app| {
                view.update(app, |t, c| t.toggle_core(uid, c));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        )
    }

    /// Render the searchable, virtualized strategy selector grouped by core.
    ///
    /// The trigger and popup are asked to look like the `MoonDropdown` filters beside them:
    /// `trigger_variant`/`trigger_size` give it the Soft button's fill, border, hover ramp and
    /// geometry, and `menu_chrome` paints the popup on the menu surface with the check mark in the
    /// leading column. Only the mono label stays a call-site choice, matching this row's dropdowns.
    ///
    /// Args:
    ///     cx: Panel context used for responsive trigger and menu sizing.
    ///
    /// Returns:
    ///     A MoonUI combobox that renders only visible core and strategy rows.
    pub(super) fn strategy_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let (summary, _) = strategy_selection_summary(
            &self.available_strategy_keys,
            self.selected_strategies.as_ref(),
            &t!("report.all_strategies"),
            |n| t!("report.strategies_n", n = n).to_string(),
        );
        let palette = MoonPalette::active(cx);
        div()
            .w(design::font_w_px(cx, crate::controls::CORE_COMBO_TRIGGER_W))
            .child(
                MoonCombobox::new(&self.strategy_select)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .menu_chrome(MoonComboboxMenuChrome::Menu)
                    .font_family(design::mono())
                    .placeholder(t!("report.all_strategies").to_string())
                    .cleanable(false)
                    .search_placeholder(t!("report.search_strategies").to_string())
                    .appearance(true)
                    .menu_width(design::font_w_px(cx, 380.0))
                    .menu_max_h(design::ui_px(cx, 420.0))
                    .render_trigger(move |_, _, _| {
                        // Centred label plus caret, the way a MoonDropdown button draws its
                        // trigger: pushed apart by `justify_between` the same text reads as
                        // left-aligned next to those buttons.
                        h_flex()
                            .w_full()
                            .justify_center()
                            .gap_1()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .truncate()
                                    .child(summary.clone()),
                            )
                            // A custom MoonCombobox trigger suppresses its built-in trailing icon.
                            .child(div().text_color(rgb(palette.text_muted)).child("▾"))
                    }),
            )
    }

    /// Render the row-scope filter: direction, trade kind and the deleted-only switch in ONE field.
    ///
    /// The three used to be two dropdowns and a bare checkbox. They are independent filters, but
    /// they are always read together as "which rows am I looking at", so they share one field split
    /// by separators — the way the Orders settings menu groups its own options. The trigger
    /// summarises the state in short words (`Все/реал.`), adding the deleted segment only while
    /// that off-by-default filter is on.
    ///
    /// Width is the old kind dropdown's 102 as a MINIMUM, not a fixed size: a fixed 102 fits ~12
    /// mono characters, so the three-part deleted-only label would be ellipsised in every locale —
    /// hiding the one filter that has no other indicator left in the toolbar. `fit_trigger_width`
    /// keeps the field at 102 for the everyday two-part label and lets it grow for the rare one.
    pub(super) fn scope_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        // With both lists on "all" the pair would read as the same word twice ("Все/все"); one word
        // says exactly as much.
        // Built by appending onto the first word: this runs on every render of the panel, so the
        // segments are pushed in place instead of allocating a fresh String per join.
        let mut label =
            if matches!(self.side, SideFilter::All) && matches!(self.kind, ReportKind::All) {
                t!("report.filter.all").to_string()
            } else {
                let mut label = crate::panels::side_label(self.side);
                label.push('/');
                label.push_str(&kind_short(self.kind));
                label
            };
        if self.deleted_only {
            label.push('/');
            label.push_str(&t!("report.filter.deleted_short"));
        }
        // Each group labels its "all" row distinctly: two bare "Все" rows in one menu would read as
        // the same option twice.
        let side_view = cx.entity();
        let mut items = crate::panels::radio_items(
            [
                (
                    SideFilter::All,
                    "rs-all".into(),
                    t!("report.filter.all_sides").to_string().into(),
                ),
                (
                    SideFilter::Long,
                    "rs-long".into(),
                    t!("report.side.long").to_string().into(),
                ),
                (
                    SideFilter::Short,
                    "rs-short".into(),
                    t!("report.side.short").to_string().into(),
                ),
            ],
            self.side,
            crate::panels::RadioMark::Check,
            move |app, side| {
                side_view.update(app, |t, c| t.set_side(side, c));
            },
        );
        items.push(MoonMenuItem::separator());
        let kind_view = cx.entity();
        items.extend(crate::panels::radio_items(
            [
                (
                    ReportKind::All,
                    "rk-all".into(),
                    t!("report.kind.all_kinds").to_string().into(),
                ),
                (
                    ReportKind::Real,
                    "rk-real".into(),
                    t!("report.kind.real").to_string().into(),
                ),
                (
                    ReportKind::Emu,
                    "rk-emu".into(),
                    t!("report.kind.emu").to_string().into(),
                ),
            ],
            self.kind,
            crate::panels::RadioMark::Check,
            move |app, k| {
                kind_view.update(app, |t, c| t.set_kind(k, c));
            },
        ));
        // Soft-deleted trades: off (the default) hides them, on shows ONLY them. The toggle reads
        // the live flag instead of a render-time copy, so it cannot invert a value the user changed
        // from elsewhere between this render and the click.
        items.push(MoonMenuItem::separator());
        let deleted_view = cx.entity();
        items.push(
            MoonMenuItem::with_key("rd-deleted", t!("report.filter.deleted").to_string())
                .checked(self.deleted_only)
                .on_click(move |_, _, app| {
                    deleted_view.update(app, |t, c| t.set_deleted_only(!t.deleted_only, c));
                }),
        );
        // The tooltip carries what the row labels cannot: that the deleted switch is exclusive
        // (off hides deleted trades, on shows ONLY them) rather than an "also show" checkbox.
        div()
            .id("rep-scope-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("report.filter.scope_tip").to_string()))
                    .into()
            })
            .child(
                MoonDropdown::new("rep-scope")
                    .label(label)
                    .trigger_caret(true)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .fit_trigger_width(102.0, 170.0)
                    .menu_width_scaled(165.0)
                    .menu_size(MoonMenuSize::Compact)
                    // Three independent filters in one field: keep the menu open so setting two of
                    // them does not cost two open/close cycles.
                    .close_on_select(false)
                    .items(items),
            )
    }

    /// Render the report-period filter using Moonbot-compatible presets.
    pub(super) fn period_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let options: Vec<(Period, SharedString, SharedString)> = Period::ALL
            .iter()
            .enumerate()
            .map(|(i, p)| (*p, format!("rp-{i}").into(), p.label().into()))
            .collect();
        let items = crate::panels::radio_items(
            options,
            self.period,
            crate::panels::RadioMark::Highlight,
            move |app, p| {
                view.update(app, |t, c| t.set_period(p, c));
            },
        );
        MoonDropdown::new("rep-period")
            .label(self.period.label())
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(100.0)
            .menu_width_scaled(130.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Build the responsive bottom action bar for the current row selection.
    ///
    /// Args:
    ///     palette: Active Moon palette used for the contained selection surface.
    ///     cx: Panel context used to wire actions and count selected replicated targets.
    ///
    /// Returns:
    ///     A wrapping action row suitable for narrow dock and wide standalone hosts.
    pub(super) fn selection_bar(&self, palette: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let selected = self.selection.len();
        let mutable = self.selection.mutable_count();
        let mut bar = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgba_from(palette.accent, 0.08))
            .child(
                div()
                    .flex_1()
                    .min_w(design::ui_px(cx, 130.0))
                    .text_size(design::t_body(cx))
                    .font_bold()
                    .text_color(rgb(palette.text))
                    .child(t!("report.selection.count", n = selected).to_string()),
            )
            .child(
                MoonButton::new("report-selection-clear")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .label(t!("report.selection.clear").to_string())
                    .leading_icon(MoonButtonIconSlot::new("icons/close.svg"))
                    .on_click(cx.listener(|this, _, _, cx| this.clear_report_selection(cx)))
                    .render(),
            )
            .child(
                MoonButton::new("report-selection-copy")
                    .size(MoonButtonSize::Micro)
                    .outline()
                    .label(t!("report.selection.copy").to_string())
                    .leading_icon(MoonButtonIconSlot::new("icons/copy.svg"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.copy_report_selection(window, cx)),
                    )
                    .render(),
            );
        if mutable > 0 {
            let (label, icon) = if self.deleted_only {
                (
                    t!("report.selection.restore", n = mutable).to_string(),
                    "icons/undo-2.svg",
                )
            } else {
                (
                    t!("report.selection.delete", n = mutable).to_string(),
                    "icons/delete.svg",
                )
            };
            let mutation = MoonButton::new("report-selection-mutate")
                .size(MoonButtonSize::Micro)
                .label(label)
                .leading_icon(MoonButtonIconSlot::new(icon))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.mutate_report_selection(!this.deleted_only, window, cx)
                }));
            bar = bar.child(if self.deleted_only {
                mutation.outline().render()
            } else {
                mutation.danger().render()
            });
        }
        bar.into_any_element()
    }

    /// Build the CSV/XLSX export menu for the visible or full schema.
    ///
    /// Export uses the panel's current filter and sort order; the period may be a preset or
    /// dates entered in the From/To MoonInput fields and parsed with `db::parse_ymd`.
    pub(super) fn export_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let item =
            |key: &'static str, label: String, fmt: super::export::Format, all_cols: bool| {
                let view = view.clone();
                MoonMenuItem::with_key(key, label).on_click(move |_, window, app| {
                    view.update(app, |t, c| t.export_report(fmt, all_cols, window, c));
                })
            };
        let items = vec![
            item(
                "exp-csv",
                t!("report.export.csv").to_string(),
                super::export::Format::Csv,
                false,
            ),
            item(
                "exp-csv-all",
                t!("report.export.csv_all").to_string(),
                super::export::Format::Csv,
                true,
            ),
            item(
                "exp-xlsx",
                t!("report.export.xlsx").to_string(),
                super::export::Format::Xlsx,
                false,
            ),
            item(
                "exp-xlsx-all",
                t!("report.export.xlsx_all").to_string(),
                super::export::Format::Xlsx,
                true,
            ),
        ];
        // Keep the glyph button alongside the column selector and explain it with a tooltip.
        div()
            .id("rep-export-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("report.export_menu").to_string()))
                    .into()
            })
            .child(
                MoonDropdown::new("rep-export")
                    .segment(moon_ui::MoonButtonSegment::new("⇩"))
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width(design::glyph_btn_w(cx))
                    .menu_width_scaled(200.0)
                    .menu_size(MoonMenuSize::Compact)
                    .items(items),
            )
    }

    /// Build a checkbox menu from the runtime DB schema, including new dynamic core fields.
    pub(super) fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let all_on = self.all_columns_on();
        let all_view = view.clone();
        let mut items: Vec<MoonMenuItem> = vec![
            // "All" enables every column; when all are already enabled, this action keeps only
            // the first so it never empties the table. Use `checked`, not `selected`: selected
            // adds a light background that makes rows hard to read in the light theme, while
            // the check glyph is an explicit selection indicator.
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(all_on)
                .on_click(move |_, _, app| {
                    all_view.update(app, |t, c| t.toggle_all_columns(c));
                }),
        ];
        items.extend(self.cols.iter().enumerate().map(|(i, c)| {
            let on = self.visible.contains(c.as_str());
            let name = c.clone();
            let view = view.clone();
            MoonMenuItem::with_key(format!("col-{i}"), header_for(c))
                .checked(on)
                .on_click(move |_, _, app| {
                    let name = name.clone();
                    view.update(app, |t, c| t.toggle_column(name, c));
                })
        }));
        // Use a glyph button instead of a list field, matching other column selectors. The
        // tooltip is localized; glyphs remain outside the locale dictionary per locales/README.
        div()
            .id("rep-cols-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("report.columns_menu").to_string()))
                    .into()
            })
            .child(
                MoonDropdown::new("rep-cols")
                    .segment(moon_ui::MoonButtonSegment::new("▦"))
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width(design::glyph_btn_w(cx))
                    .menu_width_scaled(230.0)
                    .menu_max_height_ui(420.0)
                    .menu_size(MoonMenuSize::Compact)
                    .close_on_select(false)
                    .items(items),
            )
    }
}
