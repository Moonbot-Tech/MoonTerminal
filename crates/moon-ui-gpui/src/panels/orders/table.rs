//! Таблица панели «Ордера»: колонки, строки/ячейки, клик по токену, тогл стопов.

use super::*;
use moon_core::feed::OrderStopKind;
use moon_core::session::CoreId;
use rust_i18n::t;
use std::collections::HashSet;

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

/// Переводимый/отраслевой заголовок колонки. Core/Side/Token/Cur.P идут через словарь
/// `orders.col.*`; Size/SL/TS/Vstop/Buy/Fill/Strat — отраслевые токены, намеренно НЕ
/// переводим (см. locales/README.md). Общий для header и меню выбора полей.
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
        OrdCol::Fill => "Fill".to_string(),
        OrdCol::Pnl => "PNL".to_string(),
        OrdCol::PnlPct => "PNL %".to_string(),
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
        OrdCol::Token => numeric_column("token", title, 70.0),
        OrdCol::Size => numeric_column("size", title, 70.0),
        OrdCol::Sl => MoonDataTableColumn::new("sl", title, 46.0),
        OrdCol::Ts => MoonDataTableColumn::new("ts", title, 46.0),
        OrdCol::Vstop => MoonDataTableColumn::new("vstop", title, 56.0),
        OrdCol::Buy => numeric_column("buy", title, 80.0),
        OrdCol::CurP => numeric_column("cur.p", title, 86.0),
        OrdCol::Fill => numeric_column("fill", title, 56.0),
        OrdCol::Pnl => numeric_column("pnl", title, 72.0),
        OrdCol::PnlPct => numeric_column("pnl.pct", title, 64.0),
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
    // Подсветка ОДНОЙ строки на каждую Main-открытую (монета+ядро) — первый её ордер.
    .selected(highlight.contains(&(e.core, e.row.uid)))
}

/// Тег вида стопа для ключей оверлея (синхронно с feed-слоем: SL=0, TS=1, VStop=2).
pub(super) fn stop_tag(kind: OrderStopKind) -> u8 {
    match kind {
        OrderStopKind::StopLoss => 0,
        OrderStopKind::Trailing => 1,
        OrderStopKind::VStop => 2,
    }
}

/// Ячейка для одной колонки строки. Порядок ячеек ДОЛЖЕН совпадать с `column_def` по тем
/// же видимым колонкам — оба идут по одному списку `cols`.
fn cell_for(
    col: OrdCol,
    e: &OrderEntry,
    view: &Entity<OrdersPanel>,
    p: MoonPalette,
    stop_overlay: &std::collections::HashMap<(CoreId, u64, u8), bool>,
) -> MoonDataCell {
    let r = &e.row;
    // Оптимистичный тогл: свежий клик (<3с) рисуется сразу, не дожидаясь строк от feed.
    let flag = |kind: OrderStopKind, baked: bool| -> bool {
        stop_overlay
            .get(&(e.core, r.uid, stop_tag(kind)))
            .copied()
            .unwrap_or(baked)
    };
    match col {
        OrdCol::Core => MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
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
        OrdCol::Fill => MoonDataCell::text(format!("{:.0}%", r.fill_pct)).tone(MoonTone::Muted),
        OrdCol::Pnl => pnl_cell(r),
        OrdCol::PnlPct => pnl_pct_cell(r),
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

/// Локальная оценка нереализованного PnL по исполненной части позиции:
/// `(mark − entry) · filled_qty · dir`. Серверного PnL в `OrderRow` нет (как и в
/// «Активах» — считаем сами). `None`, если позиции нет (нет исполнения) или входная
/// цена не выставлена.
///
/// Вход = `buy_price` для ОБОИХ направлений. У MoonBot входная нога всегда `buy_order`
/// (фазы «Buy*»=вход / «Sell*»=выход) — и у лонга, и у шорта; после филла ядро кладёт
/// в `buy_price` среднюю цену позиции (`pos_price`, ровно то, что берут «Активы»).
/// `sell_price` — это ВЫХОДНАЯ нога/цель (для шорта — цель профита НИЖЕ входа), брать её
/// как «вход» шорта было багом: PnL считался от цены выхода, а не входа, и расходился с
/// «Активами» (напр. VELVET: −3.96 от sell_price против ≈0 от pos_price).
/// Количество в позиции для расчёта PnL. В позиции (вход исполнен ЛИБО активна выходная
/// нога — ордер продажи из удерживаемого актива, где `fill_pct=0`) берём остаток выходной
/// ноги (`remaining_size`), иначе исполненную часть входа (`size·fill_pct`). `None` — нет
/// позиции (гейтил `fill_pct=0` для listing-sell/MoonHook → PnL показывался «–»).
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

fn order_pnl(r: &OrderRow) -> Option<f64> {
    let qty = position_qty(r)?;
    let entry = r.buy_price;
    let mark = r.price as f64;
    if entry <= 0.0 || mark <= 0.0 {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((mark - entry) * qty * dir)
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
            // PnL округляем до сотых (валютная величина), а не adaptive-формат как у цен.
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

/// PnL в процентах от входа: направленное движение цены `(mark − entry)/entry · dir · 100`.
/// `None` по тем же условиям, что и [`order_pnl`] (нет исполнения / нет входной цены).
/// Вход = `buy_price` для обоих направлений (см. [`order_pnl`]).
fn order_pnl_pct(r: &OrderRow) -> Option<f64> {
    position_qty(r)?; // тот же гейт «в позиции», что и у order_pnl
    let entry = r.buy_price;
    let mark = r.price as f64;
    if entry <= 0.0 || mark <= 0.0 {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((mark - entry) / entry * dir * 100.0)
}

/// PnL%-ячейка: colored delta (зелёный/красный, со знаком, до сотых), `–` если позиции нет.
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

/// Кликабельный флаг стопа (SL/TS/Vstop) — ЭФФЕКТИВНОЕ положение («сработает ли»).
/// `on` уже вычислен feed-слоем (convert.rs) по модели MoonBot: явный override терминала →
/// иначе `per-order флаг ИЛИ стоп стратегии` (у ордера может не быть своего стопа — тогда
/// действует стратегийный, на проводе per-order поля пустые). «ON» зелёным; «OFF» — для SL
/// красным (позиция без стоп-лосса — риск), для TS/Vstop тускло.
/// Клик тогает эффективное состояние (`set_order_stop` инверсией), уровень стопа
/// восстанавливается feed-слоем (память/стратегия/дефолт) при повторном включении.
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
                // Оптимизм: рисуем целевое состояние сразу; истина сервера (строки от
                // feed) перекроет оверлей при расхождении (запись живёт ≤3с).
                this.stop_overlay
                    .insert((core, uid, stop_tag(kind)), (!on, std::time::Instant::now()));
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

/// Ячейка типа ордера (BUY/SELL/Short-S/Short-B) — кликабельна: открывает окно
/// редактирования ордера (порт MoonBot «Active Order» dialog).
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
        .child(
            MoonText::new(side)
                .color(tone.color(p))
                .font_size(10.5)
                .line_height(14.0)
                .weight(500.0)
                .mono(true)
                .uppercase(false)
                .render(),
        )
        .on_click(move |_, window, app| {
            let backend = view.read(app).backend.clone();
            crate::panels::open_order_edit(backend, core, uid, window, app);
        })
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
