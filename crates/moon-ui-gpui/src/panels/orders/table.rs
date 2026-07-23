//! Orders panel table: columns, rows and cells, token actions, and stop toggles.

use super::*;
use crate::controls::{CoinMenuCtx, CoinMenuOrigin};
use moon_core::feed::OrderStopKind;
use moon_core::session::CoreId;
use rust_i18n::t;
use std::collections::HashSet;

/// Open the shared coin context menu for an order row from its token or strategy cell.
///
/// The menu's selected cores come from the panel's `sel_cores` filter: an empty set expands to all
/// cores in the group, while a non-empty set is passed through directly. The selected-cores
/// blacklist entry appears only when that resulting list contains more than one core.
#[allow(clippy::too_many_arguments)]
fn open_row_coin_menu(
    core: CoreId,
    market: String,
    coin: String,
    uid: u64,
    strat_id: Option<u64>,
    strat_name: Option<String>,
    short: bool,
    view: &Entity<OrdersPanel>,
    pos: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    app.stop_propagation();
    let (backend, group, sel_cores) = {
        let panel = view.read(app);
        (
            panel.backend.clone(),
            panel.group.clone(),
            panel.sel_cores.clone(),
        )
    };
    let (selected_cores, core_name) = {
        let b = backend.read(app);
        let sessions = b.session.sessions();
        let selected: Vec<CoreId> = if sel_cores.is_empty() {
            sessions
                .iter()
                .filter(|s| s.group == group)
                .map(|s| s.id)
                .collect()
        } else {
            sel_cores.iter().copied().collect()
        };
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
        strat_id,
        strat_name,
        order_uid: Some(uid),
        // A table row is not a specific chart leg, so its order section offers Edit and Cancel.
        side: None,
        short,
        origin: CoinMenuOrigin::OrderTable,
    };
    crate::controls::open_coin_menu(ctx, backend, pos, window, app);
}

pub(super) fn orders_table(
    rows: Rc<Vec<OrderEntry>>,
    columns: u16,
    state: &Entity<MoonDataTableState>,
    highlight: Rc<HashSet<(CoreId, u64)>>,
    stop_overlay: Rc<std::collections::HashMap<(CoreId, u64, u8), bool>>,
    cx: &Context<OrdersPanel>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let view = cx.entity();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);
    // Click selection is not needed. The fork frontend couples `select_row` to `selected_cell`, so
    // clear all three selection fields immediately after a click. The `selected(...)` row flag
    // below is reserved for highlighting markets open on Main.
    let state_reset = state.clone();
    // Share one canonically ordered visible-column list between the header and rows. MoonDataTable
    // applies drag reordering from `state.column_order` to both header and body cells.
    let visible: Rc<Vec<OrdCol>> = Rc::new(
        OrdCol::ALL
            .iter()
            .copied()
            .filter(|c| columns & c.bit() != 0)
            .collect(),
    );
    let row_cols = visible.clone();

    crate::panels::common::data_table_host(
        "orders-table-host",
        empty,
        t!("orders.empty").to_string(),
        p,
        cx,
        MoonDataTable::new("orders-table", row_count, move |ix, _window, _app| {
            order_table_row(
                &table_rows[ix],
                &view,
                p,
                &row_cols,
                &highlight,
                &stop_overlay,
            )
        })
        .columns(visible.iter().map(|c| column_def(*c)).collect::<Vec<_>>())
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H)
        .on_select_row(move |_ix, _window, app| {
            state_reset.update(app, |s, c| {
                s.selected_row = None;
                s.selected_column = None;
                s.selected_cell = None;
                c.notify();
            });
        }),
    )
}

