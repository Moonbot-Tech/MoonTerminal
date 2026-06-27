//! Таблица панели «Ордера»: колонки, строки/ячейки, клик по токену, тогл стопов.

use super::*;
use moon_core::feed::OrderStopKind;
use moon_core::session::CoreId;
use moon_ui::{MoonBadge, MoonBadgeSize, MoonBadgeVariant};
use rust_i18n::t;
use std::collections::HashSet;

pub(super) fn orders_table(
    rows: Rc<Vec<OrderEntry>>,
    columns: u16,
    state: &Entity<MoonDataTableState>,
    highlight: Rc<HashSet<(CoreId, u64)>>,
    cx: &Context<OrdersPanel>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let view = cx.entity();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);
    // Выделение строки/ячейки кликом нам не нужно (фронт форка ставит его жёстко: `select_row`
    // выставляет и `selected_cell`) — сбрасываем ВСЕ три поля сразу после клика. `selected(...)`
    // ниже используем ТОЛЬКО для подсветки монет, открытых в Main.
    let state_reset = state.clone();
    // Видимые колонки в каноничном порядке — общий список для header и строк. Drag-перестановку
    // (`state.column_order`) применяет сам MoonDataTable: и к шапке, и к ячейкам тела.
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
            order_table_row(&table_rows[ix], &view, p, &row_cols, &highlight)
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

/// Переводимый/отраслевой заголовок колонки. Core/Side/Token/Cur.P идут через словарь
/// `orders.col.*`; Size/SL/TS/Vstop/Buy/Fill/Strat — отраслевые токены, намеренно НЕ
/// переводим (см. locales/README.md). Общий для header и меню выбора полей.
pub(super) fn col_title(col: OrdCol) -> String {
    match col {
        OrdCol::Core => t!("orders.col.core").to_string(),
        OrdCol::Side => t!("orders.col.side").to_string(),
        OrdCol::Token => t!("orders.col.token").to_string(),
        OrdCol::CurP => t!("orders.col.price").to_string(),
        OrdCol::Status => "Status".to_string(),
        OrdCol::Size => "Size".to_string(),
        OrdCol::Sl => "SL".to_string(),
        OrdCol::Ts => "TS".to_string(),
        OrdCol::Vstop => "Vstop".to_string(),
        OrdCol::Buy => "Buy".to_string(),
        OrdCol::Fill => "Fill".to_string(),
        OrdCol::Pnl => "PNL".to_string(),
        OrdCol::Tp => "TP".to_string(),
        OrdCol::Strat => "Strat".to_string(),
    }
}

/// Схема колонки: ключ/ширина/выравнивание. Порядок задаётся `OrdCol::ALL`. Ширина —
/// логические px (минимум на узкой таблице, пропорциональный вес на широкой).
fn column_def(col: OrdCol) -> MoonDataTableColumn {
    let title = col_title(col);
    match col {
        OrdCol::Core => MoonDataTableColumn::new("core", title, 90.0),
        OrdCol::Side => MoonDataTableColumn::new("side", title, 82.0),
        OrdCol::Status => MoonDataTableColumn::new("status", title, 76.0),
        OrdCol::Token => numeric_column("token", title, 70.0),
        OrdCol::Size => numeric_column("size", title, 70.0),
        OrdCol::Sl => MoonDataTableColumn::new("sl", title, 46.0),
        OrdCol::Ts => MoonDataTableColumn::new("ts", title, 46.0),
        OrdCol::Vstop => MoonDataTableColumn::new("vstop", title, 56.0),
        OrdCol::Buy => numeric_column("buy", title, 80.0),
        OrdCol::CurP => numeric_column("cur.p", title, 86.0),
        OrdCol::Fill => numeric_column("fill", title, 56.0),
        OrdCol::Pnl => numeric_column("pnl", title, 72.0),
        OrdCol::Tp => numeric_column("tp", title, 80.0),
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
) -> MoonDataRow {
    MoonDataRow::new(
        cols.iter()
            .map(|c| cell_for(*c, e, view, p))
            .collect::<Vec<_>>(),
    )
    // Подсветка ОДНОЙ строки на каждую Main-открытую (монета+ядро) — первый её ордер.
    .selected(highlight.contains(&(e.core, e.row.uid)))
}

/// Ячейка для одной колонки строки. Порядок ячеек ДОЛЖЕН совпадать с `column_def` по тем
/// же видимым колонкам — оба идут по одному списку `cols`.
fn cell_for(
    col: OrdCol,
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
) -> MoonDataCell {
    let r = &e.row;
    match col {
        OrdCol::Core => MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
        OrdCol::Side => {
            let (side, tone) = side_label(r);
            MoonDataCell::text(side).tone(tone).weight(500.0)
        }
        OrdCol::Status => MoonDataCell::element(status_cell(r, p)),
        OrdCol::Token => MoonDataCell::element(token_cell(e, view, p)),
        OrdCol::Size => MoonDataCell::text(num(r.size)),
        OrdCol::Sl => flag_toggle_cell(e, view, OrderStopKind::StopLoss, r.sl_on, p),
        OrdCol::Ts => flag_toggle_cell(e, view, OrderStopKind::Trailing, r.ts_on, p),
        OrdCol::Vstop => flag_toggle_cell(e, view, OrderStopKind::VStop, r.vstop_on, p),
        OrdCol::Buy => MoonDataCell::text(num(r.buy_price)),
        OrdCol::CurP => MoonDataCell::text(num(r.price as f64)),
        OrdCol::Fill => MoonDataCell::text(format!("{:.0}%", r.fill_pct)).tone(MoonTone::Muted),
        OrdCol::Pnl => pnl_cell(r),
        OrdCol::Tp => tp_cell(r),
        OrdCol::Strat => MoonDataCell::text(r.strat.clone()).tone(MoonTone::Muted),
    }
}

/// Отображаемая сторона и её тон. Цвет = «вход исполнен» (синий, `Info`) vs «ждёт вход»
/// (оранжевый, `Negative`); метка различает направление и фазу:
/// - BUY — лонг/спот, вход (buy) ещё не исполнен;
/// - SELL — лонг исполнен → нога выхода (sell);
/// - Short-S — шорт, pending вход (sell-to-open);
/// - Short-B — шорт исполнен → нога выхода (buy-to-close).
/// Эмулятор → суффикс `(E)`.
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

/// Статус ордера (baseline status-badge): `filled` — исполнен (серый), `pending` —
/// ждёт условие (амбер), `live` — рабочий/частично исполнен (зелёный).
fn order_status(r: &OrderRow) -> (&'static str, MoonTone) {
    if r.fill_pct >= 99.95 {
        ("filled", MoonTone::Muted)
    } else if r.pending {
        ("pending", MoonTone::Warning)
    } else {
        ("live", MoonTone::Positive)
    }
}

/// Ячейка статуса — `MoonBadge` Outline/Status (пилюля как в `5Badges`/`8tablecells`).
fn status_cell(r: &OrderRow, p: MoonPalette) -> impl IntoElement + 'static {
    let (label, tone) = order_status(r);
    div().h_full().flex().items_center().child(
        MoonBadge::new(label)
            .tone(tone)
            .variant(MoonBadgeVariant::Outline)
            .size(MoonBadgeSize::Status)
            .render_with_palette(p),
    )
}

