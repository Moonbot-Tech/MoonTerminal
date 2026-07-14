//! Экспорт отчёта в файл: CSV (текст, `;`-разделитель, UTF-8 BOM — дружит с Excel)
//! и Excel (.xlsx, настоящая книга через `rust_xlsxwriter`). Выборка — СВОЙ запрос к
//! SQLite по ТЕКУЩЕМУ фильтру панели (период «Сегодня»/«С:–По:», ядра, монета,
//! сторона, эмулятор) и текущей сортировке — без капа таблицы `MAX_REPORT_ROWS`
//! (панель показывает топ-100, экспорт пишет всё до [`EXPORT_MAX_ROWS`]).
//! Значения — СЫРЫЕ, как лежат в БД (isshort/emulator = 0/1, числа без
//! знако-форматирования, без локализованных подписей): файл — машиночитаемая
//! выгрузка, а не скриншот таблицы. Единственное исключение — колонки дат: unix-
//! секунды в файле бесполезны, пишем "YYYY-MM-DD HH:MM" (UTC, как в таблице).

use std::path::{Path, PathBuf};

use rusqlite::types::Value;

use super::columns;
use moon_core::db::{self, ReportFilter};

/// Предохранитель от гигантских выборок: экспорт читает всё по фильтру, но не больше
/// этого количества строк (БД годами копит сотни тысяч записей; усечение — в лог).
const EXPORT_MAX_ROWS: usize = 200_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Csv,
    Xlsx,
}

impl Format {
    pub(super) fn ext(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }
}

/// Имя файла по умолчанию: период фильтра в имени (`report_2026-07-13.csv`,
/// `report_2026-07-01_2026-07-10.xlsx`, без дат — `report_all.csv`). При выгрузке ПО
/// ВСЕМ колонкам БД добавляем суффикс `-All` (`report_2026-07-13-All.csv`) — чтобы на
/// диске сразу отличать полный отчёт от урезанного до видимых колонок.
pub(super) fn suggested_name(filter: &ReportFilter, fmt: Format, all_cols: bool) -> String {
    let day = |secs: i64| {
        let full = db::fmt_unix(secs);
        full.get(..10).unwrap_or(&full).to_string()
    };
    let range = match (filter.date_from, filter.date_to) {
        (Some(f), Some(t)) => format!("{}_{}", day(f), day(t)),
        (Some(f), None) => day(f),
        (None, Some(t)) => format!("_{}", day(t)),
        (None, None) => "all".to_string(),
    };
    let all = if all_cols { "-All" } else { "" };
    format!("report_{range}{all}.{}", fmt.ext())
}

/// Стартовая папка диалога сохранения: профиль пользователя, иначе текущая.
pub(super) fn default_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Выполнить экспорт (зовётся с background executor — БД и запись файла блокирующие).
/// `cols` — экспортируемые колонки в порядке таблицы. Возвращает число строк данных.
pub(super) fn run(
    path: &Path,
    fmt: Format,
    cols: &[String],
    filter: &ReportFilter,
    sort_key: &str,
    sort_desc: bool,
) -> anyhow::Result<usize> {
    let conn = db::open_reader()
        .ok_or_else(|| anyhow::anyhow!("БД отчётов недоступна (нет файла или занята)"))?;
    let table = db::query_reports(&conn, filter, sort_key, sort_desc, EXPORT_MAX_ROWS);
    if table.rows.len() == EXPORT_MAX_ROWS {
        log::warn!("отчёт: экспорт упёрся в кап {EXPORT_MAX_ROWS} строк — выборка усечена");
    }
    // Индексы экспортируемых колонок в строках выборки (порядок — как в `cols`).
    let idx: Vec<(usize, &str)> = cols
        .iter()
        .filter_map(|name| {
            table
                .cols
                .iter()
                .position(|c| c == name)
                .map(|i| (i, name.as_str()))
        })
        .collect();
    if idx.is_empty() {
        anyhow::bail!("нет колонок для экспорта");
    }
    match fmt {
        Format::Csv => write_csv(path, &idx, &table.rows)?,
        Format::Xlsx => write_xlsx(path, &idx, &table.rows)?,
    }
    Ok(table.rows.len())
}