/// Return the column title shared by the table header and field-selection menu.
///
/// `Core`, `Side`, `Token`, `CurP`, `TpPrice`, and `PnlTp` use `orders.col.*` localization. Size,
/// SL, TS, Vstop, Buy, Fill, PNL, PNL %, and Strat are intentionally untranslated domain tokens;
/// see `locales/README.md`.
pub(super) fn col_title(col: OrdCol) -> String {
    match col {
        OrdCol::Core => t!("orders.col.core").to_string(),
        OrdCol::Side => t!("orders.col.side").to_string(),
        OrdCol::Token => t!("orders.col.token").to_string(),
        OrdCol::CurP => t!("orders.col.price").to_string(),
        OrdCol::Size => "Size".to_string(),
        OrdCol::Sl => "SL".to_string(),
        OrdCol::Ts => "TS".to_string(),
        OrdCol::Vstop => "Vstop".to_string(),
        OrdCol::Buy => "Buy".to_string(),
        OrdCol::TpPrice => t!("orders.col.tp_price").to_string(),
        OrdCol::Fill => "Fill".to_string(),
        OrdCol::Pnl => "PNL".to_string(),
        OrdCol::PnlPct => "PNL %".to_string(),
        OrdCol::PnlTp => t!("orders.col.pnl_tp").to_string(),
        OrdCol::Strat => "Strat".to_string(),
    }
}

/// Build a column's key, logical-pixel width, and alignment.
///
/// [`OrdCol::ALL`] defines canonical order. MoonDataTable treats the declared width as the base
/// width and auto-layout weight. A narrow viewport may shrink it proportionally toward the shared
/// 40px column floor, so the declared value is not a strict minimum.
fn column_def(col: OrdCol) -> MoonDataTableColumn {
    let title = col_title(col);
    match col {
        OrdCol::Core => MoonDataTableColumn::new("core", title, 90.0),
        OrdCol::Side => MoonDataTableColumn::new("side", title, 82.0),
        OrdCol::Token => numeric_column("token", title, 70.0),
        OrdCol::Size => numeric_column("size", title, 70.0),
        OrdCol::Sl => MoonDataTableColumn::new("sl", title, 46.0),
        OrdCol::Ts => MoonDataTableColumn::new("ts", title, 46.0),
        OrdCol::Vstop => MoonDataTableColumn::new("vstop", title, 56.0),
        OrdCol::Buy => numeric_column("buy", title, 80.0),
        OrdCol::CurP => numeric_column("cur.p", title, 86.0),
        OrdCol::TpPrice => numeric_column("tp.p", title, 86.0),
        OrdCol::Fill => numeric_column("fill", title, 56.0),
        OrdCol::Pnl => numeric_column("pnl", title, 72.0),
        OrdCol::PnlPct => numeric_column("pnl.pct", title, 64.0),
        OrdCol::PnlTp => numeric_column("pnl.tp", title, 72.0),
        OrdCol::Strat => numeric_column("strat", title, 90.0),
    }
}

fn numeric_column(
    key: impl Into<SharedString>,
    title: impl Into<SharedString>,
    width: f32,
) -> MoonDataTableColumn {
    MoonDataTableColumn::new(key, title, width).right()
}

fn order_table_row(
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
    cols: &[OrdCol],
    highlight: &HashSet<(CoreId, u64)>,
    stop_overlay: &std::collections::HashMap<(CoreId, u64, u8), bool>,
) -> MoonDataRow {
    MoonDataRow::new(
        cols.iter()
            .map(|c| cell_for(*c, e, view, p, stop_overlay))
            .collect::<Vec<_>>(),
    )
    // Highlight one row per core-market pair open on Main: its first order in the base ordering.
    .selected(highlight.contains(&(e.core, e.row.uid)))
}

/// Map a stop kind to an optimistic-overlay tag, mirroring the feed layer's mapping.
///
/// The stable mapping is SL = 0, TS = 1, and VStop = 2.
pub(super) fn stop_tag(kind: OrderStopKind) -> u8 {
    match kind {
        OrderStopKind::StopLoss => 0,
        OrderStopKind::Trailing => 1,
        OrderStopKind::VStop => 2,
    }
}

