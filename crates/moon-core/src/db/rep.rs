//! Typed replica of the Orders report database (`moonproto::Event::Report`) in
//! the `orders_rep` table.
//!
//! The core supplies an append-only schema through `ReportEvent::Schema`.
//! Column names are stored lowercase to match the legacy names consumed by the
//! Report panel; SQLite itself is case-insensitive. The replication key is
//! `(core_uid, newrecid)`. The legacy `closed_sell_reports` table uses a different
//! `db_id`, is read-only, and is purged per core after the first complete typed sync.
//!
//! Under the protocol-v4 replication contract, a core resumes at the local
//! `max(newRecID) + 1`; zero requests a fresh sync. Open rows below that cursor
//! are reconciled separately through `check_open_rows`.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use moonproto::{
    MoonReports, ReportRow as RepRow, ReportSchema, ReportSyncComplete, ReportSyncPage, ReportValue,
};
use rusqlite::types::Value;
use rusqlite::Connection;

pub(super) const TABLE: &str = "orders_rep";

/// Сообщение SQLite-writer'у (единственному владельцу соединения на запись).
pub enum DbMsg {
    /// Схема реплики ядра (append-only) — создать/дорастить колонки.
    Schema {
        core_uid: u64,
        schema: Arc<ReportSchema>,
    },
    /// Typed upsert живой строки по `(core_uid, newrecid)` (идемпотентен).
    Upsert {
        core_uid: u64,
        core_name: String,
        row: RepRow,
    },
    Delete {
        core_uid: u64,
        rec_id: i64,
    },
    /// Страница catch-up (flow-control контракт reports.md): writer применяет строки
    /// и шлёт `page_applied` ПОСЛЕ коммита транзакции — до этого lib следующую
    /// страницу не запрашивает (одна страница в полёте, backpressure by design).
    Page {
        core_uid: u64,
        core_name: String,
        page: Arc<ReportSyncPage>,
        ack: MoonReports,
    },
    /// Завершение catch-up (после ack последней страницы): коммит курсора, легаси-вычистка.
    SyncComplete {
        core_uid: u64,
        done: ReportSyncComplete,
    },
}

/// То, что feed-поток получает как `ReportTx`: канал к writer'у + стартовые курсоры
/// (посчитаны writer'ом при открытии БД — feed берёт свой при запуске sync).
///
/// Канал ОГРАНИЧЕН ([`super::REPORT_QUEUE_CAP`]): catch-up огромной истории льёт батчи
/// быстрее, чем writer вставляет — безлимитная очередь съедала ВСЮ память машины
/// (замерено: 88ГБ virtual commit → фриз системы). Заполнился — feed-поток ядра
/// блокируется на send (backpressure), это касается практически только догонки.
#[derive(Clone)]
pub struct ReportSink {
    pub(super) tx: SyncSender<DbMsg>,
    pub(super) cursors: Arc<Mutex<HashMap<u64, i64>>>,
    /// Открытые строки per core (newrecid, свежие первые, ≤100) на момент открытия БД —
    /// feed регистрирует их `check_open_rows` (открытая сделка могла закрыться/удалиться
    /// в оффлайне НИЖЕ курсора; результаты придут обычными RowUpsert/RowDelete).
    pub(super) open_rows: Arc<Mutex<HashMap<u64, Vec<i64>>>>,
}

impl ReportSink {
    pub fn send(&self, msg: DbMsg) {
        let _ = self.tx.send(msg);
    }

    /// Курсор следующего sync-запроса ядра: 0 → fresh, >0 → resume(from).
    /// По новому контракту reports.md курсор всегда `max(newRecID)+1` локальной реплики
    /// (страницы ack'аются последовательно — дыр в хвосте не бывает и после краша).
    pub fn next_cursor(&self, core_uid: u64) -> i64 {
        self.cursors
            .lock()
            .ok()
            .and_then(|m| m.get(&core_uid).copied())
            .unwrap_or(0)
    }

    /// Открытые строки ядра для `check_open_rows` (пусто — нечего проверять).
    pub fn open_rows(&self, core_uid: u64) -> Vec<i64> {
        self.open_rows
            .lock()
            .ok()
            .and_then(|m| m.get(&core_uid).cloned())
            .unwrap_or_default()
    }
}

