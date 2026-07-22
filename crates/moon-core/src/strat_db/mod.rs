//! Local database of strategies and their versions: `data/strategies.sqlite`.
//!
//! Kept separate from `reports.sqlite` by design: the reports replica is a recoverable
//! cache (reset by `database_recreated`/resynchronization), while version history is the
//! authoritative copy. Its per-strategy retention is bounded by the configured `version_limit`
//! when a new version is inserted; lowering the limit does not globally prune existing history,
//! and each strategy is pruned only on its next version insertion. Cross-database analytics
//! ("trades x version parameters") uses `ATTACH` from the reports reader, with single-database performance.
//!
//! The model is ported from mb_ai/moonbridge (its `storage/strategies.rs`):
//! - the `strategies` head table has one row per strategy; every server snapshot is evaluated,
//!   and the row is upserted only when its head or content changes (keyed by
//!   `(core_uid, strategy_id)`; the id is a signed i64 because
//!   the core writes an order's `strategyid` using Delphi's signed representation, or the join fails);
//! - `strategy_versions` inserts a new version only when non-ignored content differs from the
//!   latest version after excluding configured `ignore_fields` and `__` metadata. The configured
//!   fields include cosmetic, status, and presentation fields plus the `OrderSize` policy; their
//!   changes update the head when represented there and otherwise create no version. Inserting a
//!   revision closes the previous range by updating `valid_to`; restoring unchanged content can
//!   instead reopen the latest range. Echoed snapshots deduplicate naturally.
//!
//! Writes happen when a server snapshot is received (in the feed loop, not on a local edit):
//! the server is the source of truth, so this also captures edits from other clients/MoonBot.exe.
//! Dumps arrive normalized: the server omits fields whose values equal schema defaults, so the
//! feed materializes defaults before sending them, preventing phantom versions from "vanished" fields.

pub mod stats;
mod write;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};

use rusqlite::Connection;

use crate::config::paths;

/// Normalized dump of one strategy (server snapshot plus schema defaults).
pub struct StratDump {
    /// Signed representation of the server's u64 id (joined to `orders_rep.strategyid`).
    pub strategy_id: i64,
    pub name: String,
    pub kind: String,
    pub kind_ordinal: u8,
    pub folder_path: String,
    pub is_short: bool,
    pub checked: bool,
    /// Server-side edit counter from the strategy header.
    pub server_ver: i32,
    /// Server time of the latest edit (Unix milliseconds from `last_date`).
    pub server_ms: i64,
    /// Complete normalized field dump. `serde_json::Map` is a BTreeMap, so keys are sorted
    /// and serialization is canonical (yielding a stable content hash).
    pub fields: serde_json::Map<String, serde_json::Value>,
    /// Heuristic classification that the edit likely responds to our command, based on the feed's
    /// recent-send ring.
    pub local_edit: bool,
}

/// Message sent to the writer.
pub enum StratMsg {
    /// Complete set of strategies from the core (the entire moonproto registry), allowing
    /// missing head rows to be marked as deleted. `initial` marks the first set after a
    /// (re)connection, when the origin is unknown because edits may have happened offline.
    FullSet {
        core_uid: u64,
        core_name: String,
        initial: bool,
        strategies: Vec<StratDump>,
    },
}

/// Channel capacity: sets arrive from the core at no more than 1 Hz and are small. If the
/// writer is busy and the channel fills, the set is dropped; the next snapshot catches up via deduplication.
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

    /// Increases after each write that changes rows, allowing version-history readers to
    /// reload on generation changes without polling the database.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

/// Global lazy writer: the first strategy snapshot starts the thread.
/// It is global rather than passed through SessionManager -> lifecycle -> feed, so the writer,
/// like the database file itself, lives independently of session lifecycles.
static SINK: OnceLock<Option<StratSink>> = OnceLock::new();
/// Live write toggle (the Storage tab): when disabled, future `sink()` calls return `None`.
/// Existing sink clones and queued messages are not revoked. Initialized from `storage.toml` on first access.
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Live per-strategy version limit (0 means unlimited), consulted when a new version is inserted.
/// Pruning then applies only to that strategy; lowering the limit does not prune existing history immediately.
static VERSION_LIMIT: AtomicU32 = AtomicU32::new(0);
static ENABLED_INIT: OnceLock<()> = OnceLock::new();

fn ensure_enabled_loaded() {
    ENABLED_INIT.get_or_init(|| {
        let cfg = crate::config::storage::load();
        ENABLED.store(cfg.strategies.enabled, Ordering::Relaxed);
        VERSION_LIMIT.store(cfg.strategies.version_limit, Ordering::Relaxed);
    });
}

/// Channel to the writer; `None` means writing is disabled in settings or the database is unavailable.
pub fn sink() -> Option<StratSink> {
    ensure_enabled_loaded();
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    SINK.get_or_init(spawn_writer).clone()
}

/// Current state of the write toggle for the Storage tab.
pub fn is_enabled() -> bool {
    ensure_enabled_loaded();
    ENABLED.load(Ordering::Relaxed)
}

/// Updates the live write toggle; the Storage tab writes both here and to `storage.toml`.
pub fn set_enabled(on: bool) {
    ensure_enabled_loaded();
    ENABLED.store(on, Ordering::Relaxed);
}

/// Current version limit (0 means unlimited).
pub fn version_limit() -> u32 {
    ensure_enabled_loaded();
    VERSION_LIMIT.load(Ordering::Relaxed)
}

/// Updates the live version limit; the Storage tab writes both here and to `storage.toml`.
/// The new limit is applied to a strategy on its next version insertion.
pub fn set_version_limit(v: u32) {
    ensure_enabled_loaded();
    VERSION_LIMIT.store(v, Ordering::Relaxed);
}

/// Write generation (0 until the writer records its first change).
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
/// version history and cached per-version profit. Once the path passes its existence check, open
/// and query failures stay errors; `Ok(None)` means no accessible file or no rows in its keyed
/// tables.
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

/// Read-only connection for version history, the Storage tab, and analytics.
/// For cross-database report queries, the reports reader attaches this file with `ATTACH`.
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
