//! Persisted trade prints for replays: `trades.sqlite`, the disk under [`super::tick_tiles`].
//!
//! # What it is
//!
//! The tile store answers a neighbouring window inside one session; this file answers the same
//! question across a restart. Same shape — spans of `(from_ms, to_ms)` per exchange and market,
//! disjoint, holding RAW prints from the venue's route or from a core's archive, each row naming
//! which — persisted as one row per span, the prints packed into a blob. The worker hydrates the
//! tile store from here before it decides what to fetch, and writes every harvest and every
//! capture through after it filed it, so the two never disagree about what is held: the disk is
//! the tile store's memory, not a second cache with its own rules.
//!
//! # Why a separate file
//!
//! Not a table in `klines.sqlite`: prints are a different volume (a busy perpetual is a megabyte
//! per ten minutes where its bars are a kilobyte), keep their own ceiling, and are the one thing
//! the reader may want to switch off or delete without touching the bars. The Storage tab shows
//! this file on its own line with its own switch.
//!
//! # What is kept, and for how long
//!
//! Everything, until the reader's ceiling says otherwise: there is no age limit. The prints a
//! close copied out of a core's ring are the only copy there will ever be for a venue whose
//! trade route reaches back hours (Binance futures) or does not exist (Bybit, Hyperliquid), so
//! the file is an archive, not a cache. `[trade_replay] max_mb` is the one rule that runs on
//! its own — past it the spans written longest ago go first (by `updated_ms`, which only a
//! write sets — a replay that reads a span does not renew it) — and `0` keeps everything for
//! ever. The reader's own cut is the Storage tab's cleanup ([`trim`]) — by the button, or at
//! startup behind `[trade_replay] cleanup_at_startup`: down to the margin now in force around
//! the trades the tuner can be run on; deleting the file itself stays a call made with the
//! terminal closed.
//!
//! # Threading
//!
//! Exactly [`crate::market::kline_cache`]'s design: the connection is not `Sync`, so one worker
//! thread owns it and every call is a queued op. Writes are nonblocking; a read waits at most
//! [`READ_TIMEOUT`] and answers `None` for a timeout, which the caller treats as "nothing known"
//! — a fetch it could have avoided, never a wrong answer.
//!
//! # The switch
//!
//! `storage.toml` → `[trade_replay] persist_trades` (default on). Off, nothing is read from the
//! file and nothing is written to it by the replay path, and the tile store lives in memory
//! alone; a file this session already opened while the switch was on stays open, idle, until
//! the process exits — the handle is a `OnceLock`, and closing it under a queued write is not
//! worth a second state. The live value is an atomic the Storage tab flips without a restart,
//! initialised from the file on first use. The tab's own maintenance — the cleanup and its
//! preview — opens the file whatever the switch says ([`maintenance_handle`]): a file the
//! reader switched off is still a file the reader may want smaller.
//!
//! # Layout: two tables
//!
//! Prints are written to `packs`, compressed ([`codec`], ~4 bytes a print). `spans` is the
//! layout of every build before it — fixed 20-byte rows — and is still read: the worker moves
//! its rows into `packs` in small batches between queued ops ([`repack_batch`]), then compacts
//! the file once. Every read, every "what is held" and every cleanup looks at both tables, so a
//! span is served the same whichever one it sits in, and the two stay disjoint because a move
//! files only what the rest of the file does not already hold.
//!
//! Why a second table rather than a new `user_version`: a build that finds a version it does not
//! know drops the table (see [`SCHEMA_VERSION`]), and this is an archive. An older build opening
//! this file after a newer one finds version 3 and a `spans` table it can read — emptier, never
//! garbage — and writes its prints there; the next newer build moves them over.

mod codec;
mod trim;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use rusqlite::OptionalExtension;

use super::tick_tiles::TileSource;
use crate::feed::types::Tick;
pub use trim::{Inventory, KeepMap, TrimReport};

/// The two tables prints live in — see the module header's "Layout".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Table {
    /// `spans`: the fixed-width rows older builds write.
    Legacy,
    /// `packs`: compressed columns, what this build writes.
    Packs,
}

impl Table {
    const ALL: [Table; 2] = [Table::Legacy, Table::Packs];

    const fn name(self) -> &'static str {
        match self {
            Self::Legacy => "spans",
            Self::Packs => "packs",
        }
    }

    /// The SQL expression counting a row's prints without reading its blob.
    fn prints_sql(self) -> String {
        match self {
            // Integer division: a torn tail is not a print.
            Self::Legacy => format!("LENGTH(ticks) / {}", codec::LEGACY_ROW_BYTES),
            Self::Packs => "prints".to_string(),
        }
    }

    fn decode(self, blob: &[u8]) -> Result<Vec<Tick>, codec::DecodeError> {
        match self {
            Self::Legacy => Ok(codec::decode_legacy(blob)),
            Self::Packs => codec::decode(blob),
        }
    }
}