/// Состояние typed-реплики внутри writer-потока.
pub(super) struct RepState {
    /// Схема per core (field_index → имя): нужна для маппинга строк.
    schemas: HashMap<u64, Arc<ReportSchema>>,
    /// Кэш lowercase-имён колонок таблицы (не дёргать PRAGMA на каждую строку).
    cols: HashSet<String>,
    cursors: Arc<Mutex<HashMap<u64, i64>>>,
    /// Whether the read-only legacy table still exists; completed typed syncs
    /// progressively purge it by core.
    pub(super) legacy_exists: bool,
    /// Cores with a persisted completed-sync marker (`rep_synced_*=1`).
    /// Initialization uses this set to purge legacy rows left by terminal versions
    /// that still wrote close-SQL reports; protocol v4 has no such write path.
    pub(super) synced: HashSet<u64>,
    /// The legacy table was dropped, so the writer must VACUUM after committing
    /// the batch; VACUUM is forbidden inside the transaction, and without it the
    /// database retains the dropped table's allocated space.
    pub(super) vacuum_pending: bool,
    /// Все индексы реплики гарантированно созданы (не дёргать CREATE на
    /// каждую схему ядра).
    indexes_done: bool,
}

/// Индексы под горячие запросы реплики. Колонки приходят из схемы ядра в
/// непредсказуемый момент, поэтому проверяем и на старте (в старой БД колонки
/// уже есть), и после каждой схемы: создание только в ветке успешного ALTER
/// теряло индекс НАВСЕГДА, если колонка старше кода индекса (или CREATE разово
/// упал). Возвращает true, когда все индексы точно есть — больше не звать.
fn ensure_indexes(conn: &Connection, cols: &HashSet<String>) -> bool {
    let mut done = true;
    // Дефолтный фильтр периода окна «Отчёт» (как idx_csr_closedate у легаси).
    if cols.contains("closedate") {
        if let Err(e) = conn.execute(
            &format!("CREATE INDEX IF NOT EXISTS idx_rep_closedate ON {TABLE}(closedate)"),
            [],
        ) {
            log::warn!("отчёты(rep): индекс idx_rep_closedate не создался: {e}");
            done = false;
        }
    } else {
        done = false;
    }
    // Аналитика версий стратегий (strat_db): выборка сделок одной стратегии +
    // range-join по buydate к valid_from/valid_to версии.
    if cols.contains("strategyid") && cols.contains("buydate") {
        if let Err(e) = conn.execute(
            &format!(
                "CREATE INDEX IF NOT EXISTS idx_rep_strat ON {TABLE}(core_uid, strategyid, buydate)"
            ),
            [],
        ) {
            log::warn!("отчёты(rep): индекс idx_rep_strat не создался: {e}");
            done = false;
        }
    } else {
        done = false;
    }
    done
}

