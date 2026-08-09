//! Assets window chrome: the control bar (core selector, dust threshold, field selector), the
//! positions/balances table, the footer, and the "Wallets" section core list (free/total).
//!
//! The footer carries both summaries the panel produces — the visible rows on the left, the
//! scope's account equity on the right — with [`super::balances`] rendering the account side.
//! `AssetsView` supplies table rows already ordered — by the active header sort, otherwise by
//! descending raw USDT value — and the visible field set; both live in [`super::columns`]. The
//! table renders that order and uses caller-owned column-width state, restored and persisted
//! separately for dock and window contexts by the panel owner.

use super::*;
use crate::controls::{CoinMenuCtx, CoinMenuOrigin};
use moon_ui::{MoonButtonVariant, MoonDisclosure, MoonNotification, MoonText, MoonWindowExt as _};
use rust_i18n::t;

/// Open the shared coin context menu from an Assets row's ticker right-click.
///
/// Balance rows carry no strategy or order data, so the menu contains navigation and core
/// blacklist actions. Its selected-core set comes from `query_cores()`: retained Classic scope,
/// effective Auto scope for a group panel, or every core for the global window.
///
/// Args:
///     core: Core that owns the clicked balance row.
///     market: Resolved market used by navigation entries.
///     coin: Core-native token spelling used by blacklist entries.
///     view: Owning group or global Assets view.
///     pos: Window-coordinate popup position.
///     window: Window that owns the context menu.
///     app: Application context used to resolve current scope and open the menu.
///
/// Returns:
///     Nothing; an applicable menu is opened at `pos`.
fn open_asset_coin_menu(
    core: CoreId,
    market: String,
    coin: String,
    view: &Entity<AssetsView>,
    pos: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    app.stop_propagation();
    let (backend, workspace_group) = {
        let panel = view.read(app);
        let workspace_group = match &panel.scope {
            AssetsScope::Group(group) => Some(group.clone()),
            AssetsScope::All => None,
        };
        (panel.backend.clone(), workspace_group)
    };
    let (selected_cores, core_name) = {
        let b = backend.read(app);
        let sessions = b.session.sessions();
        let selected: Vec<CoreId> = view
            .read(app)
            .query_cores(b)
            .into_iter()
            .map(|(core, _)| core)
            .collect();
        let name = sessions
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        (selected, name)
    };
    let ctx = CoinMenuCtx {
        core,
        core_name,
        market,
        coin,
        selected_cores,
        strat_id: None,
        strat_name: None,
        order_uid: None,
        workspace_group,
        side: None,
        short: false,
        origin: CoinMenuOrigin::OrderTable,
        trailing: Vec::new(),
    };
    crate::controls::open_coin_menu(ctx, backend, pos, window, app);
}

