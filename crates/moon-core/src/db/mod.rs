//! Локальная SQLite-БД отчётов по закрытым ордерам — ПОЛНОЕ зеркало вашей
//! Postgres-таблицы `orders` (те же имена колонок). PK `(core_uid, db_id)` ≈ ваш
//! `(server_id, id)`: `db_id` — серверный row id; `core_uid` — наш СТАБИЛЬНЫЙ uid
//! ядра (`ServerConfig.uid`).
//!
//! Источники данных (терминал INSERT-SQL от ядра НЕ получает — есть только
//! close-report CmdId=31 в форме `update Orders set …`):
//!  - живая МОДЕЛЬ ордера (`snapshot().orders()` → `Order`, питается AllStatuses/
//!    OrderStatus) — монета, сторона, цены, объёмы, плечо, стратегия, taskid,
//!    даты open/sell-set/close (через feature `diagnostics`);
//!  - close-report SQL — финальные closedate/sellprice/profit/sellreason/comment.
//! Аналитические дельты (btc1hdelta, d5m, pump1h, signaltype, channel …) ядро
//! терминалу не присылает — колонки в схеме есть (полное зеркало), но NULL.
//!
//! Пишет ОДИН поток-writer; читает окно «Отчёты» отдельным соединением (WAL).

mod parse;

pub use parse::parse_report_sql;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::types::Value;
use rusqlite::Connection;

use crate::config::paths;

/// Один отчёт для записи (заполнены поля, доступные терминалу).
#[derive(Debug, Clone, Default)]
pub struct ReportRow {
    pub core_uid: u64,
    pub core_name: String,
    pub db_id: i64,
    pub taskid: Option<i64>,
    pub exorderid: Option<String>,
    pub coin: Option<String>,
    pub isshort: Option<bool>,
    pub buydate: Option<i64>,
    pub sellsetdate: Option<i64>,
    pub closedate: Option<i64>,
    pub quantity: Option<f64>,
    pub buyprice: Option<f64>,
    pub sellprice: Option<f64>,
    pub spentbtc: Option<f64>,
    pub gainedbtc: Option<f64>,
    pub profitbtc: Option<f64>,
    pub lev: Option<i64>,
    pub strategyid: Option<i64>,
    pub emulator: Option<bool>,
    pub status: Option<i64>,
    pub sellreason: Option<String>,
    pub comment: Option<String>,
    /// Универсальный passthrough: ВСЕ пары из close-SQL (lowercase-имя →
    /// значение). Writer сам заводит недостающие колонки и пишет их — новые поля
    /// ядра (дельты/MarkPriceDelta/…) попадают в отчёт без правок кода.
    pub extras: Vec<(String, Value)>,
    pub sql: String,
}

pub type ReportTx = Sender<ReportRow>;

/// Хэндл БД: канал записи + счётчик-генерация (растёт после КАЖДОЙ записи —
/// окно «Отчёты» по нему перезапрашивает данные без поллинга).
pub struct ReportsHandle {
    pub tx: ReportTx,
    pub generation: Arc<AtomicU64>,
}

use crate::util::now_unix_ms_i64 as now_ms;

/// Полный набор колонок (сверх core_uid/core_name/db_id/sql/created_ms/updated_ms),
/// зеркалящий Postgres `orders`. Используется и для CREATE, и для ALTER-апгрейда.
const ALL_DB_COLUMNS: &[(&str, &str)] = &[
    ("taskid", "INTEGER"),
    ("exorderid", "TEXT"),
    ("coin", "TEXT"),
    ("isshort", "INTEGER"),
    ("buydate", "INTEGER"),
    ("sellsetdate", "INTEGER"),
    ("closedate", "INTEGER"),
    ("quantity", "REAL"),
    ("boughtq", "REAL"),
    ("buyprice", "REAL"),
    ("sellprice", "REAL"),
    ("spentbtc", "REAL"),
    ("gainedbtc", "REAL"),
    ("profitbtc", "REAL"),
    ("lev", "INTEGER"),
    ("strategyid", "INTEGER"),
    ("source", "INTEGER"),
    ("channel", "INTEGER"),
    ("channelname", "TEXT"),
    ("signaltype", "TEXT"),
    ("fname", "TEXT"),
    ("basecurrency", "INTEGER"),
    ("emulator", "INTEGER"),
    ("status", "INTEGER"),
    ("sellreason", "TEXT"),
    ("comment", "TEXT"),
    ("deleted", "INTEGER"),
    ("imp", "INTEGER"),
    ("btc1hdelta", "REAL"),
    ("exchange1hdelta", "REAL"),
    ("btc24hdelta", "REAL"),
    ("exchange24hdelta", "REAL"),
    ("btc5mdelta", "REAL"),
    ("bvsvratio", "REAL"),
    ("pump1h", "REAL"),
    ("dump1h", "REAL"),
    ("d24h", "REAL"),
    ("d3h", "REAL"),
    ("d1h", "REAL"),
    ("d15m", "REAL"),
    ("d5m", "REAL"),
    ("d1m", "REAL"),
    ("dbtc1m", "REAL"),
    ("vd1m", "REAL"),
    ("pricebug", "REAL"),
    ("hvol", "REAL"),
    ("hvolf", "REAL"),
    ("dvol", "REAL"),
    ("takeprofitlag", "REAL"),
    ("last_update_at", "INTEGER"),
];

