//! Таблица панели «Ордера»: колонки, строки/ячейки, клик по токену.

use super::*;
use rust_i18n::t;

pub(super) fn orders_table(
    rows: Rc<Vec<OrderEntry>>,
    cx: &Context<OrdersPanel>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let view = cx.entity();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);

    crate::panels::common::data_table_host(
        "orders-table-host",
        empty,
        t!("orders.empty").to_string(),
        p,
        cx,
        MoonDataTable::new("orders-table", row_count, move |ix, _window, _app| {
            order_table_row(&table_rows[ix], &view, p)
        })
        .columns(order_columns())
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H),
    )
}

fn order_columns() -> Vec<MoonDataTableColumn> {
    // Колонки и их порядок — как в оригинале (egui): Core · Side · Token · Size ·
    // SL · TS · Vstop · Buy · Cur.P · Fill · Strat. Ширина — логические px: минимум,
    // когда таблица узкая, и пропорциональный вес, когда есть лишняя ширина.
    vec![
        MoonDataTableColumn::new("core", "Core", 90.0),
        MoonDataTableColumn::new("side", "Side", 60.0),
        numeric_column("Token", 70.0),
        numeric_column("Size", 70.0),
        MoonDataTableColumn::new("sl", "SL", 46.0),
        MoonDataTableColumn::new("ts", "TS", 46.0),
        MoonDataTableColumn::new("vstop", "Vstop", 56.0),
        numeric_column("Buy", 80.0),
        numeric_column("Cur.P", 86.0),
        numeric_column("Fill", 56.0),
        numeric_column("Strat", 90.0),
    ]
}

fn numeric_column(title: impl Into<SharedString>, width: f32) -> MoonDataTableColumn {
    let title = title.into();
    MoonDataTableColumn::new(title.to_lowercase(), title, width).right()
}

fn order_table_row(e: &OrderEntry, view: &Entity<OrdersPanel>, p: MoonPalette) -> MoonDataRow {
    let r = &e.row;
    // SELL (исполненный лонг) — синим, SHORT — красным, BUY (ждёт) — зелёным; (E) — эмулятор.
    let (side, side_tone) = if is_sell(r) {
        ("SELL", MoonTone::Info)
    } else if r.is_short {
        ("SHORT", MoonTone::Danger)
    } else {
        ("BUY", MoonTone::Positive)
    };
    let side = if r.emulator {
        format!("{side}(E)")
    } else {
        side.to_string()
    };

    MoonDataRow::new([
        MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
        MoonDataCell::text(side).tone(side_tone).weight(500.0),
        MoonDataCell::element(token_cell(e, view, p)),
        MoonDataCell::text(num(r.size)),
        flag_cell(r.sl_on),
        flag_cell(r.ts_on),
        flag_cell(r.vstop_on),
        MoonDataCell::text(num(r.buy_price)),
        MoonDataCell::text(num(r.price as f64)),
        MoonDataCell::text(format!("{:.0}%", r.fill_pct)).tone(MoonTone::Muted),
        MoonDataCell::text(r.strat.clone()).tone(MoonTone::Muted),
    ])
}

/// Флаг ON/OFF (SL/TS/Vstop): ON — зелёным, OFF — тускло (порт `cell_onoff`).
fn flag_cell(on: bool) -> MoonDataCell {
    if on {
        MoonDataCell::text("ON").tone(MoonTone::Positive)
    } else {
        MoonDataCell::text("OFF").tone(MoonTone::Muted)
    }
}

/// Ячейка токена (без quote: `ADAUSDT` → `ADA`), акцентом — намёк, что кликабельна.
/// Клик открывает чарт монеты на Main НА ЯДРЕ ордера (порт клика по строке egui).
fn token_cell(
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    let token = symbol::base_symbol(&e.row.market, &e.quote).to_string();
    let core = e.core;
    let market = e.row.market.clone();
    let uid = e.row.uid;
    let view = view.clone();

    div()
        .id(SharedString::from(format!("ord-tok-{core}-{uid}")))
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            MoonText::new(token)
                .color(MoonTone::Accent.color(p))
                .font_size(10.5)
                .line_height(14.0)
                .weight(500.0)
                .mono(true)
                .uppercase(false)
                .render(),
        )
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| {
                this.backend.update(cx, |b, bcx| {
                    b.open_request = Some((core, market.clone()));
                    b.open_request_rev = b.open_request_rev.wrapping_add(1);
                    // Клик в Ордерах открывает монету на Main, но окно НЕ поднимает.
                    b.open_request_activate = false;
                    bcx.notify();
                });
            });
        })
}

/// Открытые ордера всех ядер группы — для статус-бара Shell (число ордеров).
pub fn count_orders(b: &Backend, group: &str) -> usize {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .map(|c| c.orders.len())
        .sum()
}