impl AssetsView {
    /// Top controls: multi-core selector and dust threshold. Every summary figure — the row
    /// count, Σ over visible rows, and the scope balance — is rendered by [`Self::footer`].
    pub(super) fn core_bar(&self, cores: &OrderedCores, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.core_combo(cores, cx))
            // Pair the dust-threshold slider with its `≥ N$` label; zero shows everything. The
            // mouse wheel changes the threshold by $1 because dragging the narrow slider is hard.
            .child(
                div()
                    .id("assets-min-value-wheel")
                    .w(px(120.0))
                    // Explain that zero shows everything and the mouse wheel adjusts the value.
                    .tooltip(|_window, cx| {
                        cx.new(|_| {
                            moon_ui::MoonTooltipView::new(t!("assets.min_value_hint").to_string())
                        })
                        .into()
                    })
                    .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, window, cx| {
                        let dy = match ev.delta {
                            ScrollDelta::Lines(pt) => pt.y,
                            ScrollDelta::Pixels(pt) => f32::from(pt.y),
                        };
                        if dy == 0.0 {
                            return;
                        }
                        let next = (this.min_value_usd + if dy > 0.0 { 1.0 } else { -1.0 })
                            .clamp(0.0, 100.0);
                        if next != this.min_value_usd {
                            this.min_value_usd = next;
                            this.min_value_slider.update(cx, |s, c| {
                                s.set_value(next as f32, window, c);
                            });
                            let backend = this.backend.clone();
                            this.rebuild_cache(backend.read(cx));
                            this.persist_min_value(cx);
                            cx.notify();
                        }
                    }))
                    .child(
                        MoonSlider::new(&self.min_value_slider)
                            .id("assets-min-value")
                            .height(18.0),
                    ),
            )
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("≥ {}$", self.min_value_usd.round() as i64)),
            )
            // The field selector sits at the RIGHT edge, apart from the filters — the same split
            // Orders, Report and the Screener use.
            .child(div().flex_1())
            .child(self.columns_menu(cx))
    }

    /// Render the shared exchange-grouped core selector under the current scope authority.
    ///
    /// Global and Classic group views expose the retained multi-selection. Group Auto pins the
    /// effective workspace label and disables the selector without changing Classic state.
    ///
    /// Args:
    ///     cores: Scoped cores in canonical display order.
    ///     cx: View context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     Interactive retained-scope selector or disabled Auto scope indicator.
    pub(super) fn core_combo(&self, cores: &OrderedCores, cx: &Context<Self>) -> impl IntoElement {
        let scope = self.effective_scope(self.backend.read(cx));
        let workspace_owned = scope
            .as_ref()
            .is_some_and(EffectiveCoreScope::is_workspace_owned);
        let effective_selection: HashSet<CoreId> = scope
            .as_ref()
            .map(|scope| scope.ids().iter().copied().collect())
            .unwrap_or_default();
        let pinned_label = scope.as_ref().and_then(|scope| match scope.label() {
            crate::workspace::EffectiveScopeLabel::Overview => {
                Some(t!("workspace.overview").to_string())
            }
            crate::workspace::EffectiveScopeLabel::Core(core) => cores
                .iter()
                .find(|(id, _)| *id == core)
                .map(|(_, name)| name.clone()),
            crate::workspace::EffectiveScopeLabel::All
            | crate::workspace::EffectiveScopeLabel::Selection(_) => None,
        });
        let view = cx.entity();
        let exchange_view = view.clone();
        let exchange_names = self
            .backend
            .read(cx)
            .session
            .market_source()
            .core_exchange_names();
        let combo = crate::controls::core_combo(
            "assets-core",
            cores,
            &exchange_names,
            if workspace_owned {
                &effective_selection
            } else {
                &self.sel_cores
            },
            crate::controls::CoreAllRowMode::ImplicitOrComplete,
            t!("assets.all_cores").to_string(),
            |n| t!("assets.cores_n", n = n).to_string(),
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
        .disabled(workspace_owned);
        if let Some(label) = pinned_label {
            combo.label(label)
        } else {
            combo
        }
    }

    /// The panel footer: one row carrying two semantically distinct summaries.
    ///
    /// LEFT — the rows currently in the TABLE: how many, and Σ of their value after the dust
    /// filter. RIGHT — the ACCOUNTS: equity across every in-scope core
    /// ([`super::balances::summary_group`]).
    ///
    /// The two share a line but are not the same quantity: a futures core with no open positions
    /// can have an empty table (Σ = 0) and a fully funded account. Three devices keep them apart:
    /// the divider between the groups, different nouns ("positions" versus "cores"), and a tooltip on
    /// each group naming what it sums. It is also why Σ sits beside its own count rather than at
    /// the far right:
    /// tight inner gaps bind a value to its label, the wide outer gap and the spacer separate
    /// the subjects.
    ///
    /// Σ obeys the same honesty rules as the balance side: it is muted once any visible row was
    /// dropped from it for a non-finite value (the row count still includes that row), and a dash
    /// when rows exist but none contributes. An actually empty table retains its known Σ of zero.
    /// The tooltip names any excluded-row count.
    ///
    /// Narrow docks clip, never scroll — a scrolling footer hides information behind an
    /// affordance nobody looks for. The left group and the divider are `flex_none`; the balance
    /// group yields first and owns its own clipping/tooltip contract (see
    /// [`super::balances::summary_group`]).
    pub(super) fn footer(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let count = self.cached_entries.len();
        let excluded = self.cached_value_excluded;
        // Σ is muted the moment it stops covering every counted row, and becomes a dash when it
        // covers none of them — the same "partial is not complete" rule the balance group uses.
        //
        // It deliberately does NOT share the balance group's dash rule outright: an EMPTY table
        // is a known zero (no rows, so their sum really is nothing), while the balance group's
        // `counted == 0` means the cores have not reported and the figure is unknown. Same glyph,
        // different question — merging them would print a dash over an honest zero.
        let sigma = if excluded > 0 && excluded == count {
            super::balances::DASH.to_string()
        } else {
            super::balances::money_or_dash(self.cached_total_value)
        };
        let mut table_tip = t!("assets.footer_table_hint").to_string();
        if excluded > 0 {
            table_tip.push('\n');
            table_tip.push_str(&t!("assets.footer_rows_unpriced", n = excluded));
        }
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .overflow_x_hidden()
            .child(
                h_flex()
                    .id("assets-footer-table")
                    .flex_none()
                    .items_center()
                    .gap(design::ui_px(cx, 5.0))
                    .tooltip(move |_window, cx| {
                        cx.new(|_| moon_ui::MoonTooltipView::new(table_tip.clone()))
                            .into()
                    })
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_soft))
                            .child(t!("assets.section_positions").to_string()),
                    )
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_muted))
                            .child(format!("{count}")),
                    )
                    .child(super::balances::amount(
                        format!("Σ {sigma}"),
                        if excluded == 0 {
                            p.text_soft
                        } else {
                            p.text_muted
                        },
                        cx,
                    )),
            )
            .child(div().flex_1())
            .child(design::vline(cx, 12.0, p.border))
            .child(super::balances::summary_group(
                &self.cached_aggs,
                &HashSet::new(),
                cx,
            ))
    }

    /// Collapsible Wallets section: a selectable balance-aware core list plus the Spot,
    /// Futures, and Quarterly transfer containers. Expanded content shares the available height.
    ///
    /// Args:
    ///     aggs: Balance aggregates for the cores currently in scope.
    ///     wallets: Transfer-container snapshots for those cores.
    ///     cx: Assets view context.
    ///
    /// Returns:
    ///     The Wallets header and its expanded content, when open.
    pub(super) fn bottom(
        &self,
        aggs: &[CoreAgg],
        wallets: &[WalletColumnSnapshot],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        // Keep the selected core while it remains in scope; otherwise fall back to the first core.
        let selected = self
            .effective_wallet_core(self.backend.read(cx))
            .filter(|c| aggs.iter().any(|a| a.id == *c))
            .or_else(|| aggs.first().map(|a| a.id));

        // Section header, styled like the Positions and Cores headers.
        let collapsed = self.wallets_collapsed;
        let mut header = h_flex()
            .id("assets-wallets-bar")
            .w_full()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                h_flex()
                    .id("assets-wallets-toggle")
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(p.text)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wallets_collapsed = !this.wallets_collapsed;
                        cx.notify();
                    }))
                    // Passive: the enclosing toggle row owns the click, so the caret must not
                    // take a hitbox of its own and swallow it. Unlike the label beside it this
                    // caret rides the UI slider, not the Font slider — it is chrome.
                    .child(
                        MoonDisclosure::glyph(!collapsed)
                            .size(design::DISCLOSURE_GLYPH_MARKER)
                            .box_size(design::DISCLOSURE_BOX),
                    )
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text_soft))
                            .child(t!("assets.wallets_hint").to_string()),
                    ),
            )
            .child(div().flex_1());
        if let Some(core) = selected {
            header = header.child(
                MoonButton::new("assets-refresh-transfer")
                    .ghost()
                    .size(MoonButtonSize::Micro)
                    .label("↻")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Err(error) =
                            this.backend.read(cx).session.refresh_transfer_assets(core)
                        {
                            log::warn!(
                                "assets refresh failed for core {}: {error}",
                                moon_core::feed::core_label(core)
                            );
                            window
                                .push_notification(MoonNotification::error(error.to_string()), cx);
                        }
                        let backend = this.backend.clone();
                        this.rebuild_cache(backend.read(cx));
                        cx.notify();
                    }))
                    .render(),
            );
        }
        if collapsed {
            return v_flex().w_full().flex_none().child(header);
        }

        // Left column: core names with free and total USDT balances.
        let mut list = v_flex().w_full().gap_0();
        for agg in aggs {
            let cid = agg.id;
            let active = selected == Some(cid);
            let mut item = h_flex()
                .id(SharedString::from(format!("asset-core-{cid}")))
                .w_full()
                .h(design::fit_h_px(cx, 24.0, 13.0, 5.0))
                .px(design::ui_px(cx, 8.0))
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .text_color(rgb(p.text))
                .child(div().flex_1().min_w_0().truncate().child(agg.name.clone()))
                // Per-core trust, rendered by the module that owns the vocabulary — so a core
                // shown as current here cannot be one the footer total counts as stale.
                .child(super::balances::figure(Some(agg), p, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let AssetsScope::Group(_) = &this.scope
                        && this
                            .effective_scope(this.backend.read(cx))
                            .is_some_and(|scope| scope.is_workspace_owned())
                    {
                        return;
                    }
                    if this.selected_core != Some(cid) {
                        this.selected_core = Some(cid);
                        if let Err(error) =
                            this.backend.read(cx).session.refresh_transfer_assets(cid)
                        {
                            log::warn!("assets refresh failed for core {cid}: {error}");
                            window
                                .push_notification(MoonNotification::error(error.to_string()), cx);
                        }
                        let backend = this.backend.clone();
                        this.rebuild_cache(backend.read(cx));
                        cx.notify();
                    }
                }));
            if active {
                item = item.bg(rgb(p.panel)).text_color(rgb(p.blue));
            } else {
                item = item.hover(|s| s.bg(rgb(p.shell_high)));
            }
            list = list.child(item);
        }

        // Style the left container like the wallet columns, including the same `shell_high` header,
        // so the expanded section reads as four matching vertical containers.
        let left = v_flex()
            .w(px(240.0))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .px(design::ui_px(cx, 6.0))
                    .py(design::ui_px(cx, 3.0))
                    .bg(rgb(p.shell_high))
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.text_soft))
                    .child(t!("assets.cores_free_total").to_string()),
            )
            .child(
                div()
                    .id("asset-core-list")
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(list),
            );

        // Right side: Spot, Futures, and Quarterly wallet containers.
        let right = match selected {
            Some(core) => self.wallets_section(core, wallets, cx).into_any_element(),
            None => div()
                .p_4()
                .text_color(rgb(p.text_muted))
                .child(t!("assets.no_cores").to_string())
                .into_any_element(),
        };

        // Let the expanded section share flexible height while keeping headers and a few rows visible.
        v_flex()
            .w_full()
            .flex_1()
            .min_h(px(160.0))
            .child(header)
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .child(left)
                    .child(div().flex_1().h_full().min_w_0().child(right)),
            )
    }
}

