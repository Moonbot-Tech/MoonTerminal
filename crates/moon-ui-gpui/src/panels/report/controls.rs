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
        let cores = self.cores.clone();
        let mut items = vec![
            MoonMenuItem::with_key("rc-all", t!("report.filter.all").to_string())
                .selected(self.sel_core == 0)
                .on_click({
                    let view = view.clone();
                    move |_, _, app| {
                        view.update(app, |t, c| t.set_core(0, c));
                    }
                }),
        ];
        for (i, (_u, name)) in cores.into_iter().enumerate() {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("rc-{i}"), name)
                    .selected(self.sel_core == i + 1)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.set_core(i + 1, c));
                    }),
            );
        }
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
        let opts = [
            (SideFilter::All, t!("report.filter.all").to_string()),
            (SideFilter::Long, t!("report.side.long").to_string()),
            (SideFilter::Short, t!("report.side.short").to_string()),
        ];
        MoonDropdown::new("rep-side")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(86.0)
            .menu_width(120.0)
            .menu_size(MoonMenuSize::Compact)
            .items(opts.into_iter().map(move |(side, label)| {
                let view = view.clone();
                MoonMenuItem::with_key(format!("rs-{label}"), label)
                    .selected(side == self.side)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.set_side(side, c));
                    })
            }))
    }

    /// Попап выбора видимых колонок (чекбоксы).
    pub(super) fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let visible = self.visible.clone();
        let items = db::DISPLAY_COLUMNS.iter().enumerate().map(move |(i, c)| {
            let on = visible.get(i).copied().unwrap_or(false);
            let view = view.clone();
            MoonMenuItem::with_key(format!("col-{i}"), header_for(c))
                .checked(on)
                .selected(on)
                .on_click(move |_, _, app| {
                    view.update(app, |t, c| t.toggle_column(i, c));
                })
        });
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
