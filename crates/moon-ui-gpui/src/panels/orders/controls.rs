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
            .trigger_width(design::glyph_btn_w(cx))
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

    /// Builds the settings button for the current-market filter and ordering options.
    ///
    /// The trigger is the SVG gear used everywhere else in the terminal (the header core settings,
    /// the toolbar), not a `⚙` text glyph: the UI font renders that glyph as a placeholder here.
    /// MoonDropdown can only carry text in its trigger, so the menu opens through the shared
    /// Root-owned context menu instead, which is also the repo rule for popups.
    ///
    /// The items are built inside the click handler so their checkmarks read the state at the
    /// moment the menu opens rather than at the last repaint.
    pub(super) fn sort_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        MoonButton::new("orders-sort")
            .leading_icon(moon_ui::MoonButtonIconSlot::new("icons/settings-2.svg"))
            .variant(MoonButtonVariant::Ghost)
            .size(MoonButtonSize::Action)
            // Square, like the field selector beside it.
            .width(design::glyph_btn_w(cx))
            .tooltip(t!("orders.settings").to_string())
            .on_click(move |ev: &ClickEvent, window, app| {
                let items = Self::sort_menu_items(&view, app);
                window.open_moon_context_menu(app, "orders-sort-menu", ev.position(), items, 220.0);
            })
            .render()
    }

    /// Build the settings menu items: the current-market filter, the primary-sort radio group,
    /// newest/oldest ordering, and the Main-on-top modes.
    ///
    /// Every item CLOSES the menu before mutating. A Root-owned context menu does not dismiss
    /// itself on select — unlike the dropdown this replaced — and it captures its items when it
    /// opens, so a menu left standing would keep showing the checkmarks from before the click.
    /// This is also why the shared `radio_items` helper is not used for the sort group: its
    /// callback never sees the `Window` the dismissal needs.
    fn sort_menu_items(view: &Entity<Self>, app: &mut App) -> Vec<MoonMenuItem> {
        let cur = view.read(app).view;
        // One item: check state, close, then apply the edit to the copyable view state.
        let item =
            |key: &'static str, label: String, checked: bool, edit: fn(&mut OrdersViewState)| {
                let v = view.clone();
                MoonMenuItem::with_key(key, label)
                    .checked(checked)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        Self::mutate(&v, app, edit);
                    })
            };
        let mut items = vec![
            item(
                "m-onlycur",
                t!("orders.only_current").to_string(),
                cur.only_current_market,
                |s| s.only_current_market = true,
            ),
            item(
                "m-showall",
                t!("orders.show_all").to_string(),
                !cur.only_current_market,
                |s| s.only_current_market = false,
            ),
            MoonMenuItem::separator(),
        ];
        // Primary sort: a mutually exclusive group, checked like the other radio menus.
        for (variant, key, label) in [
            (
                PrimarySort::ProfitFirst,
                "m-profit",
                t!("orders.sort.profit").to_string(),
            ),
            (
                PrimarySort::SellFirst,
                "m-sell",
                t!("orders.sort.sell").to_string(),
            ),
            (
                PrimarySort::BuyFirst,
                "m-buy",
                t!("orders.sort.buy").to_string(),
            ),
            (
                PrimarySort::Creation,
                "m-creation",
                t!("orders.sort.creation").to_string(),
            ),
        ] {
            let v = view.clone();
            items.push(
                MoonMenuItem::with_key(key, label)
                    .checked(cur.primary == variant)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        Self::mutate(&v, app, |s| s.primary = variant);
                    }),
            );
        }
        items.push(MoonMenuItem::separator());
        items.push(item(
            "m-new",
            t!("orders.sort.new").to_string(),
            cur.newest_first,
            |s| s.newest_first = true,
        ));
        items.push(item(
            "m-old",
            t!("orders.sort.old").to_string(),
            !cur.newest_first,
            |s| s.newest_first = false,
        ));
        // Main on top offers two mutually exclusive choices; clicking the active choice returns to
        // Off. Row highlighting is independent of this ordering mode.
        items.push(MoonMenuItem::separator());
        items.push(item(
            "m-main-all",
            t!("orders.sort.main_all").to_string(),
            cur.main_on_top == MainOnTop::AllTicker,
            |s| {
                s.main_on_top = if s.main_on_top == MainOnTop::AllTicker {
                    MainOnTop::Off
                } else {
                    MainOnTop::AllTicker
                };
            },
        ));
        items.push(item(
            "m-main-hi",
            t!("orders.sort.main_hi").to_string(),
            cur.main_on_top == MainOnTop::Highlighted,
            |s| {
                s.main_on_top = if s.main_on_top == MainOnTop::Highlighted {
                    MainOnTop::Off
                } else {
                    MainOnTop::Highlighted
                };
            },
        ));
        items
    }
}
