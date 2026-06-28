//! Колонки/ячейки/заголовки таблицы «Отчёт»: построение колонок, форматирование
//! значений БД в текст+цвет, человекочитаемые заголовки и ширины.

use super::*;
use rust_i18n::t;

pub(super) fn report_columns(table: &ReportTable, vis: &[usize]) -> Vec<MoonDataTableColumn> {
    vis.iter()
        .map(|&i| {
            let col = table.cols[i].as_str();
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

pub(super) fn report_data_row(
    ri: usize,
    table: &ReportTable,
    vis: &[usize],
    backend: &Entity<Backend>,
    p: MoonPalette,
) -> MoonDataRow {
    let mut cells = Vec::with_capacity(vis.len());
    if let Some(r) = table.rows.get(ri) {
        let core_uid = table.core_uids.get(ri).copied().unwrap_or(0);
        for &i in vis {
            let cname = table.cols[i].as_str();
            let val = r.get(i).unwrap_or(&Value::Null);
            if cname == "coin" {
                cells.push(coin_cell(ri, val, core_uid, backend, p));
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
    backend: &Entity<Backend>,
    p: MoonPalette,
) -> MoonDataCell {
    let coin = value_to_string(val);
    let backend = backend.clone();
    let el = div()
        .id(SharedString::from(format!("rep-coin-{ri}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            MoonText::new(coin.clone())
                .color(MoonTone::Accent.color(p))
                .font_size(10.0)
                .line_height(13.0)
                .mono(true)
                .uppercase(false)
                .render(),
        )
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
        });
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
    // Уже полный рынок (кончается на quote ядра) → берём как есть.
    let already_full = !quote.is_empty() && upper.len() > quote.len() && upper.ends_with(&quote);
    let candidate = if already_full || quote.is_empty() {
        coin.to_string()
    } else {
        format!("{coin}{quote}")
    };
    // Если снимок ядра доступен — подтверждаем кандидата по юниверсу, иначе ищем рынок,
    // чья база совпадает с монетой (на случай префиксов вроде `1000PEPEUSDT`).
    let universe = b.session.market_source().search_markets(core, coin, 32);
    if universe.is_empty() || universe.iter().any(|m| m == &candidate) {
        return candidate;
    }
    universe
        .iter()
        .find(|m| {
            let q = moon_core::symbol::resolve_quote(m);
            moon_core::symbol::base_symbol(m, &q) == coin
        })
        .cloned()
        .unwrap_or(candidate)
}

fn report_data_cell(col: &str, val: &Value, p: MoonPalette) -> MoonDataCell {
    let (text, color) = cell(col, val, p);
    // Клиппируем форматированный content по реальной ширине колонки. Выравнивание — как
    // у колонки, а сам MoonDataTable дополнительно защищает границы ячейки на уровне
    // контейнера.
    let right = is_numeric_report_column(col);
    let color = color.unwrap_or_else(|| MoonTone::Default.color(p));
    let inner = div()
        .flex()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .when(right, |d| d.justify_end())
        .child(
            MoonText::new(text)
                .color(color)
                .font_size(10.0)
                .line_height(13.0)
                .mono(true)
                .uppercase(false)
                .render(),
        );
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
            | "db_id"
            | "taskid"
    ) || col.ends_with("delta")
        || col.ends_with("ratio")
}

/// Текст + цвет ячейки по имени колонки и значению (порт `cell`).
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
            (n.map(|x| format!("{x:+.6}")).unwrap_or_default(), color)
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
/// показывает, что реально приходит в отчёт.
pub(super) fn header_for(col: &str) -> String {
    col.to_string()
}

fn width_for(col: &str) -> f32 {
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