/// Initialize the replica table, per-core cursors, and open-row sets.
///
/// Table-creation, required-schema, and core-scan setup errors return `Err`.
/// In particular, an unreadable column schema makes the caller disable the
/// writer for this session rather than acknowledge incompletely written pages.
pub(super) fn init(
    conn: &Connection,
    cursors: Arc<Mutex<HashMap<u64, i64>>>,
    open_rows: Arc<Mutex<HashMap<u64, Vec<i64>>>>,
) -> rusqlite::Result<RepState> {
    conn.execute(
        &format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (core_uid INTEGER NOT NULL, \
             core_name TEXT NOT NULL, newrecid INTEGER NOT NULL, \
             PRIMARY KEY (core_uid, newrecid))"
        ),
        [],
    )?;
    // Fatal on purpose: this cache decides which fields every incoming page may
    // write. Continuing with an empty cache would omit fields while still ACKing
    // the page, and ACKed pages are not sent again.
    //
    // `spawn_writer` therefore disables report recording for this session and
    // logs the failure. The server-side cursor remains unchanged, so restarting
    // with a readable replica can resync the page instead of losing its fields.
    let cols = table_cols_for_init(conn)?;
    {
        let mut map = cursors.lock().unwrap_or_else(|e| e.into_inner());
        let mut open_map = open_rows.lock().unwrap_or_else(|e| e.into_inner());
        map.clear();
        open_map.clear();
        let mut stmt = conn.prepare(&format!("SELECT DISTINCT core_uid FROM {TABLE}"))?;
        let uids: Vec<i64> = stmt.query_map([], |r| r.get(0))?.flatten().collect();
        for uid in uids {
            map.insert(uid as u64, startup_cursor(conn, uid));
            open_map.insert(uid as u64, open_row_ids(conn, &cols, uid));
        }
    }
    let legacy_exists = table_exists(conn, "closed_sell_reports");
    // Уже синхронизированные ядра — из меты (переживает рестарт).
    let mut synced: HashSet<u64> = HashSet::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT key FROM app_meta WHERE key LIKE 'rep_synced_%' AND value='1'")
    {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
            synced.extend(rows.flatten().filter_map(|k| {
                k.strip_prefix("rep_synced_")
                    .and_then(|s| s.parse::<u64>().ok())
            }));
        }
    }
    let indexes_done = ensure_indexes(conn, &cols);
    let mut st = RepState {
        schemas: HashMap::new(),
        cols,
        cursors,
        legacy_exists,
        synced,
        vacuum_pending: false,
        indexes_done,
    };
    // Purge rows left after SyncComplete by older terminal versions that still
    // consumed the legacy close-SQL stream; otherwise the merged reader sees duplicates.
    for uid in st.synced.clone() {
        purge_legacy(conn, &mut st, uid);
    }
    Ok(st)
}

/// Удалить легаси-строки ядра; опустевшую легаси-таблицу снести насовсем (маркер
/// `legacy_dropped` — init_db больше не создаст). Общий путь SyncComplete и init-вычистки.
fn purge_legacy(conn: &Connection, st: &mut RepState, core_uid: u64) {
    if !st.legacy_exists {
        return;
    }
    let _ = conn.execute(
        "DELETE FROM closed_sell_reports WHERE core_uid=?1",
        [core_uid as i64],
    );
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM closed_sell_reports", [], |r| r.get(0))
        .unwrap_or(1);
    if left == 0 && conn.execute("DROP TABLE closed_sell_reports", []).is_ok() {
        st.legacy_exists = false;
        st.vacuum_pending = true;
        meta_set_i64(conn, "legacy_dropped", 1);
        log::info!(
            "отчёты(rep): легаси-таблица closed_sell_reports снесена — все ядра на typed-реплике"
        );
    }
}

