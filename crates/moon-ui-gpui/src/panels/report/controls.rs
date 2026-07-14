//! Поля-списки (ядро/сторона) и попап выбора видимых колонок панели «Отчёт».

use super::columns::header_for;
use super::*;
use rust_i18n::t;

impl ReportPanel {
    /// Комбобокс ядер — МУЛЬТИВЫБОР (чекбоксы, меню не закрывается на клик, как выбор
    /// колонок). Подпись: «Все» (пусто) / имя единственного / «Ядер: N».
    pub(super) fn core_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let all_selected = !self.cores.is_empty() && self.sel_cores.len() == self.cores.len();
        let cur = match self.sel_cores.len() {
            0 => t!("report.filter.all").to_string(),
            _ if all_selected => t!("report.filter.all").to_string(),
            1 => {
                let uid = *self.sel_cores.iter().next().unwrap();
                self.cores
                    .iter()
                    .find(|(u, _)| *u == uid)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| t!("report.cores_n", n = 1).to_string())
            }
            n => t!("report.cores_n", n = n).to_string(),
        };
        let view = cx.entity();
        let all_view = view.clone();
        let mut menu = MoonDropdown::new("rep-core")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(130.0)
            .menu_width(180.0)
            .menu_max_height(360.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .item(
                // «Все» — тумблер: ставит галки на все ядра / снимает все.
                MoonMenuItem::with_key("rc-all", t!("report.filter.all").to_string())
                    .checked(self.sel_cores.is_empty() || all_selected)
                    .selected(self.sel_cores.is_empty() || all_selected)
                    .on_click(move |_, _, app| {
                        all_view.update(app, |t, c| t.toggle_core(None, c));
                    }),
            );
        for (i, (uid, name)) in self.cores.iter().enumerate() {
            let uid = *uid;
            let on = self.sel_cores.contains(&uid);
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("rc-{i}"), name.clone())
                    .checked(on)
                    .selected(on)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.toggle_core(Some(uid), c));
                    }),
            );
        }
        menu
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

    /// Комбобокс периода (пресеты «Сегодня/Вчера/…», как в отчёте Moonbot).
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
            .trigger_width(100.0)
            .menu_width(130.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Меню сохранения отчёта в файл: CSV / CSV со всеми колонками / Excel (XLSX) /
    /// Excel со всеми колонками. Период выборки = текущий фильтр панели
    /// (пресет «Сегодня» и т.п. или ручные даты С:/По:).
    pub(super) fn export_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let item = |key: &'static str,
                    label: String,
                    fmt: super::export::Format,
                    all_cols: bool| {
            let view = view.clone();
            MoonMenuItem::with_key(key, label).on_click(move |_, _, app| {
                view.update(app, |t, c| t.export_report(fmt, all_cols, c));
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
        // Кнопка-глиф в ряд с селектором колонок (общий вид, подсказка тултипом).
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
                    .trigger_width(34.0)
                    .menu_width(200.0)
                    .menu_size(MoonMenuSize::Compact)
                    .items(items),
            )
    }

    /// Попап выбора видимых колонок (чекбоксы) — по рантайм-списку колонок БД,
    /// поэтому авто-добавленные поля ядра сразу доступны к показу.
    pub(super) fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let all_on = !self.table.cols.is_empty()
            && self
                .table
                .cols
                .iter()
                .all(|c| self.visible.contains(c.as_str()));
        let all_view = view.clone();
        let mut items: Vec<MoonMenuItem> = vec![
            // «Все» — тумблер: включить все колонки / повторно — оставить одну первую.
            // Только `checked` (галочка-глиф слева) — БЕЗ `selected`: голубой фон
            // `selected` на светлой теме делал выбранные строки нечитаемыми (см. правку
            // «чекбоксы вместо подсветки»), а checked-глиф — явный индикатор выбора.
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(all_on)
                .on_click(move |_, _, app| {
                    all_view.update(app, |t, c| t.toggle_all_columns(c));
                }),
        ];
        items.extend(self.table.cols.iter().enumerate().map(|(i, c)| {
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
        // Кнопка-глиф вместо поля со списком (общий вид селекторов колонок);
        // подсказка — тултипом (глифы в словарь не кладём, см. locales/README).
        div()
            .id("rep-cols-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| {
                    moon_ui::MoonTooltipView::new(t!("report.columns_menu").to_string())
                })
                .into()
            })
            .child(
                MoonDropdown::new("rep-cols")
                    .segment(moon_ui::MoonButtonSegment::new("▦"))
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width(34.0)
                    .menu_width(230.0)
                    .menu_max_height(420.0)
                    .menu_size(MoonMenuSize::Compact)
                    .close_on_select(false)
                    .items(items),
            )
    }
}