/// Legacy bytes moved into `packs` per batch. A batch runs between two queued ops, so this bounds
/// how long a read can wait behind the move: ~200 000 prints, tens of milliseconds to encode
/// (longest batch 129 ms over a 225 MB file, 2026-09-25). The one step that waits longer is the
/// compaction after the last batch — once per file, 0.3 s on the same file, since it rewrites the
/// packed result rather than the legacy bytes.
const REPACK_BATCH_BYTES: i64 = 4 * 1024 * 1024;

/// How long a read waits for the worker before answering `None`.
const READ_TIMEOUT: Duration = Duration::from_millis(250);

/// How long a maintenance call (inventory, trim) waits for the worker. Generous: a trim that
/// applies ends with a `VACUUM` over the whole file, and the call runs on a background thread
/// the Storage tab is not waiting on; a wait past this is the worker gone or wedged, and the
/// caller reports that rather than a number.
const MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(600);

/// Ceiling on the packed bytes the file may hold, from `[trade_replay] max_mb` — checked at
/// open and again after every insert; past it the spans written longest ago go first. Live,
/// like the switch: the Storage tab moves it without a restart. `None` when the reader set it
/// to zero and keeps everything, for ever.
fn max_bytes() -> Option<i64> {
    ensure_enabled_loaded();
    match MAX_MB.load(Ordering::Relaxed) {
        0 => None,
        mb => Some(i64::from(mb) * 1024 * 1024),
    }
}

/// The file layout this build writes and reads. A file whose `user_version` differs is a file in
/// a layout this build does not know — a print row of another width would unpack into garbage
/// stamps that `tick_tiles::insert` silently drops, filing the span as quiet — so its tables are
/// dropped and started over. Every build since 3 drops on a mismatch, which is why the packed
/// layout came as a second table under the same number rather than as a 4 (module header).
const SCHEMA_VERSION: i32 = 3;

/// One persisted span, as read back.
#[derive(Clone, Debug)]
pub struct StoredSpan {
    pub from_ms: i64,
    pub to_ms: i64,
    /// Ascending by time, all inside `[from_ms, to_ms]`.
    pub ticks: Vec<Tick>,
    /// Who answered — see [`TileSource`].
    pub source: TileSource,
}

enum Op {
    Insert {
        exchange: String,
        market: String,
        from_ms: i64,
        to_ms: i64,
        ticks: Vec<Tick>,
        source: TileSource,
    },
    Read {
        exchange: String,
        market: String,
        from_ms: i64,
        to_ms: i64,
        reply: mpsc::Sender<Vec<StoredSpan>>,
    },
    /// The bounds of every stored span intersecting a stretch — what is held, without reading
    /// a single print.
    Spans {
        exchange: String,
        market: String,
        from_ms: i64,
        to_ms: i64,
        reply: mpsc::Sender<rusqlite::Result<Vec<(i64, i64)>>>,
    },
    /// The file's markets and time range, for a caller building a [`KeepMap`].
    Inventory {
        reply: mpsc::Sender<rusqlite::Result<Inventory>>,
    },
    /// Clip every span to the map — counting only, or for real followed by a `VACUUM`.
    Trim {
        keep: Arc<KeepMap>,
        apply: bool,
        reply: mpsc::Sender<rusqlite::Result<TrimReport>>,
    },
}

/// Cheaply cloneable handle to the one worker.
#[derive(Clone)]
pub struct TradeCache {
    tx: mpsc::Sender<Op>,
}