/// Define the visible Moonbot Assets columns, action buttons included.
///
/// This shared multi-core panel adds a Core column on the left; Moonbot's window is per-core. Which
/// columns appear — the buttons among them — comes from the field selector ([`super::columns`]).
fn assets_columns(visible: &[AssetCol]) -> Vec<MoonDataTableColumn> {
    visible.iter().map(|c| c.column()).collect()
}

/// Assets table with caller-supplied empty-state copy for the current scope.
///
/// `visible` is the field set in canonical order; the row builder emits its cells in exactly that
/// order, so a hidden field costs no cell.
pub(super) fn assets_table(
    id: &'static str,
    rows: Rc<Vec<AssetEntry>>,
    sell_marked: Rc<std::collections::HashSet<(CoreId, String)>>,
    visible: Rc<Vec<AssetCol>>,
    state: &Entity<MoonDataTableState>,
    empty_msg: String,
    cx: &Context<AssetsView>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let view = cx.entity();
    let sort_view = cx.entity();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);
    let row_cols = visible.clone();

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        empty_msg,
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, _app| {
            let e = &table_rows[ix];
            // Hyperliquid orders and catalog markets use indexed names such as `@151`, while
            // transfer-wallet rows expose canonical token names. Coin matching bridges the two
            // representations when marking a row as being sold.
            let on_sale = sell_marked.contains(&(e.core, e.row.coin.to_ascii_uppercase()));
            assets_row(e, &row_cols, &view, p, on_sale)
        })
        .columns(assets_columns(&visible))
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H)
        // A header click re-sorts the cached rows; the action column is not sortable.
        .on_sort(move |key, ascending, _window, app| {
            let key = key.to_string();
            sort_view.update(app, |this, cx| this.set_sort(&key, ascending, cx));
        }),
    )
}

