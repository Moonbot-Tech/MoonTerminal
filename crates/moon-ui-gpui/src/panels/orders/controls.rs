//! Поля-списки (источник/тип) и меню сортировки/фильтра панели «Ордера».

use super::*;
use rust_i18n::t;

impl OrdersPanel {
    /// Поле-список источника (Все ядра + ядра группы) — порт egui ComboBox.
    pub(super) fn source_combo(
        &self,
        cores: &[(CoreId, String)],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let cur = match self.view.source {
            OrdersSource::All => t!("orders.all_cores").to_string(),
            OrdersSource::Core(id) => cores
                .iter()
                .find(|(c, _)| *c == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| t!("orders.all_cores").to_string()),
        };
        let view = cx.entity();
        let mut menu = MoonDropdown::new("orders-source")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(118.0)
            .menu_width(160.0)
            .menu_size(MoonMenuSize::Compact)
            .item(
                MoonMenuItem::with_key("all", t!("orders.all_cores").to_string())
                    .checked(matches!(self.view.source, OrdersSource::All))
                    .on_click({
                        let view = view.clone();
                        move |_, _, app| Self::mutate(&view, app, |v| v.source = OrdersSource::All)
                    }),
            );
        for (id, name) in cores {
            let id = *id;
            let selected = matches!(self.view.source, OrdersSource::Core(cur) if cur == id);
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("core-{id}"), name.clone())
                    .checked(selected)
                    .on_click(move |_, _, app| {
                        Self::mutate(&view, app, |v| v.source = OrdersSource::Core(id))
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
        let mut menu = MoonDropdown::new("orders-kind")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(102.0)
            .menu_width(138.0)
            .menu_size(MoonMenuSize::Compact);
        for (k, id, label) in [
            (OrderKind::All, "all", t!("orders.kind.all").to_string()),
            (OrderKind::Real, "real", t!("orders.kind.real").to_string()),
            (OrderKind::Emu, "emu", t!("orders.kind.emu").to_string()),
        ] {
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("kind-{id}"), label)
                    .checked(self.view.kind == k)
                    .on_click(move |_, _, app| Self::mutate(&view, app, |v| v.kind = k)),
            );
        }
        menu
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
        for (variant, label, id) in [
            (
                PrimarySort::SellFirst,
                t!("orders.sort.sell").to_string(),
                "m-sell",
            ),
            (
                PrimarySort::BuyFirst,
                t!("orders.sort.buy").to_string(),
                "m-buy",
            ),
            (
                PrimarySort::Creation,
                t!("orders.sort.creation").to_string(),
                "m-creation",
            ),
        ] {
            let v = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(id, label)
                    .checked(cur.primary == variant)
                    .on_click(move |_, _, app| Self::mutate(&v, app, |s| s.primary = variant)),
            );
        }
        let v = view.clone();
        menu = menu.item(MoonMenuItem::separator()).item(
            MoonMenuItem::with_key("m-new", t!("orders.sort.new").to_string())
                .checked(cur.newest_first)
                .on_click(move |_, _, app| Self::mutate(&v, app, |s| s.newest_first = true)),
        );
        let v = view;
        menu.item(
            MoonMenuItem::with_key("m-old", t!("orders.sort.old").to_string())
                .checked(!cur.newest_first)
                .on_click(move |_, _, app| Self::mutate(&v, app, |s| s.newest_first = false)),
        )
    }
}