impl TradeCache {
    /// Start the worker; it opens the file, runs the schema and the ceiling pass, then serves
    /// the queue. The handle comes back at once.
    ///
    /// Everything that touches the disk happens on the cache's OWN thread, never on the
    /// caller's: `handle()` is called from the trade-replay worker, whose queue holds every
    /// other window's candle and tick job, and a ceiling pass over a file this size is a
    /// blocking scan that has no business in front of them. An op queued before the open
    /// finishes simply waits in the channel; a read that waits longer than [`READ_TIMEOUT`]
    /// answers `None`, which costs one fetch and nothing else.
    ///
    /// Returns `None` only when the thread cannot be spawned. An open that fails on the thread
    /// logs once and lets the thread exit, after which every op fails the same way a closed
    /// channel does: reads answer `None`, writes are dropped.
    pub fn open(path: PathBuf) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Op>();
        std::thread::Builder::new()
            .name("trade-cache".into())
            .spawn(move || {
                let conn = match rusqlite::Connection::open(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("trade cache open failed {}: {e}", path.display());
                        return;
                    }
                };
                if let Err(e) = init_schema(&conn) {
                    log::warn!("trade cache schema failed {}: {e}", path.display());
                    return;
                }
                let held = match prune(&conn, max_bytes()) {
                    Ok(held) => held,
                    Err(e) => {
                        // The ceiling still needs a true count to work from: a zero here would
                        // hold the file's existing bytes exempt for the whole session.
                        log::warn!("trade cache ceiling pass failed {}: {e}", path.display());
                        held_bytes(&conn).unwrap_or(0)
                    }
                };
                crate::db::trace::install_on(&conn);
                log::info!("trade cache открыт: {}", path.display());
                run(conn, rx, held);
            })
            .ok()?;
        Some(Self { tx })
    }

    /// Queue one answered span. Nonblocking; the worker files only the stretches of it not yet
    /// held, exactly as the tile store does, so a re-fetch never doubles a print.
    pub fn insert(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
        ticks: Vec<Tick>,
        source: TileSource,
    ) {
        if from_ms > to_ms {
            return;
        }
        let _ = self.tx.send(Op::Insert {
            exchange: exchange.to_string(),
            market: market.to_string(),
            from_ms,
            to_ms,
            ticks,
            source,
        });
    }

    /// Every stored span intersecting `[from_ms, to_ms]`, ascending.
    ///
    /// `None` means the read did not happen (worker gone or busy past [`READ_TIMEOUT`]), which
    /// the caller must not mistake for "nothing stored".
    pub fn read(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Option<Vec<StoredSpan>> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(Op::Read {
                exchange: exchange.to_string(),
                market: market.to_string(),
                from_ms,
                to_ms,
                reply,
            })
            .ok()?;
        rx.recv_timeout(READ_TIMEOUT).ok()
    }

    /// The bounds `(from_ms, to_ms)` of every stored span of a market intersecting
    /// `[from_ms, to_ms]`, ascending — whether a stretch is held, answered off the span table's
    /// own columns, without unpacking a print. For a caller deciding what is worth asking the
    /// venue for over thousands of trades at once, where reading the prints themselves would
    /// cost as much as the fetch it is trying to spare.
    ///
    /// `None` means the read did not happen (worker gone or busy past [`READ_TIMEOUT`]) or
    /// failed; the caller must not mistake it for "nothing stored".
    pub fn held_spans(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Option<Vec<(i64, i64)>> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(Op::Spans {
                exchange: exchange.to_string(),
                market: market.to_string(),
                from_ms,
                to_ms,
                reply,
            })
            .ok()?;
        rx.recv_timeout(READ_TIMEOUT).ok()?.ok()
    }

    /// The file's markets and time range — see [`Inventory`].
    ///
    /// Waits for the worker up to [`MAINTENANCE_TIMEOUT`]; `None` means the worker is gone or
    /// did not answer in time, `Some(Err)` that the read itself failed.
    pub fn inventory(&self) -> Option<rusqlite::Result<Inventory>> {
        let (reply, rx) = mpsc::channel();
        self.tx.send(Op::Inventory { reply }).ok()?;
        rx.recv_timeout(MAINTENANCE_TIMEOUT).ok()
    }

    /// Clip every span to `keep` — see [`trim`]. With `apply`, the file is rewritten in one
    /// transaction and then compacted (`VACUUM`), so the bytes leave the disk, not only the
    /// table. Waits like [`Self::inventory`].
    pub fn trim(&self, keep: Arc<KeepMap>, apply: bool) -> Option<rusqlite::Result<TrimReport>> {
        let (reply, rx) = mpsc::channel();
        self.tx.send(Op::Trim { keep, apply, reply }).ok()?;
        rx.recv_timeout(MAINTENANCE_TIMEOUT).ok()
    }
}

/// The one process-wide cache, started on the first use that finds the switch on. `None` only
/// when its thread could not be spawned; a file that fails to open leaves the handle in place
/// with a dead channel behind it (see [`TradeCache::open`]), and the process does not retry —
/// it would only repeat the warning on every tick stage.
static CACHE: OnceLock<Option<TradeCache>> = OnceLock::new();
/// Live value of `[trade_replay] persist_trades`; the Storage tab flips it.
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Live value of `[trade_replay] max_mb`; the Storage tab moves it.
static MAX_MB: AtomicU32 = AtomicU32::new(crate::config::storage::DEFAULT_TRADES_MAX_MB);
static ENABLED_INIT: OnceLock<()> = OnceLock::new();

fn ensure_enabled_loaded() {
    ENABLED_INIT.get_or_init(|| {
        let cfg = crate::config::storage::load();
        ENABLED.store(cfg.trade_replay.persist_trades, Ordering::Relaxed);
        MAX_MB.store(cfg.trade_replay.max_mb, Ordering::Relaxed);
    });
}