/// Колонки и порядок для отображения в окне «Отчёты» (плюс заголовок/ширина —
/// в самом окне). core_uid скрыт (служебный), db_id показываем как «ID».
pub const DISPLAY_COLUMNS: &[&str] = &[
    "buydate",
    "closedate",
    "sellsetdate",
    "core_name",
    "db_id",
    "taskid",
    "exorderid",
    "coin",
    "isshort",
    "quantity",
    "boughtq",
    "buyprice",
    "sellprice",
    "spentbtc",
    "gainedbtc",
    "profitbtc",
    "lev",
    "strategyid",
    "source",
    "channel",
    "channelname",
    "signaltype",
    "fname",
    "basecurrency",
    "emulator",
    "status",
    "sellreason",
    "comment",
    "btc1hdelta",
    "exchange1hdelta",
    "btc24hdelta",
    "exchange24hdelta",
    "btc5mdelta",
    "bvsvratio",
    "pump1h",
    "dump1h",
    "d24h",
    "d3h",
    "d1h",
    "d15m",
    "d5m",
    "d1m",
    "dbtc1m",
    "vd1m",
    "pricebug",
    "hvol",
    "hvolf",
    "dvol",
    "takeprofitlag",
    "last_update_at",
];

fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let _ = conn.busy_timeout(Duration::from_secs(3));

    conn.execute(
        "CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )?;

    // Очень старая схема по рантайм-`server_id` — пересоздаём под core_uid.
    let has_old: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('closed_sell_reports') WHERE name='server_id'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if has_old {
        conn.execute("DROP TABLE IF EXISTS closed_sell_reports", [])?;
        log::warn!("отчёты: старая схема (server_id) — таблица пересоздана");
    }

    // CREATE с полным набором колонок.
    let mut cols =
        String::from("core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, db_id INTEGER NOT NULL");
    for (n, d) in ALL_DB_COLUMNS {
        cols.push_str(&format!(", {n} {d}"));
    }
    cols.push_str(", sql TEXT, created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL, PRIMARY KEY (core_uid, db_id)");
    conn.execute(
        &format!("CREATE TABLE IF NOT EXISTS closed_sell_reports ({cols})"),
        [],
    )?;

    // ALTER-апгрейд: дописываем недостающие колонки в более старую таблицу.
    let mut existing = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(closed_sell_reports)")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        for name in rows {
            existing.insert(name?);
        }
    }
    for (name, decl) in ALL_DB_COLUMNS {
        if !existing.contains(*name) {
            conn.execute(
                &format!("ALTER TABLE closed_sell_reports ADD COLUMN {name} {decl}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn insert(conn: &Connection, row: &ReportRow) -> rusqlite::Result<()> {
    let ts = now_ms();
    let isshort = row.isshort.map(|b| b as i64);
    let emulator = row.emulator.map(|b| b as i64);
    // COALESCE: новое значение если есть, иначе НЕ затираем старое (важно, чтобы
    // повторный close-report не обнулял поля, взятые из модели ранее).
    conn.execute(
        "INSERT INTO closed_sell_reports
            (core_uid, core_name, db_id, taskid, exorderid, coin, isshort, buydate,
             sellsetdate, closedate, quantity, buyprice, sellprice, spentbtc, gainedbtc,
             profitbtc, lev, strategyid, emulator, status, sellreason, comment, sql,
             created_ms, updated_ms)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?24)
         ON CONFLICT(core_uid, db_id) DO UPDATE SET
            core_name=excluded.core_name, taskid=COALESCE(excluded.taskid,taskid),
            exorderid=COALESCE(excluded.exorderid,exorderid),
            coin=COALESCE(excluded.coin,coin), isshort=COALESCE(excluded.isshort,isshort),
            buydate=COALESCE(excluded.buydate,buydate),
            sellsetdate=COALESCE(excluded.sellsetdate,sellsetdate),
            closedate=COALESCE(excluded.closedate,closedate),
            quantity=COALESCE(excluded.quantity,quantity),
            buyprice=COALESCE(excluded.buyprice,buyprice),
            sellprice=COALESCE(excluded.sellprice,sellprice),
            spentbtc=COALESCE(excluded.spentbtc,spentbtc),
            gainedbtc=COALESCE(excluded.gainedbtc,gainedbtc),
            profitbtc=COALESCE(excluded.profitbtc,profitbtc),
            lev=COALESCE(excluded.lev,lev), strategyid=COALESCE(excluded.strategyid,strategyid),
            emulator=COALESCE(excluded.emulator,emulator),
            status=COALESCE(excluded.status,status),
            sellreason=COALESCE(excluded.sellreason,sellreason),
            comment=COALESCE(excluded.comment,comment),
            sql=excluded.sql, updated_ms=excluded.updated_ms",
        rusqlite::params![
            row.core_uid as i64, row.core_name, row.db_id, row.taskid, row.exorderid,
            row.coin, isshort, row.buydate, row.sellsetdate, row.closedate, row.quantity,
            row.buyprice, row.sellprice, row.spentbtc, row.gainedbtc, row.profitbtc,
            row.lev, row.strategyid, emulator, row.status, row.sellreason, row.comment,
            row.sql, ts,
        ],
    )?;
    Ok(())
}

/// Колонки, которыми занимается типизированный `insert` (или служебные PK/мета).
/// Всё ОСТАЛЬНОЕ из close-SQL идёт через универсальный passthrough `apply_extras`.
/// `server_id` обязателен в списке: авто-создание такой колонки заставило бы
/// `init_db` принять таблицу за древнюю схему и УДАЛИТЬ её.
fn is_passthrough(name: &str) -> bool {
    const SKIP: &[&str] = &[
        "core_uid", "core_name", "db_id", "sql", "created_ms", "updated_ms", "id", "server_id",
        "taskid", "exorderid", "coin", "isshort", "buydate", "sellsetdate", "closedate",
        "quantity", "buyprice", "sellprice", "spentbtc", "gainedbtc", "profitbtc", "lev",
        "strategyid", "emulator", "status", "sellreason", "comment",
    ];
    !SKIP.contains(&name)
}

/// Универсальный passthrough: для всех непокрытых типизированным insert полей из
/// SQL заводит недостающие колонки (`ALTER TABLE … ADD COLUMN`, тип по значению) и
/// пишет их в уже существующую (после `insert`) строку. `known` — кэш имён колонок
/// в памяти writer'а, чтобы не дёргать PRAGMA на каждую запись.
fn apply_extras(
    conn: &Connection,
    known: &mut std::collections::HashSet<String>,
    row: &ReportRow,
) -> rusqlite::Result<()> {
    let cols: Vec<&(String, Value)> = row
        .extras
        .iter()
        .filter(|(n, _)| is_passthrough(n))
        .collect();
    if cols.is_empty() {
        return Ok(());
    }
    for (name, val) in &cols {
        if !known.contains(name.as_str()) {
            let decl = match val {
                Value::Text(_) => "TEXT",
                Value::Integer(_) => "INTEGER",
                _ => "REAL",
            };
            // Имя провалидировано как [a-z0-9_] в parse::valid_ident — инъекции нет.
            conn.execute(
                &format!("ALTER TABLE closed_sell_reports ADD COLUMN {name} {decl}"),
                [],
            )?;
            known.insert((*name).clone());
            log::info!("отчёты: авто-колонка «{name}» {decl} (новое поле ядра)");
        }
    }
    let set = cols
        .iter()
        .map(|(n, _)| format!("{n}=?"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPDATE closed_sell_reports SET {set}, updated_ms=? WHERE core_uid=? AND db_id=?"
    );
    let ts = now_ms();
    let uid = row.core_uid as i64;
    let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(cols.len() + 3);
    for (_, val) in &cols {
        params.push(val);
    }
    params.push(&ts);
    params.push(&uid);
    params.push(&row.db_id);
    conn.execute(&sql, params.as_slice())?;
    Ok(())
}

/// Имена колонок таблицы отчётов сейчас (для seed-кэша writer'а и рантайм-display).
fn table_columns(conn: &Connection) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if let Ok(mut stmt) = conn.prepare("PRAGMA table_info(closed_sell_reports)") {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) {
            for n in rows.flatten() {
                out.insert(n);
            }
        }
    }
    out
}

pub fn spawn_writer() -> Option<ReportsHandle> {
    let (tx, rx): (Sender<ReportRow>, Receiver<ReportRow>) = std::sync::mpsc::channel();
    let path = paths::reports_db_path();
    let conn = match Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            log::error!("отчёты: не удалось открыть {}: {e}", path.display());
            return None;
        }
    };
    if let Err(e) = init_db(&conn) {
        log::error!("отчёты: init схемы не удался: {e}");
        return None;
    }
    let generation = Arc::new(AtomicU64::new(0));
    let gen_writer = generation.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("reports-db".into())
        .spawn(move || {
            log::info!("отчёты: writer запущен ({})", path.display());
            // Кэш известных колонок (растёт по мере авто-добавления новых полей ядра).
            let mut known = table_columns(&conn);
            while let Ok(row) = rx.recv() {
                match insert(&conn, &row).and_then(|()| apply_extras(&conn, &mut known, &row)) {
                    Ok(()) => {
                        gen_writer.fetch_add(1, Ordering::Relaxed);
                        log::info!(
                            "отчёт: {} ({}) db_id={} {} buy@{:?} {}",
                            row.core_uid,
                            row.core_name,
                            row.db_id,
                            row.coin.as_deref().unwrap_or("?"),
                            row.buydate,
                            row.profitbtc
                                .map(|p| format!("{p:+.4}BTC"))
                                .unwrap_or_default(),
                        );
                    }
                    Err(e) => log::error!("отчёты: запись db_id={} упала: {e}", row.db_id),
                }
            }
            log::info!("отчёты: writer завершён");
        })
    {
        log::error!("отчёты: не удалось запустить writer thread: {e}");
        return None;
    }
    Some(ReportsHandle { tx, generation })
}

