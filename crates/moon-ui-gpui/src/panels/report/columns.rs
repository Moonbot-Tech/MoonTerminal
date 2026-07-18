//! Колонки/ячейки/заголовки таблицы «Отчёт»: построение колонок, форматирование
//! значений БД в текст+цвет, человекочитаемые заголовки и ширины.

use super::*;
use crate::controls::{CoinMenuCtx, CoinMenuOrigin};
use rust_i18n::t;

/// Build sortable table descriptors for visible indices in the cached schema.
pub(super) fn report_columns(cols: &[String], vis: &[usize]) -> Vec<MoonDataTableColumn> {
    vis.iter()
        .map(|&i| {
            let col = cols[i].as_str();
            let column = MoonDataTableColumn::new(col.to_string(), header_for(col), width_for(col))
                .sortable(true);
            if is_numeric_report_column(col) {
                column.right()
            } else {
                column
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
/// Build one report row; absent values render as NULL and coin/core cells keep their actions.
pub(super) fn report_data_row(
    ri: usize,
    cols: &[String],
    data: &ReportData,
    vis: &[usize],
    selected_cores: &Rc<Vec<u64>>,
    backend: &Entity<Backend>,
    view: &Entity<ReportPanel>,
    p: MoonPalette,
) -> MoonDataRow {
    let mut cells = Vec::with_capacity(vis.len());
    if let Some(r) = data.rows.get(ri) {
        let core_uid = data.core_uids.get(ri).copied().unwrap_or(0);
        // Стратегия сделки — колонка `strategyid` (может быть невидима, читаем по имени из
        // ВСЕХ колонок). 0/отсутствует = ручная/без стратегии → секция стратегии в меню скрыта.
        let strat_id = cols
            .iter()
            .position(|c| c == "strategyid")
            .and_then(|idx| r.get(idx))
            .and_then(|v| match v {
                Value::Integer(i) => Some(*i as u64),
                _ => None,
            })
            .filter(|id| *id != 0);
        for &i in vis {
            let cname = cols[i].as_str();
            let val = r.get(i).unwrap_or(&Value::Null);
            if cname == "coin" {
                cells.push(coin_cell(
                    ri,
                    val,
                    core_uid,
                    strat_id,
                    selected_cores.clone(),
                    backend,
                    p,
                ));
            } else if cname == "core_name" {
                cells.push(core_cell(ri, val, core_uid, view, p));
            } else {
                cells.push(report_data_cell(cname, val, p));
            }
        }
    }
    MoonDataRow::new(cells)
}

/// Ячейка монеты в «Отчёте»: кликабельна целиком (акцентным цветом — намёк), клик
/// открывает чарт монеты НА ЯДРЕ сделки (`core_uid`) — как клик по токену в «Ордерах».
/// Окно Main НЕ поднимаем (`open_request_activate = false`), как в Ордерах/Детектах.
fn coin_cell(
    ri: usize,
    val: &Value,
    core_uid: u64,
    strat_id: Option<u64>,
    selected_cores: Rc<Vec<u64>>,
    backend: &Entity<Backend>,
    p: MoonPalette,
) -> MoonDataCell {
    let coin = value_to_string(val);
    let backend = backend.clone();
    let backend_menu = backend.clone();
    let coin_menu = coin.clone();
    let el = div()
        .id(SharedString::from(format!("rep-coin-{ri}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        // Кегль/шрифт наследуются от стиля ячейки (каскад moonui, фикс `9a33dbf`).
        .text_color(rgb(MoonTone::Accent.color(p)))
        .child(coin.clone())
        .on_click(move |_, _window, app| {
            if coin.is_empty() {
                return;
            }
            // В БД отчёта монета хранится по-разному: одни ядра пишут базу (`M`), другие —
            // полный рынок (`VINEUSDT`). Чарту нужен ИМЕННО полный ключ рынка ядра, иначе
            // подписка не находит рынок → пустой график. Восстанавливаем его по quote ядра
            // и его market-юниверсу.
            let market = backend.read(app);
            let market = resolve_market(market, core_uid, &coin);
            backend.update(app, |b, bcx| {
                b.open_request = Some((core_uid, market.clone()));
                b.open_request_rev = b.open_request_rev.wrapping_add(1);
                b.open_request_activate = false;
                bcx.notify();
            });
        })
        // ПКМ — единое контекстное меню монеты. Стратегия сделки известна (`strategyid`) →
        // доступна и «В ЧС стратегии». «Выбранные ядра» = фильтр ядер отчёта.
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                if coin_menu.is_empty() {
                    return;
                }
                app.stop_propagation();
                let market = {
                    let b = backend_menu.read(app);
                    resolve_market(b, core_uid, &coin_menu)
                };
                let coin_base = moon_core::symbol::coin_of_market(&market).to_string();
                let (core_name, strat_name) = {
                    let b = backend_menu.read(app);
                    let core_name = b
                        .session
                        .sessions()
                        .iter()
                        .find(|s| s.id == core_uid)
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    let strat_name = strat_id.and_then(|sid| {
                        b.session
                            .store()
                            .core(core_uid)
                            .and_then(|cd| cd.strategies.iter().find(|s| s.id == sid))
                            .map(|s| s.name.clone())
                    });
                    (core_name, strat_name)
                };
                let ctx = CoinMenuCtx {
                    core: core_uid,
                    core_name,
                    market,
                    coin: coin_base,
                    selected_cores: (*selected_cores).clone(),
                    strat_id,
                    strat_name,
                    order_uid: None,
                    side: None,
                    short: false,
                    origin: CoinMenuOrigin::OrderTable,
                };
                crate::controls::open_coin_menu(ctx, backend_menu.clone(), e.position, window, app);
            },
        );
    MoonDataCell::element(el)
}

/// Полный ключ рынка ядра по сохранённой в отчёте монете. `coin` может быть базой
/// (`M`) или уже полным рынком (`MUSDT`). Достраиваем quote ядра (как Ордера/Детекты)
/// и, если доступен снимок, сверяемся с реальным market-юниверсом ядра.
fn resolve_market(b: &Backend, core: u64, coin: &str) -> String {
    let quote = b
        .config
        .servers
        .iter()
        .find(|s| s.id == core)
        .map(|s| moon_core::symbol::resolve_quote(&s.market))
        .unwrap_or_default();
    let upper = coin.to_ascii_uppercase();
    // Уже полный рынок: кончается на quote ядра ИЛИ содержит dex-префикс HIP-3 (`xyz:BIRD`) →
    // берём как есть (достраивать quote нельзя — HL/HIP-3 не несут суффикса в имени).
    let already_full = moon_core::symbol::is_hip3(coin)
        || (!quote.is_empty() && upper.len() > quote.len() && upper.ends_with(&quote));
    let candidate = if already_full || quote.is_empty() {
        coin.to_string()
    } else {
        format!("{coin}{quote}")
    };
    // Если снимок ядра доступен — подтверждаем кандидата по юниверсу, иначе ищем рынок,
    // чья база совпадает с монетой (префиксы вроде `1000PEPEUSDT` и dex-перпы `xyz:BIRD`).
    let universe = b.session.market_source().search_markets(core, coin, 32);
    if universe.is_empty() || universe.iter().any(|m| m == &candidate) {
        return candidate;
    }
    universe
        .iter()
        .find(|m| moon_core::symbol::coin_of_market(m).eq_ignore_ascii_case(coin))
        .cloned()
        .unwrap_or(candidate)
}

/// Ячейка «Ядро» в «Отчёте»: цвет как в Ордерах/Активах (тон Muted), кликом ставит фильтр
/// ТОЛЬКО на это ядро (повторный клик по нему же — сброс на «все»).
fn core_cell(
    ri: usize,
    val: &Value,
    core_uid: u64,
    view: &Entity<ReportPanel>,
    p: MoonPalette,
) -> MoonDataCell {
    let name = value_to_string(val);
    let view = view.clone();
    let el = div()
        .id(SharedString::from(format!("rep-core-{ri}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(rgb(MoonTone::Muted.color(p)))
        .child(name)
        .on_click(move |_, _window, app| {
            view.update(app, |t, c| t.filter_to_core(core_uid, c));
        });
    MoonDataCell::element(el)
}

fn report_data_cell(col: &str, val: &Value, p: MoonPalette) -> MoonDataCell {
    let (text, color) = cell(col, val, p);
    // Клиппируем форматированный content по реальной ширине колонки. Выравнивание — как
    // у колонки, а сам MoonDataTable дополнительно защищает границы ячейки на уровне
    // контейнера. Кегль/шрифт — от стиля ячейки (каскад moonui, фикс `9a33dbf`).
    let right = is_numeric_report_column(col);
    let color = color.unwrap_or_else(|| MoonTone::Default.color(p));
    let inner = div()
        .flex()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .when(right, |d| d.justify_end())
        .text_color(rgb(color))
        .child(text);
    MoonDataCell::element(inner)
}

fn is_numeric_report_column(col: &str) -> bool {
    matches!(
        col,
        "quantity"
            | "boughtq"
            | "buyprice"
            | "sellprice"
            | "spentbtc"
            | "gainedbtc"
            | "profitbtc"
            | "lev"
            | "id"
            | "newrecid"
            | "taskid"
    ) || col.ends_with("delta")
        || col.ends_with("ratio")
}

/// Текст + цвет ячейки по имени колонки и значению (порт `cell`). Только для показа
/// в таблице — экспорт (export.rs) пишет СЫРЫЕ значения БД, без этого форматирования.
fn cell(col: &str, v: &Value, p: MoonPalette) -> (String, Option<u32>) {
    match col {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => {
            (as_i64(v).map(db::fmt_unix).unwrap_or_default(), None)
        }
        "isshort" => match as_i64(v) {
            Some(1) => (t!("report.side.short").to_string(), Some(p.red)),
            Some(0) => (t!("report.side.long").to_string(), Some(p.green)),
            _ => (String::new(), Some(p.text_soft)),
        },
        "emulator" => match as_i64(v) {
            Some(1) => (t!("report.cell.emu").to_string(), Some(p.text_soft)),
            _ => (String::new(), None),
        },
        "profitbtc" | "gainedbtc" => {
            let n = as_f64(v);
            let color = match n {
                Some(x) if x > 0.0 => Some(p.green),
                Some(x) if x < 0.0 => Some(p.red),
                _ => None,
            };
            // Profit cells use two decimals without a currency marker; the
            // totals row owns the marker for the aggregate.
            (n.map(|x| format!("{x:+.2}")).unwrap_or_default(), color)
        }
        _ => (value_to_string(v), None),
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(*i),
        Value::Real(r) => Some(*r as i64),
        _ => None,
    }
}
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Real(r) => Some(*r),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => moon_core::util::fmt::compact(*r, 8),
        Value::Text(t) => t.clone(),
        Value::Blob(_) => "<blob>".into(),
    }
}

/// Заголовок колонки = ИМЯ колонки БД как есть, БЕЗ i18n. Единообразно с
/// авто-добавленными полями ядра (дельты/dmark/…), нейтрально к языку и сразу
/// показывает, что реально приходит в отчёт. Исключение — легаси-поля Moonbot с
/// хвостом «btc» (`profitbtc`/`spentbtc`/`gainedbtc`): суффикс исторический, суммы
/// деноминированы в котировке пары (usdt/usdc/…), а не в BTC — на не-BTC паре «btc»
/// путает. Показываем нейтральные `profit`/`spent`/`gained` (валюта зависит от строки).
pub(super) fn header_for(col: &str) -> String {
    match col {
        "profitbtc" => "profit".to_string(),
        "spentbtc" => "spent".to_string(),
        "gainedbtc" => "gained".to_string(),
        _ => col.to_string(),
    }
}

pub(super) fn width_for(col: &str) -> f32 {
    match col {
        "buydate" | "closedate" => 120.0,
        "sellsetdate" | "last_update_at" => 116.0,
        "comment" => 280.0,
        "sellreason" => 170.0,
        "channelname" | "signaltype" | "fname" | "exorderid" => 110.0,
        "core_name" | "coin" => 88.0,
        "profitbtc" | "gainedbtc" | "spentbtc" => 96.0,
        "lev" | "isshort" | "emulator" => 52.0,
        _ => 82.0,
    }
}
