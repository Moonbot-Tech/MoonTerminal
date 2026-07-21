//! Локальная SQLite-БД отчётов Orders.
//!
//! ОСНОВНОЙ путь — typed-реплика серверной БД (moonproto `Event::Report`, таблица
//! `orders_rep`, см. [`rep`]): схема от ядра, upsert/delete по `newRecID`, догонка
//! оффлайна с курсора, reconcile на `SyncComplete`. Аналитические поля (дельты/
//! pump/signaltype…) приходят настоящими значениями.
//!
//! ЛЕГАСИ путь (deprecated в moonproto) — close-SQL `Event::ClosedSellOrderReport` →
//! таблица `closed_sell_reports` (PK `(core_uid, db_id)`): жив только на переходный
//! период; после первого полного sync ядра его легаси-строки вычищаются, опустевшая
//! таблица сносится (маркер `legacy_dropped`), и поток игнорируется.
//!
//! Пишет ОДИН поток-writer; читает окно «Отчёты» отдельным соединением (WAL).

pub mod analytics;
pub mod integrity;
pub mod maint;
mod parse;
pub(crate) mod read_fail;
mod rep;
#[cfg(test)]
mod test_support;
pub mod tuner;
pub mod tuner_smart;

pub use parse::parse_report_sql;
pub use read_fail::{FailKind, ReadFail, ReadResult};
pub use rep::{DbMsg, ReportSink};

use read_fail::read_fail;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
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

/// Feed-to-writer sink for typed and legacy messages plus per-core start cursors.
pub type ReportTx = ReportSink;

/// Хэндл БД: канал записи + счётчик-генерация (растёт после записи —
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
/// в самом окне). core_uid/newrecid скрыты (служебные); `id` — серверный row id
/// (бывший легаси db_id).
pub const DISPLAY_COLUMNS: &[&str] = &[
    "buydate",
    "closedate",
    "sellsetdate",
    "core_name",
    "id",
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

    // Легаси-таблица уже снесена после полного переезда на typed-реплику —
    // не пересоздаём (CREATE IF NOT EXISTS вернул бы её из мёртвых).
    let legacy_dropped: bool = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='legacy_dropped'",
            [],
            |r| r.get::<_, String>(0),
        )
        .map(|v| v == "1")
        .unwrap_or(false);
    if legacy_dropped {
        return Ok(());
    }

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
    // Индексы под горячие запросы окна «Отчёт»: дефолтный фильтр периода (closedate) и
    // per-core вычистка/выборки. Легаси-БД бывает сотни МБ — без индекса каждый requery
    // (а их триггерит КАЖДАЯ запись writer'а) сканировал таблицу целиком (CPU-спайки).
    // Первое создание на большой базе занимает секунды — одноразово.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_csr_closedate ON closed_sell_reports(closedate)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_csr_core ON closed_sell_reports(core_uid)",
        [],
    )?;
    Ok(())
}

fn insert(conn: &Connection, row: &ReportRow) -> rusqlite::Result<()> {
    let ts = now_ms();
    let isshort = row.isshort.map(|b| b as i64);
    let emulator = row.emulator.map(|b| b as i64);
    // COALESCE: новое значение если есть, иначе НЕ затираем старое (важно, чтобы
    // повторный close-report не обнулял поля, взятые из модели ранее).
    // prepare_cached: writer может вставлять тысячи строк — компиляция SQL на каждую
    // строку была узким местом (очередь копилась).
    let mut stmt = conn.prepare_cached(
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
    )?;
    stmt.execute(rusqlite::params![
        row.core_uid as i64,
        row.core_name,
        row.db_id,
        row.taskid,
        row.exorderid,
        row.coin,
        isshort,
        row.buydate,
        row.sellsetdate,
        row.closedate,
        row.quantity,
        row.buyprice,
        row.sellprice,
        row.spentbtc,
        row.gainedbtc,
        row.profitbtc,
        row.lev,
        row.strategyid,
        emulator,
        row.status,
        row.sellreason,
        row.comment,
        row.sql,
        ts,
    ])?;
    Ok(())
}