/// CSV: `;`-разделитель (Excel в русской локали читает его без мастера импорта),
/// UTF-8 BOM (кириллица в Excel), CRLF. Поля с `;`/кавычками/переносами — в кавычках.
fn write_csv(path: &Path, idx: &[(usize, &str)], rows: &[Vec<Value>]) -> anyhow::Result<()> {
    let mut out = String::with_capacity(64 * (rows.len() + 1));
    out.push('\u{FEFF}');
    let header: Vec<String> = idx
        .iter()
        .map(|(_, name)| csv_field(&columns::header_for(name)))
        .collect();
    out.push_str(&header.join(";"));
    out.push_str("\r\n");
    for row in rows {
        let mut first = true;
        for (i, name) in idx {
            if !first {
                out.push(';');
            }
            first = false;
            let val = row.get(*i).unwrap_or(&Value::Null);
            out.push_str(&csv_field(&field_text(name, val)));
        }
        out.push_str("\r\n");
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Колонки-даты: в БД unix-секунды, в файл — читаемый "YYYY-MM-DD HH:MM" (UTC).
fn date_col(col: &str) -> bool {
    matches!(
        col,
        "buydate" | "closedate" | "sellsetdate" | "last_update_at"
    )
}

/// Колонки-идентификаторы: большие целые (i64), которые Excel при записи ЧИСЛОМ
/// показывает экспонентой (`-3,06221E+18`) и теряет точность. Это ID, а не суммы —
/// пишем ТЕКСТОМ, чтобы значение читалось полностью и не округлялось.
fn id_text_col(col: &str) -> bool {
    matches!(
        col,
        "strategyid" | "exorderid" | "taskid" | "newrecid"
    )
}

/// Текст поля для CSV: даты — форматированные, остальное — сырое значение БД
/// (без локализации: isshort/emulator остаются 0/1, числа — `Display` как есть,
/// NULL — пустое поле). Блобов в отчёте нет.
fn field_text(col: &str, v: &Value) -> String {
    if date_col(col) {
        return match v {
            Value::Integer(i) => db::fmt_unix(*i),
            Value::Real(r) => db::fmt_unix(*r as i64),
            _ => String::new(),
        };
    }
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => r.to_string(),
        Value::Text(t) => t.clone(),
        Value::Blob(_) => String::new(),
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// XLSX: заголовок жирным, значения — сырые типы БД (Integer/Real → число, Text →
/// строка, NULL — пустая ячейка); даты — форматированной строкой. Автоширина колонок.
fn write_xlsx(path: &Path, idx: &[(usize, &str)], rows: &[Vec<Value>]) -> anyhow::Result<()> {
    use rust_xlsxwriter::{Format as XFormat, Workbook};

    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let bold = XFormat::new().set_bold();
    for (c, (_, name)) in idx.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, columns::header_for(name), &bold)?;
    }
    for (r, row) in rows.iter().enumerate() {
        let r = (r + 1) as u32;
        for (c, (i, name)) in idx.iter().enumerate() {
            let c = c as u16;
            let val = row.get(*i).unwrap_or(&Value::Null);
            if date_col(name) {
                let text = field_text(name, val);
                if !text.is_empty() {
                    ws.write_string(r, c, text)?;
                }
                continue;
            }
            // ID-поля — текстом (иначе Excel рвёт большие i64 в экспоненту).
            if id_text_col(name) {
                if let Value::Integer(n) = val {
                    ws.write_string(r, c, n.to_string())?;
                    continue;
                }
            }
            match val {
                Value::Integer(n) => {
                    ws.write_number(r, c, *n as f64)?;
                }
                Value::Real(n) => {
                    ws.write_number(r, c, *n)?;
                }
                Value::Text(t) => {
                    ws.write_string(r, c, t)?;
                }
                Value::Null | Value::Blob(_) => {}
            }
        }
    }
    ws.autofit();
    wb.save(path)?;
    Ok(())
}
