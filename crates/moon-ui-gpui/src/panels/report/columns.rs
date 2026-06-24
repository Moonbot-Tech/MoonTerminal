//! Колонки/ячейки/заголовки таблицы «Отчёт»: построение колонок, форматирование
//! значений БД в текст+цвет, человекочитаемые заголовки и ширины.

use super::*;
use rust_i18n::t;

pub(super) fn report_columns(vis: &[usize]) -> Vec<MoonDataTableColumn> {
    vis.iter()
        .map(|&i| {
            let col = db::DISPLAY_COLUMNS[i];
            let column =
                MoonDataTableColumn::new(col, header_for(col), width_for(col)).sortable(true);
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
    p: MoonPalette,
) -> MoonDataRow {
    let mut cells = Vec::with_capacity(vis.len());
    if let Some(r) = table.rows.get(ri) {
        for &i in vis {
            let cname = table.cols[i];
            let val = r.get(i).unwrap_or(&Value::Null);
            cells.push(report_data_cell(cname, val, p));
        }
    }
    MoonDataRow::new(cells)
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
    )
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

/// Человекочитаемый заголовок колонки (порт `header_for`).
pub(super) fn header_for(col: &str) -> String {
    match col {
        "buydate" => t!("report.col.buydate").to_string(),
        "closedate" => t!("report.col.closedate").to_string(),
        "sellsetdate" => "Sell set".to_string(),
        "last_update_at" => t!("report.col.last_update").to_string(),
        "core_name" => t!("report.col.core").to_string(),
        "db_id" => "ID".to_string(),
        "taskid" => "TaskID".to_string(),
        "exorderid" => "ExOrderID".to_string(),
        "coin" => t!("report.col.coin").to_string(),
        "isshort" => t!("report.col.side").to_string(),
        "quantity" => t!("report.col.quantity").to_string(),
        "boughtq" => t!("report.col.bought").to_string(),
        "buyprice" => t!("report.col.buyprice").to_string(),
        "sellprice" => t!("report.col.sellprice").to_string(),
        "spentbtc" => t!("report.col.spentbtc").to_string(),
        "gainedbtc" => t!("report.col.gainedbtc").to_string(),
        "profitbtc" => t!("report.col.profitbtc").to_string(),
        "lev" => t!("report.col.lev").to_string(),
        "strategyid" => "Strat".to_string(),
        "channelname" => t!("report.col.channel").to_string(),
        "signaltype" => t!("report.col.signal").to_string(),
        "fname" => t!("report.col.file").to_string(),
        "basecurrency" => "BaseCur".to_string(),
        "emulator" => t!("report.col.emulator").to_string(),
        "status" => t!("report.col.status").to_string(),
        "sellreason" => t!("report.col.reason").to_string(),
        "comment" => t!("report.col.comment").to_string(),
        other => other.to_string(),
    }
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