/// Колонки, которыми занимается типизированный `insert` (или служебные PK/мета).
/// Всё ОСТАЛЬНОЕ из close-SQL идёт через универсальный passthrough `apply_extras`.
/// `server_id` обязателен в списке: авто-создание такой колонки заставило бы
/// `init_db` принять таблицу за древнюю схему и УДАЛИТЬ её.
fn is_passthrough(name: &str) -> bool {
    const SKIP: &[&str] = &[
        "core_uid",
        "core_name",
        "db_id",
        "sql",
        "created_ms",
        "updated_ms",
        "id",
        "server_id",
        "taskid",
        "exorderid",
        "coin",
        "isshort",
        "buydate",
        "sellsetdate",
        "closedate",
        "quantity",
        "buyprice",
        "sellprice",
        "spentbtc",
        "gainedbtc",
        "profitbtc",
        "lev",
        "strategyid",
        "emulator",
        "status",
        "sellreason",
        "comment",
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
    let sql =
        format!("UPDATE closed_sell_reports SET {set}, updated_ms=? WHERE core_uid=? AND db_id=?");
    let ts = now_ms();
    let uid = row.core_uid as i64;
    let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(cols.len() + 3);
    for (_, val) in &cols {
        params.push(val);
    }
    params.push(&ts);
    params.push(&uid);
    params.push(&row.db_id);
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.execute(params.as_slice())?;
    Ok(())
}

/// Probe legacy-table columns for the writer's close-SQL column cache.
///
/// Opening a database does not validate its schema b-tree. An absent table
/// therefore yields `Ok(empty)`, while any SQLite error remains `Err`.
fn table_columns_res(conn: &Connection) -> ReadResult<std::collections::HashSet<String>> {
    const CTX: &str = "отчёты: PRAGMA table_info(closed_sell_reports)";
    let mut out = std::collections::HashSet::new();
    let mut stmt = conn
        .prepare("PRAGMA table_info(closed_sell_reports)")
        .map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| read_fail(CTX, e))?;
    for n in rows {
        out.insert(n.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Count rows currently in the typed replica for the Storage settings tab.
///
/// Returns `NotReady` when the replica file is absent and `Failed` when opening
/// it or reading the count fails. Only a successful query may return zero.
pub fn report_row_count() -> ReadResult<i64> {
    const CTX: &str = "отчёты: число строк реплики";
    let conn = open_reader()?;
    conn.query_row(&format!("SELECT COUNT(*) FROM {}", rep::TABLE), [], |r| {
        r.get(0)
    })
    .map_err(|e| read_fail(CTX, e))
}

/// Ёмкость канала к writer'у: backpressure вместо OOM. ~16k сообщений ≈ десятки МБ
/// пика; при полном канале feed-поток ядра ждёт writer (естественный дроссель догонки).
const REPORT_QUEUE_CAP: usize = 16_384;

pub fn spawn_writer() -> Option<ReportsHandle> {
    let (tx, rx): (std::sync::mpsc::SyncSender<DbMsg>, Receiver<DbMsg>) =
        std::sync::mpsc::sync_channel(REPORT_QUEUE_CAP);
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
    let cursors = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let open_rows = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut rep_state = match rep::init(&conn, cursors.clone(), open_rows.clone()) {
        Ok(st) => st,
        Err(e) => {
            log::error!("отчёты: init typed-реплики не удался: {e}");
            return None;
        }
    };
    let generation = Arc::new(AtomicU64::new(0));
    let gen_writer = generation.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("reports-db".into())
        .spawn(move || {
            log::info!("отчёты: writer запущен ({})", path.display());
            // Путь к WAL-файлу — для ленивого чекпоинта по РЕАЛЬНОМУ размеру (см. ниже).
            let wal_path = {
                let mut s = path.clone().into_os_string();
                s.push("-wal");
                std::path::PathBuf::from(s)
            };
            // Кэш известных ЛЕГАСИ-колонок (авто-добавление новых полей close-SQL).
            // Lossy HERE and fatal in `rep::init` on purpose, and the asymmetry
            // is the point: this is a SEED for the legacy auto-column path, and
            // `apply_extras` reports its own failure per row. `rep::init`'s cache
            // instead GATES which fields a page may write while still ACKing it,
            // so a swallowed probe there destroys data silently.
            let mut known = table_columns_res(&conn).unwrap_or_default();
            // Батчим пачку сообщений в одну транзакцию: catch-up льёт тысячи строк,
            // и autocommit на каждую (fsync) растянул бы догонку на минуты.
            let mut thr_count: u64 = 0;
            let mut thr_started = std::time::Instant::now();
            // Старое значение → первый чекпоинт срабатывает на первом же батче, усекая уже
            // накопленный (возможно сотни МБ) WAL сразу после запуска, а не через 30с.
            let mut last_ckpt = std::time::Instant::now()
                .checked_sub(Duration::from_secs(30))
                .unwrap_or_else(std::time::Instant::now);
            loop {
                let first = match rx.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                };
                let mut batch = vec![first];
                while batch.len() < 512 {
                    match rx.try_recv() {
                        Ok(m) => batch.push(m),
                        Err(_) => break,
                    }
                }
                thr_count += batch.len() as u64;
                let txn = match conn.unchecked_transaction() {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!("отчёты: transaction не открылась: {e}");
                        continue;
                    }
                };
                // ack страниц (`page_applied`) — ПОСЛЕ коммита (контракт flow-control:
                // lib не запросит следующую страницу, пока эта не легла на диск).
                let mut acks = Vec::new();
                for msg in batch {
                    apply_msg(&txn, &mut rep_state, &mut known, msg, &mut acks);
                }
                match txn.commit() {
                    Ok(()) => {
                        for (ack, page) in acks {
                            if let Err(e) = ack.page_applied(&page) {
                                log::warn!("отчёты(rep): page_applied не ушёл: {e:?}");
                            }
                        }
                    }
                    Err(e) => log::error!("отчёты: commit батча упал: {e}"),
                }
                // Легаси снесена этим батчем → одноразовый VACUUM (вне транзакции):
                // возвращает диску сотни МБ старой таблицы. Блокирует только writer.
                if rep_state.vacuum_pending {
                    rep_state.vacuum_pending = false;
                    let t = std::time::Instant::now();
                    match conn.execute("VACUUM", []) {
                        Ok(_) => log::info!(
                            "отчёты: VACUUM после сноса легаси — {}с",
                            t.elapsed().as_secs()
                        ),
                        Err(e) => log::warn!("отчёты: VACUUM не удался: {e}"),
                    }
                }
                gen_writer.fetch_add(1, Ordering::Relaxed);
                // ЛЕНИВЫЙ чекпоинт WAL. Авто-чекпоинт SQLite (PASSIVE) обычно держит -wal мелким
                // сам; форсировать TRUNCATE каждый раз — лишний CPU-всплеск (копирование WAL→main
                // на writer-потоке). Поэтому TRUNCATE зовём ТОЛЬКО когда файл -wal реально разросся
                // (авто-сброс не поспевает — напр. reader «Отчётов» мешает reset'у), иначе просто
                // пропускаем. Так в норме всплесков нет, а раздувание диска в сотни МБ не случится.
                // Проверяем размер раз в 60с (stat дёшев).
                if last_ckpt.elapsed() >= Duration::from_secs(60) {
                    last_ckpt = std::time::Instant::now();
                    let wal_big = std::fs::metadata(&wal_path)
                        .map(|m| m.len() > 32 * 1024 * 1024)
                        .unwrap_or(false);
                    if wal_big {
                        if let Err(e) =
                            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                        {
                            log::debug!("отчёты: wal_checkpoint не удался: {e}");
                        }
                    }
                }
                // Диагностика пропускной способности при плотном потоке (догонка):
                // видно, успевает ли writer за входом (канал ограничен — не OOM).
                let el = thr_started.elapsed();
                if el >= Duration::from_secs(10) {
                    if thr_count > 2_000 {
                        log::info!(
                            "отчёты: writer {} сообщений за {:.0}с (~{}/с)",
                            thr_count,
                            el.as_secs_f32(),
                            (thr_count as f32 / el.as_secs_f32()) as u64,
                        );
                    }
                    thr_count = 0;
                    thr_started = std::time::Instant::now();
                }
            }
            log::info!("отчёты: writer завершён");
        })
    {
        log::error!("отчёты: не удалось запустить writer thread: {e}");
        return None;
    }
    Some(ReportsHandle {
        tx: ReportSink {
            tx,
            cursors,
            open_rows,
        },
        generation,
    })
}

/// Одно сообщение writer'а: typed-реплика или легаси close-SQL (в СВОЮ таблицу).
/// Страницы кладут ack в `acks` — вызывающий шлёт `page_applied` после коммита.
fn apply_msg(
    conn: &Connection,
    rep_state: &mut rep::RepState,
    known: &mut std::collections::HashSet<String>,
    msg: DbMsg,
    acks: &mut Vec<(
        moonproto::MoonReports,
        std::sync::Arc<moonproto::ReportSyncPage>,
    )>,
) {
    match msg {
        DbMsg::Legacy(row) => {
            // Легаси-поток игнорируем после сноса таблицы И для ядер с завершённым
            // sync: их данные уже идут typed-потоком, легаси-вставка после вычистки
            // давала дубль строки в читателе (реплика + легаси-копия).
            if !rep_state.legacy_exists || rep_state.synced.contains(&row.core_uid) {
                return;
            }
            match insert(conn, &row).and_then(|()| apply_extras(conn, known, &row)) {
                Ok(()) => log::info!(
                    "отчёт(легаси): {} ({}) db_id={} {} buy@{:?} {}",
                    row.core_uid,
                    row.core_name,
                    row.db_id,
                    row.coin.as_deref().unwrap_or("?"),
                    row.buydate,
                    row.profitbtc
                        .map(|p| format!("{p:+.4}BTC"))
                        .unwrap_or_default(),
                ),
                Err(e) => log::error!("отчёты: запись db_id={} упала: {e}", row.db_id),
            }
        }
        DbMsg::Schema { core_uid, schema } => rep::apply_schema(conn, rep_state, core_uid, schema),
        DbMsg::Upsert {
            core_uid,
            core_name,
            row,
        } => {
            if let Err(e) = rep::apply_upsert(conn, rep_state, core_uid, &core_name, &row) {
                log::error!(
                    "отчёты(rep): upsert rec_id={} ядра {core_uid} упал: {e}",
                    row.rec_id
                );
            }
        }
        DbMsg::Delete { core_uid, rec_id } => {
            if let Err(e) = rep::apply_delete(conn, core_uid, rec_id) {
                log::error!("отчёты(rep): delete rec_id={rec_id} ядра {core_uid} упал: {e}");
            }
        }
        DbMsg::Page {
            core_uid,
            core_name,
            page,
            ack,
        } => {
            if rep::apply_page(conn, rep_state, core_uid, &core_name, &page) {
                acks.push((ack, page));
            }
        }
        DbMsg::SyncComplete { core_uid, done } => {
            rep::apply_sync_complete(conn, rep_state, core_uid, &done);
        }
    }
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
    /// Выбранные ядра (мультивыбор); пусто = все.
    pub core_uids: Vec<u64>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    pub coin: String,
    pub side: SideFilter,
    /// Эмуляторные ордера: `None` — все, `Some(false)` — только реальные, `Some(true)` —
    /// только эмуляторные (поле-список типа в отчёте). NULL в колонке считается «реальный».
    pub emulator: Option<bool>,
}

/// Open a reader while distinguishing an absent replica from a SQLite failure.
///
/// A genuinely absent file maps to `NotReady`; metadata and SQLite open errors
/// map to `Failed` so callers cannot present them as an empty period.
pub fn open_reader() -> ReadResult<Connection> {
    let path = paths::reports_db_path();
    // NOT `Path::exists`: it reports false for a permission or metadata error
    // too, which would take the silent `NotReady` branch and tell the user the
    // replica is merely not synced yet. Only a genuine absence may say that.
    // The check still has to happen because `Connection::open` would otherwise
    // CREATE an empty database in place of the missing one.
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadFail::NotReady),
        Err(e) => return Err(read_fail::io_fail("отчёты(reader): доступ к файлу", &e)),
    }
    let conn = Connection::open(&path).map_err(|e| read_fail("отчёты(reader)", e))?;
    let _ = conn.busy_timeout(Duration::from_secs(3));
    Ok(conn)
}