/// Build one table row using `display_value` for both the value cell and footer-compatible data.
///
/// `visible` is the field set in canonical order, so the row emits exactly the cells the columns
/// declare — including the action buttons, which the selector can turn off like any other field.
fn assets_row(
    e: &AssetEntry,
    visible: &[AssetCol],
    view: &Entity<AssetsView>,
    p: MoonPalette,
    on_sale: bool,
) -> MoonDataRow {
    let is_position = e.row.pos_size != 0.0;
    let cells: Vec<MoonDataCell> = visible
        .iter()
        .map(|col| match col {
            // Classic clicks toggle the retained one-core filter; Auto clicks select the workspace
            // core and repeating the active core is a no-op.
            AssetCol::Core => MoonDataCell::element(core_cell(e, view, p)),
            // Clicking the ticker opens its market on Main using the row's core, as in Orders and
            // Report.
            AssetCol::Coin => MoonDataCell::element(coin_cell(e, view, p, on_sale)),
            // For spot, show the full held balance: free plus the amount locked in open sell orders.
            // Like Moonbot, this keeps a holding with TP orders visible instead of showing its
            // near-zero free amount. For futures, show the remaining position and its notional at
            // market price in the USD-stable quote currency. `fmt::qty` uses magnitude-bounded
            // precision from tenths through thousandths rather than adaptive formatting.
            AssetCol::Qty => {
                MoonDataCell::text(moon_core::util::fmt::qty(super::columns::row_qty(e)))
            }
            // The value column and the footer's Σ read the SAME field — see
            // `AssetEntry::display_value`. A dash, never `inf,0$`: a broken price must not read as
            // an astronomically valuable row, and the same non-finite check keeps that row out of Σ
            // with the footer saying so.
            AssetCol::Value => MoonDataCell::text(super::balances::money_or_dash(e.display_value)),
            AssetCol::Pnl => pnl_cell(e),
            AssetCol::Actions => actions_cell(e, view, p, is_position),
        })
        .collect();
    MoonDataRow::new(cells)
        // Highlight a coin or position being sold by an active sell order or a SellSet/
        // SellAlmostDone phase on this core; the ticker also receives a blue `SELL` badge.
        .selected(on_sale)
}