/// Move the live ceiling; the Storage tab writes `storage.toml` beside this. Takes effect on
/// the next insert — a file already past a lowered ceiling is trimmed then, not at once.
pub fn set_max_mb(mb: u32) {
    ensure_enabled_loaded();
    MAX_MB.store(mb, Ordering::Relaxed);
}

/// Whether prints are persisted, for the Storage tab.
pub fn is_enabled() -> bool {
    ensure_enabled_loaded();
    ENABLED.load(Ordering::Relaxed)
}

/// Flip the live switch; the Storage tab writes `storage.toml` beside this.
pub fn set_enabled(on: bool) {
    ensure_enabled_loaded();
    ENABLED.store(on, Ordering::Relaxed);
}

/// The cache to read and write through, or `None` while the switch is off or the worker could
/// not be started. Starts it on the first call that finds the switch on; the file itself opens
/// on the worker's thread, and a file that will not open answers every read `None` from then on.
pub fn handle() -> Option<TradeCache> {
    if !is_enabled() {
        return None;
    }
    maintenance_handle()
}

/// The cache for the Storage tab's own maintenance — the cleanup and its preview —
/// whatever the switch says: it starts the worker if the switch kept [`handle`] from doing so.
/// It never CREATES the file, though: with the switch off and no file on disk there is nothing
/// to maintain, and opening the tab must not leave behind the very file the reader switched
/// off. `None` then, and when the worker could not be started.
pub fn maintenance_handle() -> Option<TradeCache> {
    let path = crate::config::paths::trades_db_path();
    if !is_enabled() && CACHE.get().is_none() && !path.exists() {
        return None;
    }
    CACHE.get_or_init(|| TradeCache::open(path)).clone()
}

fn init_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    crate::db::wal::enable(conn)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        if version != 0 {
            log::info!("trade cache: layout {version} → {SCHEMA_VERSION}, spans dropped");
        }
        conn.execute_batch("DROP TABLE IF EXISTS spans; DROP TABLE IF EXISTS packs;")?;
    }
    // `spans` stays created: an older build opening this file expects it (module header).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS spans(
            exchange TEXT NOT NULL,
            market TEXT NOT NULL,
            from_ms INTEGER NOT NULL,
            to_ms INTEGER NOT NULL,
            ticks BLOB NOT NULL,
            source INTEGER NOT NULL,
            updated_ms INTEGER NOT NULL,
            PRIMARY KEY(exchange, market, from_ms)
        );
        CREATE INDEX IF NOT EXISTS spans_updated ON spans(updated_ms);
        CREATE TABLE IF NOT EXISTS packs(
            exchange TEXT NOT NULL,
            market TEXT NOT NULL,
            from_ms INTEGER NOT NULL,
            to_ms INTEGER NOT NULL,
            ticks BLOB NOT NULL,
            prints INTEGER NOT NULL,
            source INTEGER NOT NULL,
            updated_ms INTEGER NOT NULL,
            PRIMARY KEY(exchange, market, from_ms)
        );
        CREATE INDEX IF NOT EXISTS packs_updated ON packs(updated_ms);",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    repair_gate_futures_offset_walks(conn)?;
    Ok(())
}

/// The one-off repair recorded in the file so it runs once: `(name, spans dropped)`.
const REPAIR_GATE_FUTURES_OFFSET: &str = "gate-futures-offset-pager";