/// Pin one WAL snapshot for a multi-statement read.
///
/// Separate autocommit statements each observe a newer snapshot, so a panel
/// could publish a row list and totals that disagree about which trades fall
/// inside the period. Read-only: dropping the transaction rolls back nothing.
/// ATTACH cannot run inside a transaction, so callers attach first. Transaction
/// creation errors map to `Failed`; this function cannot produce `NotReady`.
pub fn read_snapshot(conn: &Connection) -> ReadResult<rusqlite::Transaction<'_>> {
    conn.unchecked_transaction()
        .map_err(|e| read_fail("отчёты: снимок для чтения", e))
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

/// Build the runtime display-column list from the typed and legacy schemas.
///
/// Known columns follow `DISPLAY_COLUMNS`; extra columns follow alphabetically,
/// service columns are omitted, and legacy `db_id` is exposed as `id`. Schema
/// probe errors map to `Failed`; an absent table contributes no columns. This
/// function receives an open connection and therefore cannot return `NotReady`.
pub fn display_columns(conn: &Connection) -> ReadResult<Vec<String>> {
    const SERVICE: &[&str] = &[
        "core_uid",
        "newrecid",
        "db_id",
        "sql",
        "created_ms",
        "updated_ms",
    ];
    let mut have = rep::table_cols_res(conn)?;
    let legacy = table_columns_res(conn)?;
    if legacy.contains("db_id") {
        have.insert("id".to_string());
    }
    have.extend(legacy);
    let mut out: Vec<String> = DISPLAY_COLUMNS
        .iter()
        .filter(|c| have.contains(**c))
        .map(|c| (*c).to_string())
        .collect();
    let mut extra: Vec<String> = have
        .iter()
        .filter(|h| !SERVICE.contains(&h.as_str()) && !DISPLAY_COLUMNS.contains(&h.as_str()))
        .cloned()
        .collect();
    extra.sort();
    out.extend(extra);
    Ok(out)
}