/// Build the unrealized-PnL cell as a signed green or red amount.
///
/// The number comes straight from `AssetRow::pnl_usdt`, which the FEED computes once per snapshot
/// as `(mark − entry) × size` per position leg (`moon_core::feed::assets`) and converts into USDT.
/// Recomputing it here would put the same arithmetic on a per-frame path for no new information,
/// and would risk drifting away from the figure the Orders panel shows.
///
/// [`super::columns::pnl_display`] decides whether there is anything to show at all: a spot balance
/// has no unrealized PnL, and neither a missing entry price nor an unknown quote rate may print as
/// a confident `0.00`. Those all get a muted dash, and the PnL sort orders them the same way.
fn pnl_cell(e: &AssetEntry) -> MoonDataCell {
    let Some(v) = super::columns::pnl_display(e) else {
        return MoonDataCell::text("–").tone(MoonTone::Muted);
    };
    // `> 0.0` for the sign, not `>= 0.0`: a short resting exactly at its entry produces `-0.0`,
    // which passes `>= 0.0` and would render as `+-0.00`.
    let text = if v > 0.0 {
        format!("+{v:.2}")
    } else if v < 0.0 {
        format!("{v:.2}")
    } else {
        "0.00".to_string()
    };
    let tone = if v < 0.0 {
        MoonTone::Danger
    } else {
        MoonTone::Positive
    };
    // Two decimals without `$`, matching the Orders PnL column — this is a currency amount, not an
    // adaptive price.
    MoonDataCell::text(text).tone(tone).weight(500.0)
}

/// Render a muted core-name cell that targets the row's core when clicked.
///
/// Classic toggles the retained one-core filter back to All on a repeated click. Auto ignores the
/// shortcut because only the Shell rail selects a workspace core. The coin context menu belongs to
/// the ticker cell rather than this one.
fn core_cell(
    e: &AssetEntry,
    view: &Entity<AssetsView>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    let core = e.core;
    let view = view.clone();
    div()
        .id(SharedString::from(format!(
            "asset-core-cell-{core}-{}",
            e.row.market
        )))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        // Inherit font family and size from the MoonUI cell style fixed in `9a33dbf`.
        .text_color(rgb(MoonTone::Muted.color(p)))
        .child(e.core_name.clone())
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| this.filter_to_core(core, cx));
        })
}