/// Drop every VENUE span of a Gate futures market filed before 2026-09-21, once per file.
///
/// Those spans were walked by the endpoint's `offset`, which is not a cursor — under
/// back-to-back requests the venue answered the same page twice and skipped the next — so the
/// file holds them with prints repeated up to nine times and holes of half a minute inside a
/// stretch it calls covered (GSTOCKBSC_USDT, 2026-09-21: 985 rows, 459 distinct, no print
/// between +0.06 s and +29.9 s of a trade the venue serves 11 prints for within 2 s of the
/// close). Covered is final for the walk, so the only way to a whole tape is to forget them
/// and let the next request walk them again by time ([`super::rest::TradeCursor::Before`]).
/// The route documents no retention, and a three-day-old window came back whole.
///
/// The market's venue is read off the span's exchange key (`<ordinal>:<dex>`, the ordinal a
/// core reports for its platform); a key that names no known ordinal is left alone. Core
/// spans (source 1) are the core's own ring and never wrong this way.
fn repair_gate_futures_offset_walks(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS repairs(name TEXT PRIMARY KEY, spans_dropped INTEGER NOT NULL);",
    )?;
    let done: bool = conn.query_row(
        "SELECT COUNT(*) FROM repairs WHERE name = ?1",
        [REPAIR_GATE_FUTURES_OFFSET],
        |r| r.get::<_, i64>(0),
    )? > 0;
    if done {
        return Ok(());
    }
    let mut dropped = 0i64;
    for table in Table::ALL {
        let keys: Vec<String> = conn
            .prepare(&format!(
                "SELECT DISTINCT exchange FROM {} WHERE source = ?1",
                table.name()
            ))?
            .query_map([TileSource::Venue.code()], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for key in keys.iter().filter(|k| exchange_key_is_gate_futures(k)) {
            dropped += conn.execute(
                &format!(
                    "DELETE FROM {} WHERE exchange = ?1 AND source = ?2",
                    table.name()
                ),
                rusqlite::params![key, TileSource::Venue.code()],
            )? as i64;
        }
    }
    conn.execute(
        "INSERT INTO repairs(name, spans_dropped) VALUES(?1, ?2)",
        rusqlite::params![REPAIR_GATE_FUTURES_OFFSET, dropped],
    )?;
    if dropped > 0 {
        log::info!(
            "trade cache: {dropped} Gate futures span(s) walked by offset dropped, to be fetched again by time"
        );
    }
    Ok(())
}

/// Whether an exchange key (`<ordinal>:<dex>`) names a Gate futures platform.
fn exchange_key_is_gate_futures(key: &str) -> bool {
    key.split(':')
        .next()
        .and_then(|ordinal| ordinal.parse::<u8>().ok())
        .and_then(crate::venue::venue)
        .is_some_and(|v| {
            v.brand == crate::venue::Brand::Gate && v.kind == crate::venue::MarketKind::Futures
        })
}

/// Packed bytes the file holds, re-counted from both tables.
fn held_bytes(conn: &rusqlite::Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT (SELECT COALESCE(SUM(LENGTH(ticks)), 0) FROM spans)
              + (SELECT COALESCE(SUM(LENGTH(ticks)), 0) FROM packs)",
        [],
        |r| r.get(0),
    )
}

/// The open-time pass: the spans written longest ago go until the file fits under the byte
/// ceiling — and nothing else. No age limit: a span nobody opened for a year is still the only
/// copy of those prints (see the module header), and a reader who wants the file smaller has the
/// ceiling for it.
///
/// Returns:
///     The packed bytes the file holds afterwards, for the worker to carry forward.
fn prune(conn: &rusqlite::Connection, ceiling: Option<i64>) -> rusqlite::Result<i64> {
    trim_to_ceiling(conn, held_bytes(conn)?, ceiling)
}

