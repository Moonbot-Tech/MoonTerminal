//! Локальная БД стратегий и их версий — `data/strategies.sqlite`.
//!
//! НАРОЧНО отдельный файл от `reports.sqlite`: реплика отчётов — восстановимый
//! кэш (сбрасывается `database_recreated`/пересинхроном), а история версий —
//! единственный экземпляр и живёт вечно. Кросс-БД аналитика («сделки × параметры
//! версии») — через `ATTACH` из читателя отчётов, скорость как в одной БД.
//!
//! Модель — порт mb_ai/moonbridge (`storage/strategies.rs` там):
//! - head-таблица `strategies` — одна строка на стратегию, UPSERT на каждый
//!   серверный снапшот (ключ `(core_uid, strategy_id)`, id — signed i64: ядро
//!   пишет `strategyid` ордера как Delphi signed, иначе join ломается);
//! - `strategy_versions` — append-only, НОВАЯ версия только если КОНТЕНТ
//!   отличается от последней после выкидывания игнор-полей (косметика, список —
//!   `config::storage`) и `__`-меты. Правка торговых полей → версия; тогл
//!   галки/звука/цвета → только head. Эхо-снапшоты дедупятся даром.
//!
//! Запись — на ПРИЁМЕ серверного снапшота (feed-цикл, не локальный эдит):
//! сервер — источник истины, так ловятся и правки чужих клиентов/MoonBot.exe.
//! Дампы приходят НОРМАЛИЗОВАННЫМИ: сервер не шлёт поля со значением, равным
//! дефолту схемы, поэтому feed материализует дефолты перед отправкой — иначе
//! фантомные версии от «исчезнувших» полей.

pub mod stats;
mod write;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};

use rusqlite::Connection;

use crate::config::paths;

/// Нормализованный дамп одной стратегии (снапшот сервера + дефолты схемы).
pub struct StratDump {
    /// Signed-представление серверного u64 id (join к `orders_rep.strategyid`).
    pub strategy_id: i64,
    pub name: String,
    pub kind: String,
    pub kind_ordinal: u8,
    pub folder_path: String,
    pub is_short: bool,
    pub checked: bool,
    /// Серверный счётчик правок из заголовка стратегии.
    pub server_ver: i32,
    /// Серверное время последней правки (unix-ms из `last_date`).
    pub server_ms: i64,
    /// Полный нормализованный дамп полей. `serde_json::Map` = BTreeMap →
    /// ключи отсортированы, сериализация канонична (стабильный контент-хэш).
    pub fields: serde_json::Map<String, serde_json::Value>,
    /// Правка пришла в ответ на НАШУ команду (кольцо недавних отправок feed'а).
    pub local_edit: bool,
}

/// Сообщение writer'у.
pub enum StratMsg {
    /// ПОЛНЫЙ набор стратегий ядра (реестр moonproto целиком): позволяет
    /// помечать пропавшие head-строки как удалённые. `initial` — первый набор
    /// после (ре)коннекта: origin неизвестен (правки могли случиться оффлайн).
    FullSet {
        core_uid: u64,
        core_name: String,
        initial: bool,
        strategies: Vec<StratDump>,
    },
}

/// Ёмкость канала: наборы приходят ≤1Гц с ядра и весят немного; переполнение
/// (writer занят) — дропаем набор, следующий снапшот всё догонит (дедуп).
const QUEUE_CAP: usize = 64;

#[derive(Clone)]
pub struct StratSink {
    tx: SyncSender<StratMsg>,
    generation: Arc<AtomicU64>,
}