/// Render the base-coin ticker cell, opening its chart on Main with the row's core when clicked.
///
/// This follows the Orders ticker behavior. `on_sale` adds a blue Info-tone `SELL` badge when the
/// coin has an active sell order.
fn coin_cell(
    e: &AssetEntry,
    view: &Entity<AssetsView>,
    p: MoonPalette,
    on_sale: bool,
) -> impl IntoElement + 'static {
    let coin = e.row.coin.clone();
    let core = e.core;
    let market = e.row.market.clone();
    let view = view.clone();
    let view_menu = view.clone();
    let coin_menu = coin.clone();
    let market_menu = market.clone();
    // Use an `assets/coins` icon when available; omit both icon and reserved space otherwise because
    // the narrow ticker column relies on table alignment.
    let icon = crate::media::coin_icons::coin_icon(&coin);
    div()
        .id(SharedString::from(format!(
            "asset-coin-{core}-{}",
            e.row.market
        )))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .when_some(icon, |el, tex| {
            el.child(img(tex).w(px(14.0)).h(px(14.0)).flex_none())
        })
        // Inherit ticker font family and size from the MoonUI cell style fixed in `9a33dbf`.
        .text_color(rgb(MoonTone::Accent.color(p)))
        .font_weight(FontWeight::MEDIUM)
        .child(coin)
        .when(on_sale, |el| {
            // Keep the `SELL` badge smaller than the ticker with MoonText's default size of 9.
            el.child(
                MoonText::new("SELL")
                    .color(MoonTone::Info.color(p))
                    .line_height(14.0)
                    .weight(600.0)
                    .mono(true)
                    .uppercase(false)
                    .render(),
            )
        })
        .on_click(move |_, _window, app| {
            if market.is_empty() {
                return; // A balance-only row has no market to open.
            }
            view.update(app, |this, cx| {
                let workspace_group = match &this.scope {
                    AssetsScope::Group(group) => Some(group.as_str()),
                    AssetsScope::All => None,
                };
                this.backend.update(cx, |b, bcx| {
                    if b.open_on_main_if_authorized(workspace_group, (core, market.clone()), false)
                    {
                        bcx.notify();
                    }
                });
            });
        })
        // Right-click opens the shared navigation and scoped-core blacklist menu; balance rows
        // provide no strategy or order context.
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                open_asset_coin_menu(
                    core,
                    market_menu.clone(),
                    coin_menu.clone(),
                    &view_menu,
                    e.position,
                    window,
                    app,
                );
            },
        )
}

/// Render Market Sell and the placeholder Order action for a sellable asset row.
///
/// The cell is populated for an open position or positive spot balance only when its resolved
/// market exists on the core. The Order button remains a stub for a future order-settings window.
fn actions_cell(
    e: &AssetEntry,
    view: &Entity<AssetsView>,
    _p: MoonPalette,
    is_position: bool,
) -> MoonDataCell {
    // An open position or positive spot balance is sellable only when its `<coin><quote>` market
    // actually exists on the core. For example, a USDC account may have no USDTUSDC market.
    let size = if e.row.qty.abs() > 0.0 {
        e.row.qty.abs()
    } else {
        e.row.qty_full.abs()
    };
    let sellable = is_position || size > 0.0;
    if !sellable || e.row.market.is_empty() || !e.market_exists {
        return MoonDataCell::text(String::new());
    }
    let core = e.core;
    let market = e.row.market.clone();
    let msell_id = SharedString::from(format!("asset-msell-{core}-{market}"));
    let order_id = SharedString::from(format!("asset-order-{core}-{market}"));
    let view_ms = view.clone();
    let market_ms = market.clone();
    let coin_ms = e.row.coin.clone();
    let el = h_flex()
        .w_full()
        .h_full()
        .items_center()
        .justify_end()
        .gap(px(4.0))
        .child(
            MoonButton::new(msell_id)
                .label(t!("assets.market_sell").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Danger)
                .on_click(move |_, window, app| {
                    // Require confirmation before the irreversible market close; only the dialog's
                    // Yes button submits the sale.
                    open_market_sell_confirm(
                        view_ms.clone(),
                        core,
                        market_ms.clone(),
                        is_position,
                        size,
                        coin_ms.clone(),
                        window,
                        app,
                    );
                })
                .render(),
        )
        .child(
            MoonButton::new(order_id)
                .label(t!("assets.order").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Soft)
                .on_click(move |_, _w, _app| {
                    // Placeholder for a future order-settings window.
                    log::info!(
                        "assets: order button (stub) core={} market={market}",
                        moon_core::feed::core_label(core)
                    );
                })
                .render(),
        );
    MoonDataCell::element(el)
}

