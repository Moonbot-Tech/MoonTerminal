//! Поля-списки (источник/тип) и меню сортировки/фильтра панели «Ордера».

use super::*;
use rust_i18n::t;

impl OrdersPanel {
    /// Поле-список ядер — МУЛЬТИВЫБОР (чекбоксы, меню не закрывается на клик, как в «Отчёте»
    /// и выборе колонок). Подпись: «Все» (пусто/все) / имя единственного / «Ядер: N».
    pub(super) fn source_combo(
        &self,
        cores: &[(CoreId, String)],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let all_selected = !cores.is_empty() && self.sel_cores.len() == cores.len();
        let cur = match self.sel_cores.len() {
            0 => t!("orders.all_cores").to_string(),
            _ if all_selected => t!("orders.all_cores").to_string(),
            1 => {
                let id = *self.sel_cores.iter().next().unwrap();
                cores
                    .iter()
                    .find(|(c, _)| *c == id)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| t!("orders.cores_n", n = 1).to_string())
            }
            n => t!("orders.cores_n", n = n).to_string(),
        };
        let view = cx.entity();
        let all_view = view.clone();
        let mut menu = MoonDropdown::new("orders-source")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(118.0)
            .menu_width(170.0)
            .menu_max_height(360.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .item(
                // «Все» — тумблер: снять все / выбрать все ядра группы.
                MoonMenuItem::with_key("os-all", t!("orders.all_cores").to_string())
                    .checked(self.sel_cores.is_empty() || all_selected)
                    .selected(self.sel_cores.is_empty() || all_selected)
                    .on_click(move |_, _, app| {
                        all_view.update(app, |t, c| t.toggle_core(None, c));
                    }),
            );
        for (i, (id, name)) in cores.iter().enumerate() {
            let id = *id;
            let on = self.sel_cores.contains(&id);
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("os-{i}"), name.clone())
                    .checked(on)
                    .selected(on)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.toggle_core(Some(id), c));
                    }),
            );
        }
        menu
    }

    /// Поле-список типа ордеров (Все / Реальные / Эмуляторные).
    pub(super) fn kind_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cur = match self.view.kind {
            OrderKind::All => t!("orders.kind.all"),
            OrderKind::Real => t!("orders.kind.real"),
            OrderKind::Emu => t!("orders.kind.emu"),
        };
        let view = cx.entity();
        let items = crate::panels::radio_items(
            [
                (
                    OrderKind::All,
                    "kind-all".into(),
                    t!("orders.kind.all").to_string().into(),
                ),
                (
                    OrderKind::Real,
                    "kind-real".into(),
                    t!("orders.kind.real").to_string().into(),
                ),
                (
                    OrderKind::Emu,
                    "kind-emu".into(),
                    t!("orders.kind.emu").to_string().into(),
                ),
            ],
            self.view.kind,
            crate::panels::RadioMark::Check,
            move |app, k| Self::mutate(&view, app, |v| v.kind = k),
        );
        MoonDropdown::new("orders-kind")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(102.0)
            .menu_width(138.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Поле-список выбора отображаемых колонок таблицы. Каждый пункт — чекбокс-тогл
    /// видимости колонки; меню НЕ закрывается на клик (`close_on_select(false)`), чтобы
    /// можно было отметить сразу несколько. Нельзя скрыть ВСЕ колонки — последняя
    /// видимая колонка не тогается (иначе таблица станет пустой). Состояние персистится.
    pub(super) fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cur = self.view;
        let mut menu = MoonDropdown::new("orders-columns")
            // Кнопка-глиф вместо поля со списком (общий вид селекторов колонок).
            .segment(moon_ui::MoonButtonSegment::new("▦"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(34.0)
            .menu_width(170.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false);
        // «Все» — тумблер: включить все колонки / повторно — оставить одну первую.
        let full_mask = OrdCol::ALL.iter().fold(0u16, |m, c| m | c.bit());
        let all_on = cur.columns == full_mask;
        let all_view = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| {
                    Self::mutate(&all_view, app, |v| {
                        v.columns = if v.columns == full_mask {
                            OrdCol::ALL[0].bit()
                        } else {
                            full_mask
                        };
                    })
                }),
        );
        for col in OrdCol::ALL {
            let shown = cur.shows(col);
            // Последняя оставшаяся видимая колонка заблокирована на выключение.
            let last_visible = shown && cur.columns == col.bit();
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("col-{}", col.key()), super::table::col_title(col))
                    .checked(shown)
                    .disabled(last_visible)
                    .on_click(move |_, _, app| {
                        Self::mutate(&view, app, |v| {
                            let next = v.columns ^ col.bit();
                            // Защита от пустой таблицы: не применяем, если погасли все колонки.
                            if next != 0 {
                                v.columns = next;
                            }
                        })
                    }),
            );
        }
        div()
            .id("orders-cols-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("orders.columns").to_string()))
                    .into()
            })
            .child(menu)
    }

    /// Меню сортировки/фильтра (порт ПКМ-меню egui): фильтр текущего маркета + две
    /// тогл-группы сортировки. В GPUI — попап-кнопка (PopupMenu основан на Action).
    pub(super) fn sort_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cur = self.view;
        let v = view.clone();
        let mut menu = MoonDropdown::new("orders-sort")
            .label("⚙")
            .trigger_variant(MoonButtonVariant::Ghost)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(34.0)
            .menu_width(220.0)
            .menu_size(MoonMenuSize::Normal)
            .item(
                MoonMenuItem::with_key("m-onlycur", t!("orders.only_current").to_string())
                    .checked(cur.only_current_market)
                    .on_click(move |_, _, app| {
                        Self::mutate(&v, app, |s| s.only_current_market = true)
                    }),
            );
        let v = view.clone();
        menu = menu
            .item(
                MoonMenuItem::with_key("m-showall", t!("orders.show_all").to_string())
                    .checked(!cur.only_current_market)
                    .on_click(move |_, _, app| {
                        Self::mutate(&v, app, |s| s.only_current_market = false)
                    }),
            )
            .item(MoonMenuItem::separator());
        menu = menu.items(crate::panels::radio_items(
            [
                (
                    PrimarySort::ProfitFirst,
                    "m-profit".into(),
                    t!("orders.sort.profit").to_string().into(),
                ),
                (
                    PrimarySort::SellFirst,
                    "m-sell".into(),
                    t!("orders.sort.sell").to_string().into(),
                ),
                (
                    PrimarySort::BuyFirst,
                    "m-buy".into(),
                    t!("orders.sort.buy").to_string().into(),
                ),
                (
                    PrimarySort::Creation,
                    "m-creation".into(),
                    t!("orders.sort.creation").to_string().into(),
                ),
            ],
            cur.primary,
            crate::panels::RadioMark::Check,
            {
                let v = view.clone();
                move |app, variant| Self::mutate(&v, app, |s| s.primary = variant)
            },
        ));
        let v = view.clone();
        menu = menu.item(MoonMenuItem::separator()).item(
            MoonMenuItem::with_key("m-new", t!("orders.sort.new").to_string())
                .checked(cur.newest_first)
                .on_click(move |_, _, app| Self::mutate(&v, app, |s| s.newest_first = true)),
        );
        let v = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("m-old", t!("orders.sort.old").to_string())
                .checked(!cur.newest_first)
                .on_click(move |_, _, app| Self::mutate(&v, app, |s| s.newest_first = false)),
        );
        // «Main сверху» — две взаимоисключающие галки + возможность выключить (клик по уже
        // активной снимает её → Off). Подсветка строк от этого не зависит.
        let v = view.clone();
        menu = menu.item(MoonMenuItem::separator()).item(
            MoonMenuItem::with_key("m-main-all", t!("orders.sort.main_all").to_string())
                .checked(cur.main_on_top == MainOnTop::AllTicker)
                .on_click(move |_, _, app| {
                    Self::mutate(&v, app, |s| {
                        s.main_on_top = if s.main_on_top == MainOnTop::AllTicker {
                            MainOnTop::Off
                        } else {
                            MainOnTop::AllTicker
                        };
                    })
                }),
        );
        let v = view;
        menu.item(
            MoonMenuItem::with_key("m-main-hi", t!("orders.sort.main_hi").to_string())
                .checked(cur.main_on_top == MainOnTop::Highlighted)
                .on_click(move |_, _, app| {
                    Self::mutate(&v, app, |s| {
                        s.main_on_top = if s.main_on_top == MainOnTop::Highlighted {
                            MainOnTop::Off
                        } else {
                            MainOnTop::Highlighted
                        };
                    })
                }),
        )
    }
}