/// Drop the spans written longest ago until `held` packed bytes fit under `ceiling`.
///
/// "Written", not "used": `updated_ms` is set by [`insert_span`] alone, a read leaves it as it
/// was, so the order is the order the prints were filed in — the oldest trades' prints go first.
///
/// The ceiling is a parameter, not read here: the worker resolves the live setting
/// ([`max_bytes`]) at each call, and a test hands in a number of its own.
///
/// Args:
///     conn: The open connection.
///     held: Packed bytes the file holds now.
///     ceiling: Packed bytes allowed, or `None` for no ceiling.
///
/// Returns:
///     Packed bytes held afterwards.
fn trim_to_ceiling(
    conn: &rusqlite::Connection,
    mut held: i64,
    ceiling: Option<i64>,
) -> rusqlite::Result<i64> {
    let Some(ceiling) = ceiling else {
        return Ok(held);
    };
    while held > ceiling {
        // One span at a time, oldest first: the excess is usually one wide harvest, and a
        // batch would take a fresh neighbour down with it. Each table answers its own oldest
        // off its `updated_ms` index; the older of the two goes.
        let mut oldest: Option<(Table, i64, i64, i64)> = None;
        for table in Table::ALL {
            let row: Option<(i64, i64, i64)> = conn
                .query_row(
                    &format!(
                        "SELECT rowid, LENGTH(ticks), updated_ms FROM {}
                         ORDER BY updated_ms ASC, rowid ASC LIMIT 1",
                        table.name()
                    ),
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((rowid, bytes, updated_ms)) = row else {
                continue;
            };
            if oldest.is_none_or(|(.., held_updated)| updated_ms < held_updated) {
                oldest = Some((table, rowid, bytes, updated_ms));
            }
        }
        let Some((table, rowid, bytes, _)) = oldest else {
            break;
        };
        conn.execute(
            &format!("DELETE FROM {} WHERE rowid = ?1", table.name()),
            [rowid],
        )?;
        held -= bytes;
    }
    Ok(held)
}

/// The worker: serves the queue, carrying the file's packed byte count so the ceiling is held
/// after every insert rather than only at open. While `spans` still holds rows it moves them
/// into `packs` a batch at a time, only when no op is waiting — an op never queues behind more
/// than one batch.
fn run(conn: rusqlite::Connection, rx: mpsc::Receiver<Op>, mut held: i64) {
    let mut repack = match legacy_rows_left(&conn) {
        Ok(true) => Some(RepackTally::start()),
        Ok(false) => None,
        Err(e) => {
            log::warn!("trade cache: legacy table unreadable, not repacked: {e}");
            None
        }
    };
    loop {
        let op = if let Some(tally) = repack.as_mut() {
            match rx.try_recv() {
                Ok(op) => op,
                Err(mpsc::TryRecvError::Empty) => {
                    if !tally.step(&conn, &mut held) {
                        repack = None;
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(op) => op,
                Err(_) => return,
            }
        };
        serve(&conn, op, &mut held);
    }
}

/// One queued op.
fn serve(conn: &rusqlite::Connection, op: Op, held: &mut i64) {
    match op {
        Op::Insert {
            exchange,
            market,
            from_ms,
            to_ms,
            ticks,
            source,
        } => {
            let now = crate::util::time::now_unix_ms_i64();
            let res = conn.unchecked_transaction().and_then(|tx| {
                let wrote =
                    insert_span(&tx, &exchange, &market, from_ms, to_ms, &ticks, source, now)?;
                tx.commit()?;
                Ok(wrote)
            });
            match res {
                Ok(wrote) => {
                    *held += wrote;
                    match trim_to_ceiling(conn, *held, max_bytes()) {
                        Ok(now_held) => *held = now_held,
                        Err(e) => {
                            // Rows may already be gone: re-count rather than carry an
                            // overcount that would evict fresh spans on the next insert.
                            log::warn!("trade cache ceiling failed: {e}");
                            *held = held_bytes(conn).unwrap_or(*held);
                        }
                    }
                }
                Err(e) => log::warn!("trade cache insert failed {exchange}/{market}: {e}"),
            }
        }
        Op::Read {
            exchange,
            market,
            from_ms,
            to_ms,
            reply,
        } => {
            let mut dropped = 0i64;
            let spans =
                match read_spans_dropping(conn, &exchange, &market, from_ms, to_ms, &mut dropped) {
                    Ok(spans) => spans,
                    Err(e) => {
                        log::warn!("trade cache read failed {exchange}/{market}: {e}");
                        Vec::new()
                    }
                };
            // A row that did not decode left the file: the ceiling must not keep counting it.
            *held -= dropped;
            let _ = reply.send(spans);
        }
        Op::Spans {
            exchange,
            market,
            from_ms,
            to_ms,
            reply,
        } => {
            let spans = read_span_bounds(conn, &exchange, &market, from_ms, to_ms);
            if let Err(e) = &spans {
                log::warn!("trade cache span read failed {exchange}/{market}: {e}");
            }
            let _ = reply.send(spans);
        }
        Op::Inventory { reply } => {
            let _ = reply.send(trim::inventory(conn));
        }
        Op::Trim { keep, apply, reply } => {
            let result = trim::trim(conn, &keep, apply);
            if apply {
                // Whatever the pass did, the carried count must be the file's: a failed
                // transaction rolled back to what it was, a committed one removed rows.
                *held = held_bytes(conn).unwrap_or(*held);
                if let Ok(report) = &result {
                    log::info!(
                        "trade cache trimmed: {} span(s) dropped, {} cut, {} print(s) / {} bytes gone of {}",
                        report.spans_dropped,
                        report.spans_cut,
                        report.prints_dropped,
                        report.bytes_dropped,
                        report.bytes_total
                    );
                    // The bytes leave the disk only here; the table alone would keep the
                    // file its old size and the tab's readout would not move.
                    if let Err(e) = crate::db::wal::vacuum(conn) {
                        log::warn!("trade cache vacuum after trim failed: {e}");
                    }
                }
            }
            let _ = reply.send(result);
        }
    }
}

/// Whether the legacy table still holds a row to move.
fn legacy_rows_left(conn: &rusqlite::Connection) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM spans)", [], |r| r.get(0))
}

/// The move of `spans` into `packs` over one session, counted for the one log line it ends with.
struct RepackTally {
    started: Instant,
    batches: u32,
    spans: u64,
    prints: u64,
    legacy_bytes: i64,
    packed_bytes: i64,
    /// Time spent inside batches — what queued ops could have waited behind.
    busy: Duration,
    /// The longest single batch — the longest any one op could have waited.
    longest: Duration,
}

impl RepackTally {
    fn start() -> Self {
        Self {
            started: Instant::now(),
            batches: 0,
            spans: 0,
            prints: 0,
            legacy_bytes: 0,
            packed_bytes: 0,
            busy: Duration::ZERO,
            longest: Duration::ZERO,
        }
    }

    /// Move one batch; when the legacy table is empty afterwards, compact the file and log.
    ///
    /// Returns:
    ///     `true` while there is more to move; `false` once done, or after a failure, which
    ///     leaves the rest where it is for the next session — both tables are read either way.
    fn step(&mut self, conn: &rusqlite::Connection, held: &mut i64) -> bool {
        let t = Instant::now();
        let batch = repack_batch(conn, REPACK_BATCH_BYTES);
        let took = t.elapsed();
        self.busy += took;
        self.longest = self.longest.max(took);
        let batch = match batch {
            Ok(batch) => batch,
            Err(e) => {
                log::warn!(
                    "trade cache repack stopped after {} span(s): {e}",
                    self.spans
                );
                *held = held_bytes(conn).unwrap_or(*held);
                return false;
            }
        };
        self.batches += 1;
        self.spans += batch.spans;
        self.prints += batch.prints;
        self.legacy_bytes += batch.legacy_bytes;
        self.packed_bytes += batch.packed_bytes;
        *held += batch.packed_bytes - batch.legacy_bytes;
        if batch.more {
            return true;
        }
        let moved = self.started.elapsed();
        let t = Instant::now();
        if let Err(e) = crate::db::wal::vacuum(conn) {
            log::warn!("trade cache vacuum after repack failed: {e}");
        }
        let compacted = t.elapsed();
        *held = held_bytes(conn).unwrap_or(*held);
        log::info!(
            "[x] trade cache repacked: {} span(s), {} print(s), {:.1} MB → {:.1} MB in {} batch(es), \
             {:.1} s wall / {:.1} s busy / longest batch {} ms; compacted in {:.1} s",
            self.spans,
            self.prints,
            self.legacy_bytes as f64 / 1e6,
            self.packed_bytes as f64 / 1e6,
            self.batches,
            moved.as_secs_f64(),
            self.busy.as_secs_f64(),
            self.longest.as_millis(),
            compacted.as_secs_f64()
        );
        false
    }
}

/// What one [`repack_batch`] moved.
#[derive(Debug, Default)]
struct RepackBatch {
    spans: u64,
    prints: u64,
    legacy_bytes: i64,
    packed_bytes: i64,
    /// Whether the legacy table still holds rows afterwards.
    more: bool,
}

/// Move the oldest legacy rows, up to `budget` bytes of them (always at least one), into
/// `packs`, in one transaction. Each row keeps its bounds, its source and its `updated_ms` — the
/// ceiling's eviction order is the order the prints were first written, not the move's.
///
/// The row goes before its prints are filed: [`insert_span`] files only what the file does not
/// already hold, and the row itself would otherwise be what holds it.
fn repack_batch(conn: &rusqlite::Connection, budget: i64) -> rusqlite::Result<RepackBatch> {
    let candidates: Vec<(i64, i64)> = conn
        .prepare("SELECT rowid, LENGTH(ticks) FROM spans ORDER BY rowid LIMIT 256")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut picked = Vec::new();
    let mut picked_bytes = 0i64;
    for (rowid, bytes) in candidates {
        if !picked.is_empty() && picked_bytes + bytes > budget {
            break;
        }
        picked.push(rowid);
        picked_bytes += bytes;
    }
    let mut batch = RepackBatch::default();
    let tx = conn.unchecked_transaction()?;
    for rowid in picked {
        let row: LegacyRow = tx.query_row(
            "SELECT exchange, market, from_ms, to_ms, ticks, source, updated_ms
             FROM spans WHERE rowid = ?1",
            [rowid],
            |r| {
                Ok(LegacyRow {
                    exchange: r.get(0)?,
                    market: r.get(1)?,
                    from_ms: r.get(2)?,
                    to_ms: r.get(3)?,
                    blob: r.get(4)?,
                    source: r.get(5)?,
                    updated_ms: r.get(6)?,
                })
            },
        )?;
        let ticks = codec::decode_legacy(&row.blob);
        tx.execute("DELETE FROM spans WHERE rowid = ?1", [rowid])?;
        batch.packed_bytes += insert_span(
            &tx,
            &row.exchange,
            &row.market,
            row.from_ms,
            row.to_ms,
            &ticks,
            TileSource::from_code(row.source),
            row.updated_ms,
        )?;
        batch.spans += 1;
        batch.prints += ticks.len() as u64;
        batch.legacy_bytes += row.blob.len() as i64;
    }
    tx.commit()?;
    batch.more = legacy_rows_left(conn)?;
    Ok(batch)
}

/// One `spans` row, whole, as [`repack_batch`] moves it.
struct LegacyRow {
    exchange: String,
    market: String,
    from_ms: i64,
    to_ms: i64,
    blob: Vec<u8>,
    source: i64,
    updated_ms: i64,
}

/// File `[from_ms, to_ms]` into `packs` as one row per stretch of it not already stored in
/// either table, each holding the prints of `ticks` inside that stretch. A span already covered
/// whole writes nothing.
///
/// What is held comes from the span bounds alone ([`read_span_bounds`]): deciding the gaps never
/// needs a print, and unpacking every neighbour to learn its edges would cost more than the write.
///
/// Args:
///     updated_ms: The row's write stamp — now for a fresh answer, the original stamp for a
///         row [`repack_batch`] moves, so the ceiling's order survives the move.
///
/// Returns:
///     Packed bytes written.
#[allow(clippy::too_many_arguments)]
fn insert_span(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
    ticks: &[Tick],
    source: TileSource,
    updated_ms: i64,
) -> rusqlite::Result<i64> {
    let mut written = 0i64;
    let held = read_span_bounds(conn, exchange, market, from_ms, to_ms)?;
    let mut sorted: Vec<Tick> = ticks
        .iter()
        .copied()
        .filter(|t| t.time_ms.is_finite())
        .collect();
    sorted.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    for (gap_from, gap_to) in super::tick_tiles::gaps_between(&held, from_ms, to_ms) {
        let inside: Vec<Tick> = sorted
            .iter()
            .copied()
            .filter(|t| {
                let time_ms = t.time_ms as i64;
                time_ms >= gap_from && time_ms <= gap_to
            })
            .collect();
        let blob = codec::encode(&inside);
        written += blob.len() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO packs(exchange, market, from_ms, to_ms, ticks, prints, source, updated_ms)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                exchange,
                market,
                gap_from,
                gap_to,
                blob,
                inside.len() as i64,
                source.code(),
                updated_ms
            ],
        )?;
    }
    Ok(written)
}