/// Decide whether a confirmation captured for one core still has dispatch authority.
///
/// Args:
///     scope: Persistent host authority; the global Assets window is deliberately unrestricted.
///     effective_core_ids: Group panel's current Classic or Auto scope at dispatch time.
///     core: Core captured when the confirmation dialog opened.
///
/// Returns:
///     `true` for global Assets or while the captured core remains in the live group scope.
fn market_sell_core_is_authorized(
    scope: &AssetsScope,
    effective_core_ids: Option<&[CoreId]>,
    core: CoreId,
) -> bool {
    matches!(scope, AssetsScope::All)
        || effective_core_ids.is_some_and(|cores| cores.contains(&core))
}

/// Open the Market Sell confirmation dialog directly from the button click's mutable app context.
///
/// Only the Yes button submits the irreversible action: `market_sell_position` closes a position,
/// while `market_sell_token` sells a spot balance. A group-owned dialog revalidates its captured
/// core against the current effective workspace scope immediately before either command.
///
/// Args:
///     view: Assets entity retaining host scope and Backend authority.
///     core: Core captured from the rendered row.
///     market: Resolved market submitted on confirmation.
///     is_position: Whether to close a position instead of selling a spot balance.
///     size: Spot quantity used only when `is_position` is false.
///     coin: Display token interpolated into the confirmation question.
///     window: Window that owns the unique dialog and refusal notification.
///     app: Application context used to build the dialog.
///
/// Returns:
///     Nothing; stale group authority closes with a visible warning and sends no command.
#[allow(clippy::too_many_arguments)]
fn open_market_sell_confirm(
    view: Entity<AssetsView>,
    core: CoreId,
    market: String,
    is_position: bool,
    size: f64,
    coin: String,
    window: &mut Window,
    app: &mut App,
) {
    window.open_unique_moon_dialog(
        "assets-market-sell-confirm",
        app,
        move |dialog, _window, cx| {
            let p = MoonPalette::active(cx);
            let confirm_view = view.clone();
            let market_c = market.clone();
            let question = t!("assets.market_sell_q", coin = coin.clone()).to_string();
            dialog
                .w(px(320.0))
                .close_button(true)
                .overlay(true)
                .overlay_closable(true)
                .bg(rgb(p.shell_high))
                .border_color(rgb(p.border))
                .rounded(design::r_container(cx))
                .text_color(rgb(p.text))
                .header(
                    div()
                        .w_full()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("assets.market_sell_confirm").to_string()),
                )
                .content(move |content, _window, cx| {
                    let p = MoonPalette::active(cx);
                    content.child(
                        div()
                            .font_family(design::mono())
                            .text_size(design::t_body(cx))
                            .text_color(rgb(p.text))
                            .child(question.clone()),
                    )
                })
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            MoonButton::new("assets-msell-no")
                                .outline()
                                .size(MoonButtonSize::Action)
                                .label(format!("  {}  ", t!("dialogs.no")))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                })
                                .render(),
                        )
                        .child(
                            MoonButton::new("assets-msell-yes")
                                .size(MoonButtonSize::Action)
                                .variant(MoonButtonVariant::Danger)
                                .label(format!("  {}  ", t!("dialogs.yes")))
                                .on_click(move |_, window, cx| {
                                    let authorized = confirm_view.update(cx, |this, cx| {
                                        let b = this.backend.read(cx);
                                        let effective_scope = this.effective_scope(b);
                                        if !market_sell_core_is_authorized(
                                            &this.scope,
                                            effective_scope.as_ref().map(|scope| scope.ids()),
                                            core,
                                        ) {
                                            return false;
                                        }
                                        // Close a position at market, or sell the remaining spot token.
                                        let res = if is_position {
                                            b.session.market_sell_position(core, market_c.clone())
                                        } else {
                                            b.session.market_sell_token(
                                                core,
                                                market_c.clone(),
                                                size,
                                            )
                                        };
                                        if let Err(err) = res {
                                            log::warn!(
                                                "assets market sell {market_c} failed: {err:#}"
                                            );
                                        }
                                        cx.notify();
                                        true
                                    });
                                    if !authorized {
                                        window.push_notification(
                                            MoonNotification::warning(
                                                t!("assets.market_sell_scope_changed").to_string(),
                                            ),
                                            cx,
                                        );
                                    }
                                    window.close_dialog(cx);
                                })
                                .render(),
                        ),
                )
        },
    );
}

#[cfg(test)]
mod tests;