impl StratSink {
    pub fn send(&self, msg: StratMsg) {
        match self.tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                log::warn!(
                    "стратегии(db): очередь writer'а полна — набор пропущен (догонит следующий)"
                );
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Растёт после каждой записи — читатели (история версий) перезапрашивают
    /// данные по смене поколения, без поллинга БД.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

/// Глобальный ленивый writer: первый снапшот стратегий поднимает поток.
/// Глобал (а не проброс через SessionManager→lifecycle→feed) — writer живёт
/// независимо от жизненного цикла сессий, как и сам файл БД.
static SINK: OnceLock<Option<StratSink>> = OnceLock::new();
/// Живой тогл записи (вкладка «Хранилище»): выключен — sink() отдаёт None,
/// writer простаивает. Инициализируется из `storage.toml` при первом обращении.
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Живой лимит версий на стратегию (0 = без лимита) — writer читает на каждую
/// запись, правка из вкладки «Хранилище» действует сразу.
static VERSION_LIMIT: AtomicU32 = AtomicU32::new(0);
static ENABLED_INIT: OnceLock<()> = OnceLock::new();

fn ensure_enabled_loaded() {
    ENABLED_INIT.get_or_init(|| {
        let cfg = crate::config::storage::load();
        ENABLED.store(cfg.strategies.enabled, Ordering::Relaxed);
        VERSION_LIMIT.store(cfg.strategies.version_limit, Ordering::Relaxed);
    });
}

/// Канал к writer'у; `None` — запись выключена в настройках или БД недоступна.
pub fn sink() -> Option<StratSink> {
    ensure_enabled_loaded();
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    SINK.get_or_init(spawn_writer).clone()
}

/// Текущее состояние тогла записи (для вкладки «Хранилище»).
pub fn is_enabled() -> bool {
    ensure_enabled_loaded();
    ENABLED.load(Ordering::Relaxed)
}

/// Живой тогл записи (вкладка «Хранилище» пишет и сюда, и в storage.toml).
pub fn set_enabled(on: bool) {
    ensure_enabled_loaded();
    ENABLED.store(on, Ordering::Relaxed);
}

/// Текущий лимит версий (0 = без лимита).
pub fn version_limit() -> u32 {
    ensure_enabled_loaded();
    VERSION_LIMIT.load(Ordering::Relaxed)
}

/// Живой лимит версий (вкладка «Хранилище» пишет и сюда, и в storage.toml).
pub fn set_version_limit(v: u32) {
    ensure_enabled_loaded();
    VERSION_LIMIT.store(v, Ordering::Relaxed);
}

/// Поколение записей (0 — writer не поднимался).
pub fn generation() -> u64 {
    SINK.get()
        .and_then(|s| s.as_ref())
        .map(|s| s.generation())
        .unwrap_or(0)
}

fn spawn_writer() -> Option<StratSink> {
    let path = paths::strategies_db_path();
    let conn = match Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            log::error!("стратегии(db): не удалось открыть {}: {e}", path.display());
            return None;
        }
    };
    let cfg = crate::config::storage::load().strategies;
    let mut state = match write::init(&conn, &cfg) {
        Ok(st) => st,
        Err(e) => {
            log::error!("стратегии(db): init схемы не удался: {e}");
            return None;
        }
    };
    let (tx, rx): (SyncSender<StratMsg>, Receiver<StratMsg>) =
        std::sync::mpsc::sync_channel(QUEUE_CAP);
    let generation = Arc::new(AtomicU64::new(0));
    let gen_writer = generation.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("strategies-db".into())
        .spawn(move || {
            log::info!("стратегии(db): writer запущен ({})", path.display());
            while let Ok(msg) = rx.recv() {
                match msg {
                    StratMsg::FullSet {
                        core_uid,
                        core_name,
                        initial,
                        strategies,
                    } => {
                        match write::apply_full_set(
                            &conn,
                            &mut state,
                            core_uid,
                            &core_name,
                            initial,
                            &strategies,
                        ) {
                            Ok(n) if n > 0 => {
                                gen_writer.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(_) => {}
                            Err(e) => log::error!(
                                "стратегии(db): набор ядра {core_uid} не записался: {e:#}"
                            ),
                        }
                    }
                }
            }
            log::info!("стратегии(db): writer завершён");
        })
    {
        log::error!("стратегии(db): не удалось запустить writer thread: {e}");
        return None;
    }
    Some(StratSink { tx, generation })
}

/// Tables in this store keyed by `core_uid`. `version_stats` is created lazily on the first
/// stats read, so its absence is an ordinary state rather than a damaged schema.
const CORE_UID_TABLES: [&str; 3] = ["strategies", "strategy_versions", "version_stats"];

/// Highest `core_uid` this store has ever recorded, across all three keyed tables.
///
/// Version history is the single copy and is never purged when a server is deleted, so a uid
/// surviving here must not be reissued — a new core would otherwise inherit the deleted one's
/// version history and cached per-version profit. `Ok(None)` means the store is genuinely
/// absent or empty; a read failure stays an error so the caller can refuse to treat an
/// unreadable store as "nothing to avoid".
///
/// Reports a failed open rather than reusing [`open_reader`], which collapses an absent file and
/// a failed open into the same `None`.
pub fn max_core_uid() -> crate::db::ReadResult<Option<u64>> {
    const CTX: &str = "стратегии: max_core_uid";
    let path = paths::strategies_db_path();
    if !path.exists() {
        // The writer is lazy — no strategy snapshot has arrived yet on a fresh install.
        return Ok(None);
    }
    let conn = open_ro(&path).map_err(|e| crate::db::read_fail::read_fail(CTX, e))?;
    let mut max: Option<u64> = None;
    for table in CORE_UID_TABLES {
        max = max.max(crate::db::max_core_uid_in(&conn, table, CTX)?);
    }
    Ok(max)
}

/// Read-only connection to the strategies file, with the store's shared busy timeout.
///
/// The one place the open flags live: [`open_reader`] swallows the error, [`max_core_uid`]
/// reports it, and both need the connection opened the same way.
fn open_ro(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    Ok(conn)
}

/// Read-only соединение (история версий, вкладка «Хранилище», аналитика).
/// Для кросс-БД запросов к отчётам читатель отчётов делает ATTACH этого файла.
pub fn open_reader() -> Option<Connection> {
    let path = paths::strategies_db_path();
    if !path.exists() {
        return None;
    }
    match open_ro(&path) {
        Ok(c) => Some(c),
        Err(e) => {
            log::warn!("стратегии(db): reader не открылся: {e}");
            None
        }
    }
}
