//! Typed-реплика БД отчёта Orders (moonproto `Event::Report`, docs/reports.md) —
//! таблица `orders_rep`.
//!
//! Схема приходит от ядра (`ReportEvent::Schema`, append-only), имена колонок храним
//! lowercase — они совпадают с легаси-именами (панель «Отчёт» форматирует по ним), а
//! SQLite к регистру безразличен. Ключ репликации — `(core_uid, newrecid)`;
//! `newRecID` ≠ легаси `db_id`, поэтому typed-поток и легаси close-SQL пишут в РАЗНЫЕ
//! таблицы (легаси вычищается по мере первых полных sync'ов и сносится целиком).
//!
//! Курсор (правило docs/reports.md): min(newRecID открытых строк) ИЛИ committed_max+1,
//! где committed_max — максимум из ПОДТВЕРЖДЁННЫХ `SyncComplete` (не max таблицы: батчи
//! catch-up приходят вне порядка, и max прерванной догонки перепрыгнул бы дыры).
//! 0 = fresh-sync со всей удержанной историей ядра.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use moonproto::{
    MoonReports, ReportRow as RepRow, ReportSchema, ReportSyncComplete, ReportSyncPage,
    ReportValue,
};
use rusqlite::types::Value;
use rusqlite::Connection;

pub(super) const TABLE: &str = "orders_rep";

/// Сообщение SQLite-writer'у (единственному владельцу соединения на запись).
pub enum DbMsg {
    /// Легаси close-SQL поток (deprecated в moonproto). Живёт до сноса легаси-таблицы
    /// и пишет ТОЛЬКО в неё — потоки в одну таблицу не смешиваются.
    Legacy(super::ReportRow),
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

    pub fn legacy(&self, row: super::ReportRow) {
        let _ = self.tx.send(DbMsg::Legacy(row));
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
    /// Легаси-таблица ещё существует (пустеет по мере первых полных sync'ов ядер).
    pub(super) legacy_exists: bool,
    /// Ядра с завершённым sync (rep_synced_*=1): их легаси close-SQL ИГНОРИРУЕТСЯ —
    /// данные идут typed-потоком, а легаси-вставка после вычистки давала ДУБЛИ строк
    /// (реплика + свежая легаси-копия) в объединённом читателе.
    pub(super) synced: HashSet<u64>,
    /// Легаси-таблица только что снесена → писателю нужно прогнать VACUUM ПОСЛЕ
    /// коммита батча (внутри транзакции VACUUM запрещён): без него файл БД остаётся
    /// прежних сотен МБ.
    pub(super) vacuum_pending: bool,
    /// Индекс `idx_rep_strat` уже создан в этой сессии (не дёргать CREATE на
    /// каждую схему ядра).
    strat_index_done: bool,
}

/// Создаёт скелет таблицы реплики (колонки доращивает схема ядра), считает стартовые
/// курсоры и наборы открытых строк всех ядер, присутствующих в реплике.
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
    let cols = table_cols(conn);
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
    let mut st = RepState {
        schemas: HashMap::new(),
        cols,
        cursors,
        legacy_exists,
        synced,
        vacuum_pending: false,
        strat_index_done: false,
    };
    // Вычистка легаси-строк синхронизированных ядер, накопившихся ПОСЛЕ их SyncComplete
    // (легаси-поток успевал дописывать до фикса скипа) — иначе дубли в читателе.
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
        log::info!("отчёты(rep): легаси-таблица closed_sell_reports снесена — все ядра на typed-реплике");
    }
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
        if !super::parse::valid_ident(&name) {
            log::warn!("отчёты(rep): поле схемы «{}» — не идентификатор, пропущено", f.name);
            continue;
        }
        match conn.execute(
            &format!("ALTER TABLE {TABLE} ADD COLUMN \"{name}\" {}", f.sql_spec),
            [],
        ) {
            Ok(_) => {
                log::info!("отчёты(rep): колонка «{name}» {} (схема ядра {core_uid})", f.sql_spec);
                // Индекс под дефолтный фильтр периода окна «Отчёт» (как у легаси-таблицы).
                if name == "closedate" {
                    let _ = conn.execute(
                        &format!("CREATE INDEX IF NOT EXISTS idx_rep_closedate ON {TABLE}(closedate)"),
                        [],
                    );
                }
                st.cols.insert(name);
            }
            Err(e) => log::error!("отчёты(rep): ADD COLUMN {name} не удался: {e}"),
        }
    }
    // Индекс под аналитику версий стратегий (strat_db): выборка сделок одной
    // стратегии + range-join по buydate к valid_from/valid_to версии. Колонки —
    // авто (из схемы ядра), поэтому создаём здесь, когда обе точно есть;
    // IF NOT EXISTS делает повторные вызовы бесплатными.
    if !st.strat_index_done && st.cols.contains("strategyid") && st.cols.contains("buydate") {
        match conn.execute(
            &format!(
                "CREATE INDEX IF NOT EXISTS idx_rep_strat ON {TABLE}(core_uid, strategyid, buydate)"
            ),
            [],
        ) {
            Ok(_) => st.strat_index_done = true,
            Err(e) => log::warn!("отчёты(rep): индекс idx_rep_strat не создался: {e}"),
        }
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
    st.synced.insert(core_uid);
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

    // Легаси: typed-реплика с полной историей заменяет строки этого ядра (дальнейший
    // легаси-поток ядра игнорируется — см. `synced`).
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

pub(super) fn table_cols(conn: &Connection) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({TABLE})")) {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) {
            for n in rows.flatten() {
                out.insert(n.to_ascii_lowercase());
            }
        }
    }
    out
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
