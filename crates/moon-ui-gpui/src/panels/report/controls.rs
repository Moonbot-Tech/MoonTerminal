//! Поля-списки (ядро/сторона) и попап выбора видимых колонок панели «Отчёт».

use super::columns::header_for;
use super::*;
use rust_i18n::t;

impl ReportPanel {
    /// Комбобокс выбора ядра (Все + ядра из БД).
    pub(super) fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = if self.sel_core == 0 {
            t!("report.filter.all").to_string()
        } else {
            self.cores
                .get(self.sel_core - 1)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| t!("report.filter.all").to_string())
        };
        let view = cx.entity();
        let mut options: Vec<(usize, SharedString, SharedString)> = vec![(
            0,
            "rc-all".into(),
            t!("report.filter.all").to_string().into(),
        )];
        for (i, (_u, name)) in self.cores.iter().enumerate() {
            options.push((i + 1, format!("rc-{i}").into(), name.clone().into()));
        }
        let items = crate::panels::radio_items(
            options,
            self.sel_core,
            crate::panels::RadioMark::Highlight,
            move |app, idx| {
                view.update(app, |t, c| t.set_core(idx, c));
            },
        );
        MoonDropdown::new("rep-core")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(130.0)
            .menu_width(180.0)
            .menu_max_height(360.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Комбобокс стороны (Все/Лонг/Шорт).
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
            .trigger_width(86.0)
            .menu_width(120.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Попап выбора видимых колонок (чекбоксы) — по рантайм-списку колонок БД,
    /// поэтому авто-добавленные поля ядра сразу доступны к показу.
    pub(super) fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let items: Vec<MoonMenuItem> = self
            .table
            .cols
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let on = self.visible.contains(c.as_str());
                let name = c.clone();
                let view = view.clone();
                MoonMenuItem::with_key(format!("col-{i}"), header_for(c))
                    .checked(on)
                    .selected(on)
                    .on_click(move |_, _, app| {
                        let name = name.clone();
                        view.update(app, |t, c| t.toggle_column(name, c));
                    })
            })
            .collect();
        MoonDropdown::new("rep-cols")
            .label(format!("{} ▾", t!("report.columns_menu")))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(110.0)
            .menu_width(230.0)
            .menu_max_height(420.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .items(items)
    }
}
