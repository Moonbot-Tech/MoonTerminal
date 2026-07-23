//! Report panel dropdown filters and the visible-column selection menu.

use super::columns::header_for;
use super::*;
use rust_i18n::t;

impl ReportPanel {
    /// Render the shared multi-select core combo.
    ///
    /// An empty set means all cores; the label shows all, a sole name, or the selected count.
    pub(super) fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        // Rank the raw DB result at render time; the query has no config and may include
        // deleted cores with database-owned names.
        let cores = CoreOrder::new(&self.backend.read(cx).config).from_db(self.cores.clone());
        crate::controls::core_combo(
            cx,
            "rep-core",
            &cores,
            &self.sel_cores,
            t!("report.all_cores").to_string(),
            |n| t!("report.cores_n", n = n).to_string(),
            180.0,
            move |uid, app| {
                view.update(app, |t, c| t.toggle_core(uid, c));
            },
        )
    }

    /// Render the direction filter: all, Long, or Short.
    pub(super) fn side_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.side {
            SideFilter::All => t!("report.filter.all").to_string(),
            SideFilter::Long => t!("report.side.long").to_string(),
            SideFilter::Short => t!("report.side.short").to_string(),
        };
        let view = cx.entity();
        let items = crate::panels::radio_items(
            [
                (
                    SideFilter::All,
                    "rs-all".into(),
                    t!("report.filter.all").to_string().into(),
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
            crate::panels::RadioMark::Highlight,
            move |app, side| {
                view.update(app, |t, c| t.set_side(side, c));
            },
        );
        MoonDropdown::new("rep-side")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 69.0))
            .menu_width(design::font_w(cx, 120.0))
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Render the trade-kind filter: all, real, or emulated.
    pub(super) fn kind_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.kind {
            ReportKind::All => t!("report.kind.all"),
            ReportKind::Real => t!("report.kind.real"),
            ReportKind::Emu => t!("report.kind.emu"),
        };
        let view = cx.entity();
        let items = crate::panels::radio_items(
            [
                (
                    ReportKind::All,
                    "rk-all".into(),
                    t!("report.kind.all").to_string().into(),
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
                view.update(app, |t, c| t.set_kind(k, c));
            },
        );
        MoonDropdown::new("rep-kind")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 102.0))
            .menu_width(design::font_w(cx, 138.0))
            .menu_size(MoonMenuSize::Compact)
            .items(items)
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
            .label(format!("{} ▾", self.period.label()))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 100.0))
            .menu_width(design::font_w(cx, 130.0))
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Bare checkbox filtering soft-deleted trades: unchecked (the default) hides them,
    /// checked shows ONLY them.
    ///
    /// No label — the tooltip carries the explanation, matching the glyph buttons beside it.
    pub(super) fn deleted_check(&self, cx: &Context<Self>) -> impl IntoElement {
        crate::panels::icon_checkbox(
            "rep-deleted",
            t!("report.filter.deleted_tip").to_string(),
            self.deleted_only,
            cx.listener(|t, on: &bool, _w, cx| t.set_deleted_only(*on, cx)),
        )
    }

    /// Detached-window trash button that toggles deletion mode and doubles as its Save.
    ///
    /// A square icon button matching the main toolbar's window launchers (raw `ICON_BTN_W`, an SVG
    /// glyph, `ToolbarCompact`). Soft (inactive) → click enters the mode and shows the `deleted`
    /// checkbox column; while in the mode with no edits, a click leaves it; once any checkbox
    /// changed it turns amber and a click commits (see [`ReportPanel::on_delete_button`]).
    /// `selected` marks the active mode so it reads as pressed even before an edit arms the amber.
    pub(super) fn delete_mode_button(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let variant = if self.delete_dirty() {
            MoonButtonVariant::Amber
        } else {
            MoonButtonVariant::Soft
        };
        MoonButton::new("rep-delmode")
            // Icon-only width stays raw, like the toolbar launchers — one source in `controls::toolbar`.
            .width(crate::controls::toolbar::ICON_BTN_W)
            .variant(variant)
            .size(MoonButtonSize::ToolbarCompact)
            .selected(self.delete_mode)
            .leading_icon(MoonButtonIconSlot::new("icons/delete.svg").color(p.text_soft))
            .tooltip(t!("report.delete_mode_tip").to_string())
            .on_click(cx.listener(|t, _, window, c| t.on_delete_button(window, c)))
            .render()
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
                    .trigger_width(design::font_w(cx, 34.0))
                    .menu_width(design::font_w(cx, 200.0))
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
                    .trigger_width(design::font_w(cx, 34.0))
                    .menu_width(design::font_w(cx, 230.0))
                    .menu_max_height(420.0)
                    .menu_size(MoonMenuSize::Compact)
                    .close_on_select(false)
                    .items(items),
            )
    }
}