/// Whether a lowercase key is a safe SQLite identifier (guards against injection
/// where the name is interpolated into `ALTER TABLE` without quoting):
/// `[a-z_][a-z0-9_]*`.
fn valid_ident(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && (b[0] == b'_' || b[0].is_ascii_lowercase())
        && b.iter()
            .all(|&c| c == b'_' || c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Append-only схема ядра → доращиваем недостающие колонки. Имя lowercase; `sql_spec`
/// приходит от аутентифицированного ядра (тот же trust, что `sqlite_add_column_sql`
/// самого moonproto), имя дополнительно валидируем как идентификатор.
pub(super) fn apply_schema(
    conn: &Connection,
    st: &mut RepState,
    core_uid: u64,
    schema: Arc<ReportSchema>,
) {
    for f in schema.fields() {
        let name = f.name.to_ascii_lowercase();
        if name == "newrecid" || st.cols.contains(&name) {
            continue;
        }
        if !valid_ident(&name) {
            log::warn!(
                "отчёты(rep): поле схемы «{}» — не идентификатор, пропущено",
                f.name
            );
            continue;
        }
        match conn.execute(
            &format!("ALTER TABLE {TABLE} ADD COLUMN \"{name}\" {}", f.sql_spec),
            [],
        ) {
            Ok(_) => {
                log::info!(
                    "отчёты(rep): колонка «{name}» {} (схема ядра {core_uid})",
                    f.sql_spec
                );
                st.cols.insert(name);
            }
            Err(e) => log::error!("отчёты(rep): ADD COLUMN {name} не удался: {e}"),
        }
    }
    // Индексные колонки — авто (из схемы ядра), поэтому доводим индексы здесь,
    // когда колонки точно есть; IF NOT EXISTS делает повторные вызовы бесплатными.
    if !st.indexes_done {
        st.indexes_done = ensure_indexes(conn, &st.cols);
    }
    st.schemas.insert(core_uid, schema);
}

/// Идемпотентный upsert строки по `(core_uid, newrecid)`. Пишем только поля,
/// присутствующие в строке (live-обновления могут быть частичными) — остальные
/// значения не затираются.
pub(super) fn apply_upsert(
    conn: &Connection,
    st: &RepState,
    core_uid: u64,
    core_name: &str,
    row: &RepRow,
) -> rusqlite::Result<()> {
    let Some(schema) = st.schemas.get(&core_uid) else {
        log::warn!(
            "отчёты(rep): строка rec_id={} ядра {core_uid} до схемы — пропущена",
            row.rec_id
        );
        return Ok(());
    };
    let mut names: Vec<String> = Vec::with_capacity(row.fields.len());
    let mut vals: Vec<Value> = Vec::with_capacity(row.fields.len());
    for fv in &row.fields {
        let Some(f) = schema.field(fv.field_index) else {
            continue;
        };
        let name = f.name.to_ascii_lowercase();
        if name == "newrecid" || !st.cols.contains(&name) {
            continue;
        }
        vals.push(match &fv.value {
            ReportValue::Integer(i) => Value::Integer(*i),
            ReportValue::Float(x) => Value::Real(*x),
            ReportValue::Text(s) => Value::Text(s.clone()),
        });
        names.push(name);
    }
    let mut cols_sql = String::from("core_uid, core_name, newrecid");
    let mut ph = String::from("?, ?, ?");
    let mut set_sql = String::from("core_name=excluded.core_name");
    for n in &names {
        cols_sql.push_str(&format!(", \"{n}\""));
        ph.push_str(", ?");
        set_sql.push_str(&format!(", \"{n}\"=excluded.\"{n}\""));
    }
    let sql = format!(
        "INSERT INTO {TABLE} ({cols_sql}) VALUES ({ph}) \
         ON CONFLICT(core_uid, newrecid) DO UPDATE SET {set_sql}"
    );
    let uid = core_uid as i64;
    let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(vals.len() + 3);
    params.push(&uid);
    params.push(&core_name);
    params.push(&row.rec_id);
    for v in &vals {
        params.push(v);
    }
    // prepare_cached: у батчей catch-up набор полей стабилен → SQL одинаков, компиляция
    // на каждую строку была узким местом writer'а (очередь копилась → OOM).
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.execute(params.as_slice())?;
    Ok(())
}

pub(super) fn apply_delete(conn: &Connection, core_uid: u64, rec_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        &format!("DELETE FROM {TABLE} WHERE core_uid=?1 AND newrecid=?2"),
        rusqlite::params![core_uid as i64, rec_id],
    )?;
    Ok(())
}

/// Страница catch-up: `database_recreated` → сброс реплики ядра (lib после ack сама
/// рестартует операцию с нуля), затем идемпотентные upsert'ы строк. Возвращает `true`,
/// если страницу можно ack'ать (ошибки отдельных строк логируются, но не стопорят
/// поток — иначе догонка застряла бы навсегда).
pub(super) fn apply_page(
    conn: &Connection,
    st: &mut RepState,
    core_uid: u64,
    core_name: &str,
    page: &ReportSyncPage,
) -> bool {
    if page.database_recreated {
        let _ = conn.execute(
            &format!("DELETE FROM {TABLE} WHERE core_uid=?1"),
            [core_uid as i64],
        );
        log::warn!(
            "отчёты(rep): ядро {core_uid} пересоздало БД — реплика сброшена, lib рестартует sync"
        );
    }
    for row in page.rows.iter() {
        if let Err(e) = apply_upsert(conn, st, core_uid, core_name, row) {
            log::error!(
                "отчёты(rep): страница ядра {core_uid}: upsert rec_id={} упал: {e}",
                row.rec_id
            );
        }
    }
    true
}

/// `SyncComplete` (после ack последней страницы): коммит курсора, вычистка легаси-строк
/// ядра (+DROP легаси-таблицы, когда она опустела — закладка на полный переезд).
pub(super) fn apply_sync_complete(
    conn: &Connection,
    st: &mut RepState,
    core_uid: u64,
    done: &ReportSyncComplete,
) {
    meta_set_i64(conn, &format!("rep_synced_{core_uid}"), 1);
    if let Ok(mut m) = st.cursors.lock() {
        m.insert(core_uid, done.next_from_rec_id.max(1));
    }
    log::info!(
        "отчёты(rep): sync ядра {core_uid} завершён: страниц={} строк={} max_rec_id={} курсор={}",
        done.page_count,
        done.total_rows,
        done.max_rec_id,
        done.next_from_rec_id,
    );

    // A complete typed replica supersedes this core's legacy rows. No legacy
    // write path remains, so the read-only rows cannot reappear after this purge.
    purge_legacy(conn, st, core_uid);
}

/// Курсор ядра по локальной БД: новый контракт reports.md — ВСЕГДА `max(newRecID)+1`
/// (страницы ack'аются последовательно, дыр в хвосте не бывает и после краша).
/// 0 = реплика ядра пуста → fresh-sync. Оффлайн-изменения открытых строк ниже курсора
/// закрывает `check_open_rows`, min-open-правила больше нет.
fn startup_cursor(conn: &Connection, uid: i64) -> i64 {
    conn.query_row(
        &format!("SELECT MAX(newrecid) FROM {TABLE} WHERE core_uid=?1"),
        [uid],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
    .map(|m| m + 1)
    .unwrap_or(0)
}

/// Открытые строки ядра (closedate пустой/нулевой), свежие первыми, ≤100 — набор для
/// `check_open_rows` (lib сам сортирует/капит, но не гоняем лишнее через канал).
fn open_row_ids(conn: &Connection, cols: &HashSet<String>, uid: i64) -> Vec<i64> {
    if !cols.contains("closedate") {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT newrecid FROM {TABLE} \
         WHERE core_uid=?1 AND newrecid>0 AND (closedate IS NULL OR closedate<=0) \
         ORDER BY newrecid DESC LIMIT 100"
    )) {
        if let Ok(rows) = stmt.query_map([uid], |r| r.get::<_, i64>(0)) {
            out.extend(rows.flatten());
        }
    }
    out
}

/// Probe replica columns, surfacing SQLite's own error.
///
/// The writer needs the raw error to refuse startup; readers wrap it through
/// [`table_cols_res`].
fn table_cols_raw(conn: &Connection) -> rusqlite::Result<HashSet<String>> {
    let mut out = HashSet::new();
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({TABLE})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for n in rows {
        out.insert(n?.to_ascii_lowercase());
    }
    Ok(out)
}

/// Probe replica columns for [`init`], retrying a transient lock first.
///
/// The caller turns a failure into "no writer for the entire session", which is
/// far too high a price for the 3 s `busy_timeout` expiring under a checkpoint
/// or a second process. Only a failure that is NOT mere contention is worth
/// failing closed on — which is exactly the distinction
/// [`super::read_fail::classify`] exists to make.
fn table_cols_for_init(conn: &Connection) -> rusqlite::Result<HashSet<String>> {
    const ATTEMPTS: u32 = 3;
    let mut last = None;
    for attempt in 1..=ATTEMPTS {
        match table_cols_raw(conn) {
            Ok(cols) => return Ok(cols),
            Err(e) if super::read_fail::classify(&e) == super::FailKind::Busy => {
                log::warn!(
                    "отчёты(rep): PRAGMA table_info занят ({e}) — попытка {attempt} из {ATTEMPTS}"
                );
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(250 * u64::from(attempt)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("цикл выходит сюда только после ошибки занятости"))
}

/// Probe replica columns while distinguishing an absent schema from PRAGMA failure.
pub(super) fn table_cols_res(conn: &Connection) -> super::ReadResult<HashSet<String>> {
    use super::read_fail::read_fail;
    // Const, not `format!`: this runs on the healthy path of every schema probe
    // (2-3 per analytics query plus the writer init).
    const CTX: &str = "отчёты(rep): PRAGMA table_info(orders_rep)";
    table_cols_raw(conn).map_err(|e| read_fail(CTX, e))
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn meta_set_i64(conn: &Connection, key: &str, val: i64) {
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, val.to_string()],
    );
}