/// Локальная оценка нереализованного PnL по исполненной части позиции:
/// `(mark − entry) · filled_qty · dir`. Серверного PnL в `OrderRow` нет (как и в
/// «Активах» — считаем сами). `None`, если позиции нет (нет исполнения) или входные
/// цены не выставлены. Для шорта вход — `sell_price`, для лонга — `buy_price`.
fn order_pnl(r: &OrderRow) -> Option<f64> {
    let filled_qty = r.size * (r.fill_pct as f64) / 100.0;
    if filled_qty <= 0.0 {
        return None;
    }
    let entry = if r.is_short {
        r.sell_price
    } else {
        r.buy_price
    };
    let mark = r.price as f64;
    if entry <= 0.0 || mark <= 0.0 {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((mark - entry) * filled_qty * dir)
}

/// PnL-ячейка: colored delta (зелёный/красный, со знаком), `–` если позиции нет.
fn pnl_cell(r: &OrderRow) -> MoonDataCell {
    match order_pnl(r) {
        Some(v) => {
            let tone = if v >= 0.0 {
                MoonTone::Positive
            } else {
                MoonTone::Danger
            };
            let text = if v >= 0.0 {
                format!("+{}", num(v))
            } else {
                num(v)
            };
            MoonDataCell::text(text).tone(tone).weight(500.0)
        }
        None => MoonDataCell::text("–").tone(MoonTone::Muted),
    }
}

/// TP-ячейка: take-profit синим (`Info`/tp trader-cell), `–` если не выставлен.
fn tp_cell(r: &OrderRow) -> MoonDataCell {
    match r.take_profit {
        Some(v) if v > 0.0 => MoonDataCell::text(num(v)).tone(MoonTone::Info),
        _ => MoonDataCell::text("–").tone(MoonTone::Muted),
    }
}

/// Кликабельный флаг стопа (SL/TS/Vstop): ON — зелёным, OFF — тускло. Клик шлёт ядру
/// `set_order_stop` (включить/выключить ИНВЕРСИЕЙ текущего флага) для ЭТОГО ордера —
/// уровень стопа сохраняется feed-слоем при повторном включении.
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
        .child(
            MoonText::new(label)
                .color(tone.color(p))
                .font_size(10.5)
                .line_height(14.0)
                .weight(500.0)
                .mono(true)
                .uppercase(false)
                .render(),
        )
        .on_click(move |_, _window, app| {
            log::info!(
                "orders UI click toggle stop core={core} uid={uid} kind={kind:?} on={on} -> {}",
                !on
            );
            view.update(app, |this, cx| {
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
        // Кликабельна ВСЯ ячейка (а не только текст токена) — по узкому тикеру в одну
        // букву иначе сложно попасть. `.right()` колонки → прижимаем содержимое вправо.
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .justify_end()
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