/// Every span of the key intersecting `[from_ms, to_ms]`, from both tables, ascending by
/// `from_ms`.
///
/// A packed row that does not decode is deleted and left out, with a warning: kept, it would
/// still count as held and its stretch would never be fetched again; gone, the next request
/// fetches it like any stretch the file never had.
#[cfg(test)]
fn read_spans(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<StoredSpan>> {
    read_spans_dropping(conn, exchange, market, from_ms, to_ms, &mut 0)
}

/// [`read_spans`], adding the blob bytes of every row it deleted to `dropped` — the worker
/// carries the file's byte count and must take them off it.
fn read_spans_dropping(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
    dropped: &mut i64,
) -> rusqlite::Result<Vec<StoredSpan>> {
    let mut out = Vec::new();
    for table in Table::ALL {
        let mut broken = Vec::new();
        {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT rowid, from_ms, to_ms, ticks, source FROM {}
                 WHERE exchange = ?1 AND market = ?2 AND to_ms >= ?3 AND from_ms <= ?4",
                table.name()
            ))?;
            let mut rows = stmt.query(rusqlite::params![exchange, market, from_ms, to_ms])?;
            while let Some(r) = rows.next()? {
                let rowid: i64 = r.get(0)?;
                let blob = r.get_ref(3)?.as_blob()?;
                match table.decode(blob) {
                    Ok(ticks) => out.push(StoredSpan {
                        from_ms: r.get(1)?,
                        to_ms: r.get(2)?,
                        ticks,
                        source: TileSource::from_code(r.get(4)?),
                    }),
                    Err(e) => broken.push((rowid, blob.len() as i64, e)),
                }
            }
        }
        for (rowid, bytes, e) in broken {
            log::warn!(
                "trade cache: a {} span of {exchange}/{market} does not decode ({e}); dropped to be fetched again",
                table.name()
            );
            conn.execute(
                &format!("DELETE FROM {} WHERE rowid = ?1", table.name()),
                [rowid],
            )?;
            *dropped += bytes;
        }
    }
    out.sort_by_key(|s| s.from_ms);
    Ok(out)
}

/// The bounds of every span of a market intersecting `[from_ms, to_ms]`, from both tables,
/// ascending — the columns alone, never the blob.
fn read_span_bounds(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT from_ms, to_ms FROM spans
         WHERE exchange = ?1 AND market = ?2 AND to_ms >= ?3 AND from_ms <= ?4
         UNION ALL
         SELECT from_ms, to_ms FROM packs
         WHERE exchange = ?1 AND market = ?2 AND to_ms >= ?3 AND from_ms <= ?4
         ORDER BY 1",
    )?;
    let rows = stmt.query_map(rusqlite::params![exchange, market, from_ms, to_ms], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests;