/// Источник чтения отчётов: таблица + её колонки + легаси-флаг. Читаем typed-реплику и
/// (пока не снесена) легаси-таблицу РАЗДЕЛЬНЫМИ запросами — каждый со своим WHERE/
/// ORDER/LIMIT, работают индексы — и сливаем в Rust. UNION ALL-подзапрос с NULL-
/// проекцией SQLite не флаттенит: фильтр не проталкивался в ветки → полный скан
/// легаси-БД в сотни МБ на каждый requery (замерено: ~400мс при «Сегодня»).
/// Дубли при слиянии невозможны: строки ядра живут ровно в одной таблице (легаси
/// синхронизированных ядер вычищен и их поток игнорируется).
struct ReadSource {
    table: &'static str,
    cols: std::collections::HashSet<String>,
    legacy: bool,
}

/// Discover report sources while preserving failures from either schema probe.
///
/// An absent legacy table is omitted. Schema probe errors map to `Failed`; this
/// function receives an open connection and therefore cannot return `NotReady`.
fn read_sources_res(conn: &Connection) -> ReadResult<Vec<ReadSource>> {
    let mut out = vec![ReadSource {
        table: rep::TABLE,
        cols: rep::table_cols_res(conn)?,
        legacy: false,
    }];
    let legacy_cols = table_columns_res(conn)?;
    if !legacy_cols.is_empty() {
        out.push(ReadSource {
            table: "closed_sell_reports",
            cols: legacy_cols,
            legacy: true,
        });
    }
    Ok(out)
}

