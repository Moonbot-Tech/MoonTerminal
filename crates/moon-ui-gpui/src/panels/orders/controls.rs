//! Source, order-kind, column, sorting, and filtering controls for the Orders panel.

use super::*;
use rust_i18n::t;

/// Maximum fitted width that keeps the pinned Auto core inside the narrow Orders panel by itself.
const AUTO_CORE_TRIGGER_MAX_W: f32 = 250.0;
/// Minimum settings-menu width preserving the former compact footprint.
const SETTINGS_MENU_MIN_W: f32 = 220.0;
/// Maximum settings-menu width before unusually long translations truncate safely.
const SETTINGS_MENU_MAX_W: f32 = 560.0;

impl OrdersPanel {
    /// Build the effective core dropdown. Classic mode routes through
    /// [`crate::controls::core_combo`], shared with the Report and Assets panels.
    ///
    /// Classic mode shows the retained selection and permits core or exchange toggles. Auto mode
    /// renders a non-interactive pinned scope chip naming the workspace label instead, without
    /// mutating Classic state.
    ///
    /// Args:
    ///     cores: Group cores in canonical display order.
    ///     cx: Panel context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     Interactive Classic selector or non-interactive pinned scope chip.
    pub(super) fn source_combo(&self, cores: &OrderedCores, cx: &Context<Self>) -> AnyElement {
        let scope = self.effective_scope(self.backend.read(cx));
        let workspace_owned = scope.is_workspace_owned();
        let auto_core = scope.is_auto_core();
        let effective_selection: HashSet<CoreId> = scope.ids().iter().copied().collect();
        let pinned_label = match scope.label() {
            crate::workspace::EffectiveScopeLabel::Overview => {
                Some(t!("workspace.overview").to_string())
            }
            crate::workspace::EffectiveScopeLabel::Core(core) => cores
                .iter()
                .find(|(id, _)| *id == core)
                .map(|(_, name)| crate::display_text::flatten_lines(name)),
            crate::workspace::EffectiveScopeLabel::All
            | crate::workspace::EffectiveScopeLabel::Selection(_) => None,
        };
        let selection = if workspace_owned {
            &effective_selection
        } else {
            &self.sel_cores
        };
        if workspace_owned {
            let p = MoonPalette::active(cx);
            let label = pinned_label.unwrap_or_else(|| {
                crate::controls::core_selection_summary(
                    cores,
                    selection,
                    crate::controls::CoreAllRowMode::ImplicitOrComplete,
                    &t!("orders.all_cores").to_string(),
                    &|n| t!("orders.cores_n", n = n).to_string(),
                )
                .label
            });
            let width = if auto_core {
                px(MoonDropdown::fitted_trigger_label(
                    cx,
                    &label,
                    MoonButtonSize::Action,
                    crate::controls::CORE_COMBO_TRIGGER_W,
                    AUTO_CORE_TRIGGER_MAX_W,
                )
                .1)
            } else {
                px(crate::controls::wrap_fit::action_width(
                    cx,
                    crate::controls::CORE_COMBO_TRIGGER_W,
                ))
            };
            crate::panels::pinned_scope_host(
                "orders-source-tip",
                "orders-source",
                label,
                width,
                p,
                cx,
            )
        } else {
            let view = cx.entity();
            let exchange_view = view.clone();
            let venues = self.backend.read(cx).session.core_venues();
            let extras =
                crate::controls::core_combo_extras(!workspace_owned, &view, &self.backend, cx);
            crate::controls::core_combo(
                "orders-source",
                cores,
                &venues,
                selection,
                crate::controls::CoreAllRowMode::ImplicitOrComplete,
                t!("orders.all_cores").to_string(),
                |n| t!("orders.cores_n", n = n).to_string(),
                170.0,
                extras,
                move |id, app| {
                    view.update(app, |t, c| t.toggle_core(id, c));
                },
                move |exchange_cores, app| {
                    exchange_view.update(app, |t, c| {
                        t.toggle_exchange_cores(exchange_cores, c);
                    });
                },
            )
            .into_any_element()
        }
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
            // An icon button instead of a text field, matching the other column selectors; the
            // asset and the childless trigger are `design::COLUMN_SELECTOR_ICON`'s contract. It is
            // deliberately NOT `sort_menu`'s gear below: the two sit side by side in this bar.
            .trigger_icon(design::COLUMN_SELECTOR_ICON)
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

    /// Build the settings dropdown for the current-market filter and ordering options.
    ///
    /// The MoonDropdown trigger uses the same SVG gear and square width as neighboring toolbar
    /// controls. Its standard selection lifecycle owns anchoring, dismissal, and fresh checkmarks
    /// on the repaint caused by each view-state mutation.
    ///
    /// Args:
    ///     cx: Panel context used to build current-state menu items and callbacks.
    ///
    /// Returns:
    ///     A tooltip host containing the configured settings dropdown.
    pub(super) fn sort_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let menu = MoonDropdown::new("orders-sort")
            .trigger_icon("icons/settings-2.svg")
            .trigger_variant(MoonButtonVariant::Ghost)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::glyph_btn_w(cx))
            .fit_menu_width(SETTINGS_MENU_MIN_W, SETTINGS_MENU_MAX_W)
            .items(Self::sort_menu_items(&view, self.view));
        div()
            .id("orders-sort-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("orders.settings").to_string()))
                    .into()
            })
            .child(menu)
    }

    /// Build current-state settings rows for the standard dropdown selection lifecycle.
    ///
    /// Args:
    ///     view: Orders panel entity mutated by item callbacks.
    ///     cur: Copy of the view state whose values determine selection marks.
    ///
    /// Returns:
    ///     Menu rows for market scope, primary sort, age order, and Main-on-top mode.
    fn sort_menu_items(view: &Entity<Self>, cur: OrdersViewState) -> Vec<MoonMenuItem> {
        // Build paired boolean choices with the same mutation path as the radio group below.
        let item =
            |key: &'static str, label: String, checked: bool, edit: fn(&mut OrdersViewState)| {
                let v = view.clone();
                MoonMenuItem::with_key(key, label)
                    .checked(checked)
                    .on_click(move |_, _, app| Self::mutate(&v, app, edit))
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
        // Primary sort uses the shared mutually exclusive dropdown-row helper.
        let sort_view = view.clone();
        items.extend(crate::panels::radio_items(
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
            move |app, primary| {
                Self::mutate(&sort_view, app, |state| {
                    state.primary = primary;
                    state.header_sort = None;
                })
            },
        ));
        items.push(MoonMenuItem::separator());
        items.push(item(
            "m-new",
            t!("orders.sort.new").to_string(),
            cur.newest_first,
            |s| {
                s.newest_first = true;
                s.header_sort = None;
            },
        ));
        items.push(item(
            "m-old",
            t!("orders.sort.old").to_string(),
            !cur.newest_first,
            |s| {
                s.newest_first = false;
                s.header_sort = None;
            },
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
