//! Source, order-kind, column, sorting, and filtering controls for the Orders panel.

use super::*;
use rust_i18n::t;

impl OrdersPanel {
    /// Builds the multi-select core dropdown shared with the Report and Assets panels through
    /// [`crate::controls::core_combo`].
    ///
    /// Its fixed-width label shows All Cores for an empty or complete selection and the localized
    /// current-core count otherwise, including a sole selection. Clicking a known exchange header
    /// batch-toggles its currently available cores.
    ///
    /// Args:
    ///     cores: Group cores in canonical display order.
    ///     cx: Panel context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     The configured fixed-trigger dropdown.
    pub(super) fn source_combo(
        &self,
        cores: &OrderedCores,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        let exchange_view = view.clone();
        let exchange_names = self
            .backend
            .read(cx)
            .session
            .market_source()
            .core_exchange_names();
        crate::controls::core_combo(
            "orders-source",
            cores,
            &exchange_names,
            &self.sel_cores,
            t!("orders.all_cores").to_string(),
            |n| t!("orders.cores_n", n = n).to_string(),
            170.0,
            move |id, app| {
                view.update(app, |t, c| t.toggle_core(id, c));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        )
    }

    /// Builds the All, Real, and Emulated order-kind dropdown.
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
            .label(cur)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(102.0)
            .menu_width_scaled(138.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Builds the persisted table-column visibility menu.
    ///
    /// Each item toggles one column, and `close_on_select(false)` keeps the menu open for multiple
    /// edits. The final visible column cannot be disabled, preventing an empty table.
    pub(super) fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cur = self.view;
        let mut menu = MoonDropdown::new("orders-columns")
            // Use a glyph button instead of a text field, matching the other column selectors.
            .segment(moon_ui::MoonButtonSegment::new("▦"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(34.0)
            .menu_width_scaled(170.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false);
        // The All item enables every column; clicking it again leaves only the first canonical one.
        // `ALL_COLUMNS_MASK` is the single source of truth for "every column visible".
        let full_mask = ALL_COLUMNS_MASK;
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
            // Disable turning off the final visible column.
            let last_visible = shown && cur.columns == col.bit();
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("col-{}", col.key()), super::table::col_title(col))
                    .checked(shown)
                    .disabled(last_visible)
                    .on_click(move |_, _, app| {
                        Self::mutate(&view, app, |v| {
                            let next = v.columns ^ col.bit();
                            // Reject an empty visibility mask as a final guard against a blank table.
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

    /// Builds the gear-triggered dropdown for the current-market filter and ordering options.
    ///
    /// The menu combines a primary-sort radio group, newest/oldest ordering, and Main-on-top modes.
    pub(super) fn sort_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let cur = self.view;
        let v = view.clone();
        let mut menu = MoonDropdown::new("orders-sort")
            .label("⚙")
            .trigger_variant(MoonButtonVariant::Ghost)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(34.0)
            .menu_width_scaled(220.0)
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
        // Main on top offers two mutually exclusive choices; clicking the active choice returns to
        // Off. Row highlighting is independent of this ordering mode.
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