/// SELECT-проекция источника на общий набор `cols`: своя колонка как есть, легаси
/// `db_id` → `id`, отсутствующая → NULL.
fn source_select(src: &ReadSource, cols: &[String]) -> String {
    cols.iter()
        .map(|c| {
            if src.legacy && c == "id" && src.cols.contains("db_id") {
                "db_id AS \"id\"".to_string()
            } else if src.cols.contains(c) {
                format!("\"{c}\"")
            } else {
                format!("NULL AS \"{c}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Сравнение значений при слиянии сортированных источников (числа как f64, текст
/// лексикографически; NULL обрабатывает вызывающий — всегда в хвост).
fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            _ => None,
        }
    }
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => match (a, b) {
            (Value::Text(x), Value::Text(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        },
    }
}

/// Валидируем ключ сортировки против рантайм-колонок (без инъекций). Фолбэк —
/// closedate, а до прихода схемы ядра (нет такой колонки) — newrecid (есть всегда).
fn sort_column(cols: &[String], key: &str) -> String {
    if let Some(c) = cols.iter().find(|c| c.as_str() == key) {
        return c.clone();
    }
    if cols.iter().any(|c| c == "closedate") {
        "closedate".to_string()
    } else {
        "newrecid".to_string()
    }
}

/// Условия по колонкам ставим только на СУЩЕСТВУЮЩИЕ в источнике: пока схема ядра не
/// пришла, у реплики может не быть closedate/coin/isshort/emulator — фильтр по
/// отсутствующей колонке валил бы весь SELECT (нет колонки = нет и данных для условия).
fn build_where(
    f: &ReportFilter,
    cols: &std::collections::HashSet<String>,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let has = |n: &str| cols.contains(n);
    let mut sql = String::from(" WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if !f.core_uids.is_empty() {
        // Числа инлайним (инъекции нет), IN — под мультивыбор ядер.
        let ids = f
            .core_uids
            .iter()
            .map(|u| (*u as i64).to_string())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND core_uid IN ({ids})"));
    }
    if has("closedate") {
        if let Some(from) = f.date_from {
            sql.push_str(" AND closedate IS NOT NULL AND closedate >= ?");
            params.push(Box::new(from));
        }
        if let Some(to) = f.date_to {
            sql.push_str(" AND closedate IS NOT NULL AND closedate <= ?");
            params.push(Box::new(to));
        }
    }
    let coin = f.coin.trim();
    if !coin.is_empty() && has("coin") {
        sql.push_str(" AND coin LIKE ?");
        params.push(Box::new(format!("%{}%", coin.to_uppercase())));
    }
    if has("isshort") {
        match f.side {
            SideFilter::All => {}
            SideFilter::Long => sql.push_str(" AND isshort = 0"),
            SideFilter::Short => sql.push_str(" AND isshort = 1"),
        }
    }
    if has("emulator") {
        match f.emulator {
            None => {}
            Some(true) => sql.push_str(" AND COALESCE(emulator, 0) = 1"),
            Some(false) => sql.push_str(" AND COALESCE(emulator, 0) = 0"),
        }
    }
    (sql, params)
}

/// Aggregate profit and order count over the complete filter, not only the top N.
///
/// Returns `Failed` when source discovery or any aggregate query fails; only a
/// successful empty result returns `(0.0, 0)`. The open connection means this
/// function cannot return `NotReady`.
pub fn query_totals(conn: &Connection, f: &ReportFilter) -> ReadResult<(f64, i64)> {
    const CTX: &str = "отчёты: query_totals";
    let (mut sum, mut count) = (0.0f64, 0i64);
    for src in read_sources_res(conn)? {
        let (where_sql, params) = build_where(f, &src.cols);
        let profit = if src.cols.contains("profitbtc") {
            "COALESCE(SUM(profitbtc),0.0)"
        } else {
            "0.0"
        };
        let sql = format!("SELECT {profit}, COUNT(*) FROM {}{where_sql}", src.table);
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let (s, c) = conn
            .query_row(&sql, refs.as_slice(), |r| {
                Ok((r.get::<_, f64>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| read_fail(CTX, e))?;
        sum += s;
        count += c;
    }
    Ok((sum, count))
}

/// Return the top `limit` reports for the filter and sort using all display columns.
///
/// Source, schema, query, and row-conversion errors map to `Failed`; no partial
/// table is returned, which also keeps exports from writing incomplete data.
/// The open connection means this function cannot return `NotReady`.
pub fn query_reports(
    conn: &Connection,
    f: &ReportFilter,
    sort_key: &str,
    desc: bool,
    limit: usize,
) -> ReadResult<ReportTable> {
    const CTX: &str = "отчёты: query_reports";
    let cols = display_columns(conn)?;
    let col = sort_column(&cols, sort_key);
    let sort_ix = cols.iter().position(|c| *c == col);
    let dir = if desc { "DESC" } else { "ASC" };

    // Топ-N с КАЖДОГО источника своим запросом (индексы работают), слияние ниже.
    let mut merged: Vec<(u64, Vec<Value>)> = Vec::new();
    for src in read_sources_res(conn)? {
        let (where_sql, mut params) = build_where(f, &src.cols);
        let select = source_select(&src, &cols);
        // Сортируем на стороне SQL только если колонка есть у источника (alias `id`
        // у легаси виден в ORDER BY); иначе порядок неважен — доупорядочит слияние.
        let sortable = src.cols.contains(&col) || (src.legacy && col == "id");
        let order = if sortable {
            format!("\"{col}\" IS NULL, \"{col}\" {dir}")
        } else {
            "1".to_string()
        };
        let sql = format!(
            "SELECT core_uid, {select} FROM {}{where_sql} ORDER BY {order} LIMIT ?",
            src.table
        );
        params.push(Box::new(limit as i64));
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let n = cols.len();
        let mapped = stmt
            .query_map(refs.as_slice(), |r| {
                let core_uid = r.get::<_, i64>(0)? as u64;
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    v.push(r.get::<_, Value>(i + 1)?);
                }
                Ok((core_uid, v))
            })
            .map_err(|e| read_fail(CTX, e))?;
        // Every row is a trade the user is entitled to see, and the same rows
        // are what the export writes — so no row error is skippable here.
        for row in mapped {
            merged.push(row.map_err(|e| read_fail(CTX, e))?);
        }
    }

    // Слияние: NULL всегда в хвост (как «{col} IS NULL» в SQL), затем направление.
    merged.sort_by(|a, b| {
        let va = sort_ix.and_then(|i| a.1.get(i)).unwrap_or(&Value::Null);
        let vb = sort_ix.and_then(|i| b.1.get(i)).unwrap_or(&Value::Null);
        match (matches!(va, Value::Null), matches!(vb, Value::Null)) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => {
                let o = cmp_values(va, vb);
                if desc {
                    o.reverse()
                } else {
                    o
                }
            }
        }
    });
    merged.truncate(limit);

    let mut rows = Vec::with_capacity(merged.len());
    let mut core_uids = Vec::with_capacity(merged.len());
    for (uid, row) in merged {
        core_uids.push(uid);
        rows.push(row);
    }
    Ok(ReportTable {
        cols,
        rows,
        core_uids,
    })
}

/// Open the replica strictly read-only, for a probe that must not own the file.
///
/// [`open_reader`] opens read-WRITE despite its name, which is harmless for its own callers
/// because the writer connection is alive by then. A probe that runs BEFORE `spawn_writer` would
/// instead be the only connection, and closing a read-write connection makes SQLite checkpoint
/// the WAL and delete `-wal`/`-shm` on the spot — a WAL left by a killed run reaches hundreds of
/// MB, so that would land as a synchronous copy on the pre-window startup path. A read-only
/// connection never checkpoints on close.
pub fn open_readonly() -> ReadResult<Connection> {
    let path = paths::reports_db_path();
    // Same reasoning as `open_reader`: only a genuine absence may report `NotReady`.
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadFail::NotReady),
        Err(e) => return Err(read_fail::io_fail("отчёты(ro): доступ к файлу", &e)),
    }
    Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| read_fail("отчёты(ro)", e))
}

/// Highest `core_uid` in one table, or `Ok(None)` when it is absent or holds no rows.
///
/// Shared by both stores keyed on `core_uid`, so the negative-value drop and the
/// absent-versus-failed distinction have one definition rather than one per caller.
pub(crate) fn max_core_uid_in(
    conn: &Connection,
    table: &str,
    ctx: &'static str,
) -> ReadResult<Option<u64>> {
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )
        .map_err(|e| read_fail(ctx, e))?;
    if present == 0 {
        return Ok(None);
    }
    let found: Option<i64> = conn
        .query_row(&format!("SELECT MAX(core_uid) FROM {table}"), [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .map_err(|e| read_fail(ctx, e))?;
    // A negative value cannot be a uid; dropping it beats wrapping into a huge `u64`.
    Ok(found.and_then(|v| u64::try_from(v).ok()))
}

/// Highest `core_uid` any report row has ever carried, across both schemas.
///
/// Feeds the durable uid high-water mark: rows here outlive the server that wrote them, so a
/// uid still present in this replica must never be handed to a new core. `Ok(None)` means the
/// read succeeded and found no rows — the caller must keep that distinct from a failure, since
/// only the former is safe to treat as "this store contributes nothing".
///
/// A source whose schema lacks `core_uid` is skipped rather than queried: `read_sources_res`
/// always reports the modern table, which does not exist until `rep::init` has run. Negative
/// values cannot be uids and are dropped instead of wrapping into a huge `u64`.
pub fn max_core_uid(conn: &Connection) -> ReadResult<Option<u64>> {
    const CTX: &str = "отчёты: max_core_uid";
    let mut max: Option<u64> = None;
    for src in read_sources_res(conn)? {
        if !src.cols.contains("core_uid") {
            continue;
        }
        // MAX over the leading PK column is an index seek. The selector-shaped GROUP BY in
        // `distinct_cores` would instead scan every row of a replica that reaches hundreds of MB.
        max = max.max(max_core_uid_in(conn, src.table, CTX)?);
    }
    Ok(max)
}

/// Load cores for the filter selector.
///
/// Source, query, and row-conversion errors map to `Failed`; only a successful
/// query may return an empty list. The open connection means this function
/// cannot return `NotReady`.
pub fn distinct_cores(conn: &Connection) -> ReadResult<Vec<(u64, String)>> {
    const CTX: &str = "отчёты: distinct_cores";
    let mut out: Vec<(u64, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for src in read_sources_res(conn)? {
        let order = if src.legacy {
            "MAX(COALESCE(updated_ms, 0))"
        } else {
            "MAX(newrecid)"
        };
        let mut stmt = conn
            .prepare(&format!(
                "SELECT core_uid, core_name FROM {} GROUP BY core_uid ORDER BY {order} DESC",
                src.table
            ))
            .map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            // core_uid keys the whole selector, so a conversion miss here is
            // never a skippable "dirty label".
            let (uid, name) = row.map_err(|e| read_fail(CTX, e))?;
            if seen.insert(uid) {
                out.push((uid, name));
            }
        }
    }
    Ok(out)
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
    let (y, m, d) = crate::util::time::civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// unix-секунды → "YYYY-MM-DD HH:MM:SS" в UTC (для лога команд).
pub fn fmt_unix_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = crate::util::time::civil_from_days(days);
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

#[cfg(test)]
mod tests;