/// Build one cell for a row column.
///
/// Cell order must match [`column_def`]; both consume the same visible `cols` list. Element cells
/// leave font size, line height, and monospace styling to the cascading Moon UI cell style
/// introduced by fix `9a33dbf`.
fn cell_for(
    col: OrdCol,
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
    stop_overlay: &std::collections::HashMap<(CoreId, u64, u8), bool>,
) -> MoonDataCell {
    let r = &e.row;
    // Render a recent click optimistically for up to three seconds without waiting for feed rows.
    let flag = |kind: OrderStopKind, baked: bool| -> bool {
        stop_overlay
            .get(&(e.core, r.uid, stop_tag(kind)))
            .copied()
            .unwrap_or(baked)
    };
    match col {
        OrdCol::Core => MoonDataCell::element(core_cell(e, view, p)),
        OrdCol::Side => MoonDataCell::element(side_cell(e, view, p)),
        OrdCol::Token => MoonDataCell::element(token_cell(e, view, p)),
        OrdCol::Size => MoonDataCell::text(num(r.size)),
        OrdCol::Sl => flag_toggle_cell(
            e,
            view,
            OrderStopKind::StopLoss,
            flag(OrderStopKind::StopLoss, r.sl_on),
            p,
        ),
        OrdCol::Ts => flag_toggle_cell(
            e,
            view,
            OrderStopKind::Trailing,
            flag(OrderStopKind::Trailing, r.ts_on),
            p,
        ),
        OrdCol::Vstop => flag_toggle_cell(
            e,
            view,
            OrderStopKind::VStop,
            flag(OrderStopKind::VStop, r.vstop_on),
            p,
        ),
        OrdCol::Buy => MoonDataCell::text(num(r.buy_price)),
        OrdCol::CurP => MoonDataCell::text(num(r.price as f64)),
        OrdCol::TpPrice => {
            if r.sell_price > 0.0 {
                MoonDataCell::text(num(r.sell_price))
            } else {
                MoonDataCell::text("–").tone(MoonTone::Muted)
            }
        }
        OrdCol::Fill => MoonDataCell::text(format!("{:.0}%", r.fill_pct)).tone(MoonTone::Muted),
        OrdCol::Pnl => pnl_cell(r),
        OrdCol::PnlPct => pnl_pct_cell(r),
        OrdCol::PnlTp => pnl_tp_cell(r),
        OrdCol::Strat => strat_cell(e, view, p),
    }
}

/// Return the displayed order side and tone.
///
/// An executed entry uses blue `Info`, while an entry still waiting uses orange `Negative`. The
/// label distinguishes direction and lifecycle phase:
///
/// - `BUY`: long or spot buy entry is not yet executed;
/// - `SELL`: long entry is executed and the sell exit leg is active;
/// - `Short-S`: short sell-to-open entry is pending;
/// - `Short-B`: short entry is executed and the buy-to-close exit leg is active.
///
/// Emulated orders add the `(E)` suffix.
fn side_label(r: &OrderRow) -> (String, MoonTone) {
    let (side, tone) = match (r.is_short, executed(r)) {
        (false, false) => ("BUY", MoonTone::Negative),
        (false, true) => ("SELL", MoonTone::Info),
        (true, false) => ("Short-S", MoonTone::Negative),
        (true, true) => ("Short-B", MoonTone::Info),
    };
    let side = if r.emulator {
        format!("{side}(E)")
    } else {
        side.to_string()
    };
    (side, tone)
}

/// Return the position quantity used by the PnL calculations.
///
/// When `filled` indicates an executed entry or active exit leg, use `remaining_size` when positive
/// and otherwise the original `size`. This preserves positions such as a sale from an already-held
/// asset whose `fill_pct` is zero. Before that state, use the filled entry quantity
/// (`size * fill_pct`). Return `None` when the resulting quantity is not positive.
fn position_qty(r: &OrderRow) -> Option<f64> {
    let qty = if r.filled {
        if r.remaining_size > 0.0 {
            r.remaining_size
        } else {
            r.size
        }
    } else {
        r.size * (r.fill_pct as f64) / 100.0
    };
    (qty > 0.0).then_some(qty)
}