// ============================================================================
//  Чтение для окна «Отчёты»
// ============================================================================

/// Результат выборки: имена колонок + строки значений (generic, под все колонки).
/// `cols` — РАНТАЙМ-список (из `PRAGMA table_info`), поэтому авто-добавленные поля
/// ядра показываются без правок: известные колонки в каноничном порядке, новые — в хвост.
pub struct ReportTable {
    pub cols: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// `core_uid` каждой строки (параллельно `rows`). Служебная колонка не входит в
    /// `cols`/DISPLAY_COLUMNS, но нужна, чтобы клик по монете в отчёте открыл чарт НА
    /// ТОМ ЯДРЕ, где была сделка (`core_uid` == рантайм-`CoreId`).
    pub core_uids: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideFilter {
    #[default]
    All,
    Long,
    Short,
}

#[derive(Debug, Clone, Default)]
pub struct ReportFilter {
    pub core_uid: Option<u64>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    pub coin: String,
    pub side: SideFilter,
}

pub fn open_reader() -> Option<Connection> {
    let path = paths::reports_db_path();
    if !path.exists() {
        return None;
    }
    match Connection::open(&path) {
        Ok(c) => {
            let _ = c.busy_timeout(Duration::from_secs(3));
            Some(c)
        }
        Err(e) => {
            log::warn!("отчёты(reader): {e}");
            None
        }
    }
}

pub fn load_sort(conn: &Connection) -> Option<(String, bool)> {
    let key: String = conn
        .query_row("SELECT value FROM app_meta WHERE key='sort_key'", [], |r| {
            r.get(0)
        })
        .ok()?;
    let desc: String = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='sort_desc'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "1".into());
    Some((key, desc != "0"))
}