/// Estimate unrealized PnL locally as `(mark - entry) * position_qty * direction`.
///
/// [`OrderRow`] carries no server PnL, so this table calculates it just as the Assets panel does.
/// Return `None` when there is no position or either entry or mark price is unavailable.
///
/// `OrderRow::buy_price` is the normalized entry for both directions, and Moonbot models
/// `buy_order` as the entry leg for both long and short lifecycle phases. After execution, the raw
/// core snapshot stores a break-even price including commission in `buy_price`; the feed converter
/// replaces it with the average position price (`pos_price`) before constructing `OrderRow`.
/// `sell_price` is the exit target; for a profitable short it lies below entry. Treating it as a
/// short entry previously calculated PnL from the exit price and diverged from Assets, for example
/// VELVET showed -3.96 from `sell_price` versus about zero from `pos_price`.
pub(super) fn order_pnl(r: &OrderRow) -> Option<f64> {
    let qty = position_qty(r)?;
    let entry = r.buy_price;
    let mark = r.price as f64;
    if entry <= 0.0 || mark <= 0.0 {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((mark - entry) * qty * dir)
}

/// Estimate PnL if the position closes at its take-profit price (`sell_price`).
///
/// This uses the same formula as [`order_pnl`] with the take target in place of the current mark,
/// which shows the expected profit from a split grid of sell orders. Return `None` without a
/// position, entry price, or take-profit price.
fn order_pnl_at_tp(r: &OrderRow) -> Option<f64> {
    let qty = position_qty(r)?;
    let entry = r.buy_price;
    let tp = r.sell_price;
    if entry <= 0.0 || tp <= 0.0 {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((tp - entry) * qty * dir)
}

/// Build the PNL TP cell as a signed colored delta, or a dash without a position or target.
fn pnl_tp_cell(r: &OrderRow) -> MoonDataCell {
    match order_pnl_at_tp(r) {
        Some(v) => {
            let tone = if v >= 0.0 {
                MoonTone::Positive
            } else {
                MoonTone::Danger
            };
            let text = if v >= 0.0 {
                format!("+{v:.2}")
            } else {
                format!("{v:.2}")
            };
            MoonDataCell::text(text).tone(tone).weight(500.0)
        }
        None => MoonDataCell::text("–").tone(MoonTone::Muted),
    }
}

/// Build the PnL cell as a signed green or red delta, or a dash without a position.
fn pnl_cell(r: &OrderRow) -> MoonDataCell {
    match order_pnl(r) {
        Some(v) => {
            let tone = if v >= 0.0 {
                MoonTone::Positive
            } else {
                MoonTone::Danger
            };
            // Show two decimals without `$`; this is a currency amount, not an adaptive price.
            let text = if v >= 0.0 {
                format!("+{v:.2}")
            } else {
                format!("{v:.2}")
            };
            MoonDataCell::text(text).tone(tone).weight(500.0)
        }
        None => MoonDataCell::text("–").tone(MoonTone::Muted),
    }
}

/// Calculate directional PnL percentage as `(mark - entry) / entry * direction * 100`.
///
/// Return `None` under the same conditions as [`order_pnl`]. `buy_price` is the entry for both
/// directions.
fn order_pnl_pct(r: &OrderRow) -> Option<f64> {
    position_qty(r)?; // Apply the same in-position gate as `order_pnl`.
    let entry = r.buy_price;
    let mark = r.price as f64;
    if entry <= 0.0 || mark <= 0.0 {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((mark - entry) / entry * dir * 100.0)
}

/// Build the PnL-percent cell as a signed green or red value with two decimals, or a dash.
fn pnl_pct_cell(r: &OrderRow) -> MoonDataCell {
    match order_pnl_pct(r) {
        Some(v) => {
            let tone = if v >= 0.0 {
                MoonTone::Positive
            } else {
                MoonTone::Danger
            };
            let text = if v >= 0.0 {
                format!("+{v:.2}%")
            } else {
                format!("{v:.2}%")
            };
            MoonDataCell::text(text).tone(tone).weight(500.0)
        }
        None => MoonDataCell::text("–").tone(MoonTone::Muted),
    }
}

/// Build a clickable SL, TS, or VStop flag showing whether that protection is effectively active.
///
/// The feed layer derives each baked flag from Moonbot's model: a terminal override, otherwise the
/// per-order flag or inherited strategy stop. The `on` argument may temporarily replace that value
/// with a recent optimistic click. `ON` is green; `OFF` is red for SL because an unprotected
/// position is risky, and muted for TS or VStop.
///
/// Clicking sends the inverse effective state through `set_order_stop`. When re-enabled, the feed
/// layer restores the stop level from memory, strategy settings, or defaults.
fn flag_toggle_cell(
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    kind: OrderStopKind,
    on: bool,
    p: MoonPalette,
) -> MoonDataCell {
    let core = e.core;
    let uid = e.row.uid;
    let view = view.clone();
    let (label, tone) = if on {
        ("ON", MoonTone::Positive)
    } else if kind == OrderStopKind::StopLoss {
        ("OFF", MoonTone::Danger)
    } else {
        ("OFF", MoonTone::Muted)
    };
    let key = match kind {
        OrderStopKind::StopLoss => "sl",
        OrderStopKind::Trailing => "ts",
        OrderStopKind::VStop => "vs",
    };
    let el = div()
        .id(SharedString::from(format!("ord-{key}-{core}-{uid}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        // Inherit font size and family from the cascading Moon UI cell style; override only tone
        // and weight.
        .text_color(rgb(tone.color(p)))
        .font_weight(FontWeight::MEDIUM)
        .child(label)
        .on_click(move |_, _window, app| {
            log::info!(
                "orders UI click toggle stop core={core} uid={uid} kind={kind:?} on={on} -> {}",
                !on
            );
            view.update(app, |this, cx| {
                // Draw the target state immediately. Feed rows remain authoritative and replace a
                // disagreeing overlay after its lifetime of at most three seconds.
                this.stop_overlay.insert(
                    (core, uid, stop_tag(kind)),
                    (!on, std::time::Instant::now()),
                );
                this.backend.update(cx, |b, _| {
                    if let Err(err) = b.session.set_order_stop(core, uid, kind, !on) {
                        log::warn!(
                            "orders toggle stop failed core={core} uid={uid} kind={kind:?}: {err:#}"
                        );
                    }
                });
                cx.notify();
            });
        });
    MoonDataCell::element(el)
}

/// Build a clickable BUY, SELL, Short-S, or Short-B cell that opens the order editor.
///
/// The editor ports Moonbot's Active Order dialog.
fn side_cell(
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    let (side, tone) = side_label(&e.row);
    let core = e.core;
    let uid = e.row.uid;
    let view = view.clone();
    div()
        .id(SharedString::from(format!("ord-side-{core}-{uid}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(rgb(tone.color(p)))
        .font_weight(FontWeight::MEDIUM)
        .child(side)
        .on_click(move |_, window, app| {
            let backend = view.read(app).backend.clone();
            crate::panels::open_order_edit(backend, core, uid, window, app);
        })
}

/// Build the order's strategy cell.
///
/// A strategy order (`strat_id != 0`) is clickable and opens the Strategies window focused on that
/// core and strategy, clearing active-only mode before expanding and selecting it. A manual order
/// without a strategy renders as plain text.
fn strat_cell(e: &OrderEntry, view: &Entity<OrdersPanel>, p: MoonPalette) -> MoonDataCell {
    let r = &e.row;
    if r.strat_id == 0 {
        return MoonDataCell::text(r.strat.clone()).tone(MoonTone::Muted);
    }
    let core = e.core;
    let uid = r.uid;
    let strat_id = r.strat_id;
    let strat_name = r.strat.clone();
    let market_menu = r.market.clone();
    let coin_menu = symbol::coin_of_market(&r.market).to_string();
    let short = r.is_short;
    let view = view.clone();
    let view_menu = view.clone();
    let el = div()
        .id(SharedString::from(format!("ord-strat-{core}-{uid}")))
        // Make the full cell clickable, as in `token_cell`, and right-align its column content.
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .cursor_pointer()
        .text_color(rgb(MoonTone::Muted.color(p)))
        .font_weight(FontWeight::MEDIUM)
        .child(r.strat.clone())
        .on_click(move |_, window, app| {
            let backend = view.read(app).backend.clone();
            let owner_display = window.display(app).map(|d| d.id());
            crate::strategies::open_goto(
                backend,
                core,
                strat_id,
                Some(window.window_handle()),
                owner_display,
                app,
            );
        })
        // Right-click opens the shared coin menu with this order's known strategy context.
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                open_row_coin_menu(
                    core,
                    market_menu.clone(),
                    coin_menu.clone(),
                    uid,
                    Some(strat_id),
                    Some(strat_name.clone()),
                    short,
                    &view_menu,
                    e.position,
                    window,
                    app,
                );
            },
        );
    MoonDataCell::element(el)
}

/// Build the core-name cell as a quick single-core filter.
///
/// Clicking sets the panel filter to exactly this core. Clicking it again when it is the sole
/// selection clears the set back to All. The text retains the muted style of the former plain
/// `MoonDataCell::text` cell.
fn core_cell(
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    let core = e.core;
    let uid = e.row.uid;
    let view = view.clone();
    div()
        .id(SharedString::from(format!("ord-core-{core}-{uid}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(rgb(MoonTone::Muted.color(p)))
        .child(e.core_name.clone())
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| this.filter_to_core(core, cx));
        })
}

/// Build the clickable base-token cell, resolving the token from the display market name.
///
/// The quote is omitted (`ADAUSDT` becomes `ADA`) and accent styling signals interactivity.
/// Left-click opens the exact order market on Main for the order's core without activating the
/// Main window; right-click opens the shared coin context menu.
fn token_cell(
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    // Resolve from the display (`mb_classic`) name so `@206` becomes `UENA`, not the raw index.
    let token = symbol::coin_of_market(&e.row.market_display).to_string();
    let coin = token.clone();
    let core = e.core;
    let market = e.row.market.clone();
    let uid = e.row.uid;
    let strat_id = (e.row.strat_id != 0).then_some(e.row.strat_id);
    let strat_name = strat_id.map(|_| e.row.strat.clone());
    let short = e.row.is_short;
    let view = view.clone();
    let view_menu = view.clone();
    let market_menu = market.clone();

    div()
        .id(SharedString::from(format!("ord-tok-{core}-{uid}")))
        // Make the full cell clickable because a one-letter ticker is otherwise a narrow target.
        // The column is right-aligned, so align its content to the end as well.
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .cursor_pointer()
        .text_color(rgb(MoonTone::Accent.color(p)))
        .font_weight(FontWeight::MEDIUM)
        .child(token)
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| {
                this.backend.update(cx, |b, bcx| {
                    b.open_on_main((core, market.clone()), false);
                    bcx.notify();
                });
            });
        })
        // Right-click opens the shared coin menu for core, selected-core, and strategy blacklists,
        // strategy navigation, and order editing or cancellation.
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                open_row_coin_menu(
                    core,
                    market_menu.clone(),
                    coin.clone(),
                    uid,
                    strat_id,
                    strat_name.clone(),
                    short,
                    &view_menu,
                    e.position,
                    window,
                    app,
                );
            },
        )
}