pub fn save_sort(conn: &Connection, key: &str, desc: bool) {
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES('sort_key',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key],
    );
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES('sort_desc',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![if desc { "1" } else { "0" }],
    );
}

/// Сохранённый набор видимых колонок отчёта (имена через запятую). None — не сохраняли.
pub fn load_visible(conn: &Connection) -> Option<Vec<String>> {
    let csv: String = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='report_visible'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    Some(
        csv.split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Сохранить набор видимых колонок отчёта (имена через запятую).
pub fn save_visible(conn: &Connection, cols: &[&str]) {
    let csv = cols.join(",");
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES('report_visible',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![csv],
    );
}

/// Рантайм-список колонок для отображения: известные (`DISPLAY_COLUMNS`) в
/// каноничном порядке + авто-добавленные поля ядра в хвост (по алфавиту), минус
/// служебные. Используется и для SELECT, и окном «Отчёт» (заголовки/видимость/меню).
pub fn display_columns(conn: &Connection) -> Vec<String> {
    const SERVICE: &[&str] = &["core_uid", "sql", "created_ms", "updated_ms"];
    let have = table_columns(conn);
    let mut out: Vec<String> = DISPLAY_COLUMNS
        .iter()
        .filter(|c| have.contains(**c))
        .map(|c| (*c).to_string())
        .collect();
    let mut extra: Vec<String> = have
        .iter()
        .filter(|h| {
            !SERVICE.contains(&h.as_str()) && !DISPLAY_COLUMNS.contains(&h.as_str())
        })
        .cloned()
        .collect();
    extra.sort();
    out.extend(extra);
    out
}

/// Валидируем ключ сортировки против рантайм-колонок (без инъекций).
fn sort_column(cols: &[String], key: &str) -> String {
    cols.iter()
        .find(|c| c.as_str() == key)
        .cloned()
        .unwrap_or_else(|| "closedate".to_string())
}

fn build_where(f: &ReportFilter) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let mut sql = String::from(" WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(uid) = f.core_uid {
        sql.push_str(" AND core_uid = ?");
        params.push(Box::new(uid as i64));
    }
    if let Some(from) = f.date_from {
        sql.push_str(" AND closedate IS NOT NULL AND closedate >= ?");
        params.push(Box::new(from));
    }
    if let Some(to) = f.date_to {
        sql.push_str(" AND closedate IS NOT NULL AND closedate <= ?");
        params.push(Box::new(to));
    }
    let coin = f.coin.trim();
    if !coin.is_empty() {
        sql.push_str(" AND coin LIKE ?");
        params.push(Box::new(format!("%{}%", coin.to_uppercase())));
    }
    match f.side {
        SideFilter::All => {}
        SideFilter::Long => sql.push_str(" AND isshort = 0"),
        SideFilter::Short => sql.push_str(" AND isshort = 1"),
    }
    (sql, params)
}

/// Итог по ВСЕМУ фильтру (не по топ-N): (сумма profitbtc, число ордеров).
pub fn query_totals(conn: &Connection, f: &ReportFilter) -> (f64, i64) {
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "SELECT COALESCE(SUM(profitbtc),0.0), COUNT(*) FROM closed_sell_reports{where_sql}"
    );
    let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    conn.query_row(&sql, refs.as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap_or((0.0, 0))
}

/// Топ-`limit` отчётов по фильтру и сортировке. Возвращает все DISPLAY_COLUMNS.
pub fn query_reports(
    conn: &Connection,
    f: &ReportFilter,
    sort_key: &str,
    desc: bool,
    limit: usize,
) -> ReportTable {
    let (where_sql, mut params) = build_where(f);
    let cols = display_columns(conn);
    let col = sort_column(&cols, sort_key);
    let dir = if desc { "DESC" } else { "ASC" };
    let select = cols.join(", ");
    // `core_uid` тянем первой (служебной) колонкой — в `cols` не попадает, идёт в `core_uids`.
    let sql = format!(
        "SELECT core_uid, {select} FROM closed_sell_reports{where_sql}
         ORDER BY {col} IS NULL, {col} {dir}, closedate DESC LIMIT ?"
    );
    params.push(Box::new(limit as i64));
    let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();

    let mut rows = Vec::new();
    let mut core_uids = Vec::new();
    if let Ok(mut stmt) = conn.prepare(&sql) {
        let n = cols.len();
        if let Ok(mapped) = stmt.query_map(refs.as_slice(), |r| {
            let core_uid = r.get::<_, i64>(0)? as u64;
            let mut v = Vec::with_capacity(n);
            for i in 0..n {
                v.push(r.get::<_, Value>(i + 1)?);
            }
            Ok((core_uid, v))
        }) {
            for (uid, row) in mapped.flatten() {
                core_uids.push(uid);
                rows.push(row);
            }
        }
    }
    ReportTable {
        cols,
        rows,
        core_uids,
    }
}

pub fn distinct_cores(conn: &Connection) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT core_uid, core_name FROM closed_sell_reports
         GROUP BY core_uid ORDER BY MAX(updated_ms) DESC",
    ) {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?))
        }) {
            for row in rows.flatten() {
                out.push(row);
            }
        }
    }
    out
}

// ============================================================================
//  Дата/время без внешних крейтов (кроссплатформенно)
// ============================================================================

/// unix-секунды → "YYYY-MM-DD HH:MM" в UTC. Пусто для <=0.
pub fn fmt_unix(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// unix-секунды → "YYYY-MM-DD HH:MM:SS" в UTC (для лога команд).
pub fn fmt_unix_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// "YYYY-MM-DD" → unix-секунды (UTC, начало суток).
pub fn parse_ymd(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut it = s.split('-');
    let y: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let d: i64 = it.next()?.trim().parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
