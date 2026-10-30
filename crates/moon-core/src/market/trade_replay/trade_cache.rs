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
//! per ten minutes where its bars are a kilobyte) and keep their own ceiling. The Storage tab
//! shows this file separately from the candle cache.
//!
//! # What is kept, and for how long
//!
//! Everything, until the built-in ceiling says otherwise: there is no age limit. The prints a
//! close copied out of a core's ring are the only copy there will ever be for a venue whose
//! trade route reaches back hours (Binance futures) or does not exist (Bybit, Hyperliquid), so
//! the file is an archive, not a cache. The built-in byte cap runs on its own: past it the spans
//! written longest ago go first (by `updated_ms`, which only a write sets; a replay read does
//! not renew it). Startup cleanup ([`trim`]) retains the current margin around tunable trades
//! before autoload fills their missing tape.
//!
//! # Threading
//!
//! Exactly [`crate::market::kline_cache`]'s design: the connection is not `Sync`, so one worker
//! thread owns it and every call is a queued op. Background writes wait when the bounded queue is
//! full; reads use nonblocking admission and wait at most [`READ_TIMEOUT`], answering `None` for a
//! timeout, which the caller treats as "nothing known"
//! — a fetch it could have avoided, never a wrong answer.
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
mod evict;
mod filed;
mod remote;
mod trim;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use rusqlite::OptionalExtension;

use super::tick_tiles::TileSource;
use crate::feed::types::Tick;
pub use filed::filed_since;
pub(super) use filed::market_revision;
pub(super) use filed::note as note_filed;
pub use remote::{TapeFile, decode_prints, encode_prints};
pub use trim::{Compact, CompactFail, Inventory, KeepMap, TrimOutcome, TrimProgress, TrimReport};

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

    /// The overlap read shared by archive decoding and its query-plan regression test.
    fn read_sql(self) -> String {
        format!(
            "SELECT rowid, from_ms, to_ms, ticks, source FROM {}
             WHERE exchange = ?1 AND market = ?2 AND to_ms >= ?3 AND from_ms <= ?4",
            self.name()
        )
    }

    /// Selects this table's oldest victim using the same age and rowid ordering as eviction.
    const fn oldest_sql(self) -> &'static str {
        match self {
            Self::Legacy => {
                "SELECT rowid, LENGTH(ticks), updated_ms FROM spans
                         ORDER BY updated_ms ASC, rowid ASC LIMIT 1"
            }
            Self::Packs => {
                "SELECT rowid, LENGTH(ticks), updated_ms FROM packs
                         ORDER BY updated_ms ASC, rowid ASC LIMIT 1"
            }
        }
    }

    /// Deletes the selected row without allocating SQL on every insert.
    const fn delete_sql(self) -> &'static str {
        match self {
            Self::Legacy => "DELETE FROM spans WHERE rowid = ?1",
            Self::Packs => "DELETE FROM packs WHERE rowid = ?1",
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

/// Operations admitted before background producers wait; one oversized span remains lossless.
const OP_QUEUE: usize = 256;

/// How long a read waits for the worker before answering `None`.
const READ_TIMEOUT: Duration = Duration::from_millis(250);

/// How long a maintenance call (inventory, trim) waits for the worker. Generous: a trim that
/// applies ends with a `VACUUM` over the whole file, and the call runs on a background thread
/// the Storage tab is not waiting on; a wait past this is the worker gone or wedged, and the
/// caller reports that rather than a number.
const MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(600);

/// Built-in packed-print cap; the terminal also preserves the shared filesystem reserve.
pub const TRADES_MAX_MB: u32 = 131_072;

/// Supply the built-in cap, independently of the worker's cached disk clamp.
fn max_bytes() -> Option<i64> {
    Some(i64::from(TRADES_MAX_MB) * 1024 * 1024)
}

/// Filesystem measurement injected into the production worker, absent for custom ceilings.
type DiskProbe = fn(&Path) -> Option<(u64, u64)>;

/// Probe-time packed limit stays fixed between samples so inserts consume headroom once.
struct DiskCeiling {
    path: PathBuf,
    probe: DiskProbe,
    sampled: Option<(Instant, Option<u64>, bool)>,
    interval: Duration,
}

impl DiskCeiling {
    /// Bind a probe to the database whose main and WAL bytes determine packing density.
    fn new(path: PathBuf, probe: DiskProbe) -> Self {
        Self {
            path,
            probe,
            sampled: None,
            interval: Duration::from_secs(60),
        }
    }

    /// Clamp each snapshot, carrying a replacement floor only across consecutive low-disk probes.
    fn limit(&mut self, held: i64, cap: Option<i64>) -> Option<i64> {
        let cap = cap?;
        if self
            .sampled
            .is_none_or(|(at, _, _)| at.elapsed() >= self.interval)
        {
            let mut below_reserve = false;
            let limit =
                self.path
                    .parent()
                    .and_then(|dir| (self.probe)(dir))
                    .map(|(free, total)| {
                        let (main, wal) = crate::db::maint::db_sizes(&self.path);
                        let held = held.max(0) as u64;
                        below_reserve = total != 0 && free <= crate::disk_reserve::reserve(total);
                        // Evicting a larger victim must not lower the next reserve-bound sample.
                        let held = if below_reserve {
                            self.sampled
                                .filter(|(_, _, below)| *below)
                                .and_then(|(_, limit, _)| limit)
                                .unwrap_or(0)
                                .max(held)
                        } else {
                            held
                        };
                        crate::disk_reserve::tape_ceiling(
                            held,
                            main.saturating_add(wal),
                            free,
                            total,
                            cap.max(0) as u64,
                        )
                    });
            self.sampled = Some((Instant::now(), limit, below_reserve));
        }
        let limit = self.sampled.and_then(|(_, limit, _)| limit);
        Some(limit.map_or(cap, |limit| i64::try_from(limit).unwrap_or(i64::MAX)))
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

/// One stored row as the cache thread read it, its blob not yet decoded: decoding runs on the
/// reader's thread, so the one connection's thread only runs SQL.
struct RawSpan {
    table: Table,
    rowid: i64,
    from_ms: i64,
    to_ms: i64,
    blob: Vec<u8>,
    source: i64,
}

/// A row that did not decode, as reported back to the cache thread to be deleted.
struct BrokenRow {
    table: Table,
    rowid: i64,
    exchange: String,
    market: String,
    from_ms: i64,
    bytes: i64,
}

/// Queued archive operations; a failed read stays distinct from a successful empty result.
enum Op {
    Insert {
        exchange: String,
        market: String,
        from_ms: i64,
        to_ms: i64,
        ticks: Vec<Tick>,
        source: TileSource,
    },
    /// Consecutive inserts drained from one worker wake and committed together.
    InsertBatch(Vec<Op>),
    Read {
        exchange: String,
        market: String,
        from_ms: i64,
        to_ms: i64,
        reply: mpsc::Sender<rusqlite::Result<Vec<RawSpan>>>,
    },
    /// A row a reader could not decode: deleted if it is still the row that was read, and only
    /// then taken off the byte count — two readers may report the same row.
    DropBroken { row: BrokenRow },
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
    /// Serialize daily full compaction with live inserts and retention.
    CompactFull {
        reply: mpsc::Sender<Result<(), CompactFail>>,
    },
    /// Commit a trim and report compaction separately from deletion.
    TrimStart {
        keep: Arc<KeepMap>,
        compact: Compact,
        progress: Option<Arc<TrimProgress>>,
        reply: mpsc::Sender<rusqlite::Result<TrimOutcome>>,
    },
    /// Nothing: answered once every op queued before it has been served.
    Sync {
        reply: mpsc::Sender<()>,
        /// Release an optional caller-owned reservation after all preceding writes finish.
        reservation: Option<Box<dyn Send>>,
    },
    /// Drop the spans written longest ago until this many packed bytes are gone, and give the
    /// pages back to the disk ([`evict`]).
    Evict { bytes: i64 },
}

/// Cheaply cloneable handle to the one worker.
#[derive(Clone)]
pub struct TradeCache {
    tx: mpsc::SyncSender<Op>,
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
        Self::start(
            path,
            max_bytes,
            false,
            Some(crate::disk_reserve::disk_space),
        )
    }

    /// [`Self::open`] for a file with a custom byte ceiling: `ceiling` is asked at open and after every insert, and
    /// `None` keeps everything.
    pub fn open_with_ceiling(path: PathBuf, ceiling: fn() -> Option<i64>) -> Option<Self> {
        Self::start(path, ceiling, false, None)
    }

    /// [`Self::open`] for a file without a byte ceiling that gives room back to the disk on
    /// request ([`Self::evict_oldest`]): the station's tape, kept while the server has space.
    pub fn open_evictable(path: PathBuf) -> Option<Self> {
        Self::start(path, || None, true, None)
    }

    /// The worker behind every constructor; `shrinkable` keeps the file in the mode an eviction
    /// can shrink it in ([`evict::make_shrinkable`]); `disk` enables the terminal's reserve clamp.
    fn start(
        path: PathBuf,
        ceiling: fn() -> Option<i64>,
        shrinkable: bool,
        disk: Option<DiskProbe>,
    ) -> Option<Self> {
        let (tx, rx) = mpsc::sync_channel::<Op>(OP_QUEUE);
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
                if shrinkable {
                    if let Err(e) = evict::make_shrinkable(&conn) {
                        log::warn!("trade cache {} cannot shrink: {e}", path.display());
                    }
                }
                let mut disk = disk.map(|probe| DiskCeiling::new(path.clone(), probe));
                let count = disk.as_ref().and_then(|_| match held_bytes(&conn) {
                    Ok(held) => Some(held),
                    Err(e) => {
                        log::warn!("trade cache disk count failed {}: {e}", path.display());
                        None
                    }
                });
                let limit = resolved_ceiling(&mut disk, count, ceiling);
                let held = match prune(&conn, limit) {
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
                run(conn, rx, held, ceiling, disk);
            })
            .ok()?;
        Some(Self { tx })
    }

    /// Queue one answered span. A background producer waits for room rather than losing tape;
    /// the worker files only stretches not yet held, so a re-fetch never doubles a print.
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
    /// `None` means the read failed (SQL error, worker gone or busy past [`READ_TIMEOUT`]), which
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
            .try_send(Op::Read {
                exchange: exchange.to_string(),
                market: market.to_string(),
                from_ms,
                to_ms,
                reply,
            })
            .ok()?;
        let raw = rx.recv_timeout(READ_TIMEOUT).ok()?.ok()?;
        let (spans, broken) = decode_raw(raw, exchange, market);
        for row in broken {
            let _ = self.tx.send(Op::DropBroken { row });
        }
        Some(spans)
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
        self.held_spans_start(exchange, market, from_ms, to_ms)
            .recv_timeout(READ_TIMEOUT)
            .ok()?
            .ok()
    }

    /// Queue a coverage read; a background recorder waits for room instead of losing the answer.
    pub fn held_spans_start(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> mpsc::Receiver<rusqlite::Result<Vec<(i64, i64)>>> {
        let (reply, rx) = mpsc::channel();
        let _ = self.tx.send(Op::Spans {
            exchange: exchange.to_string(),
            market: market.to_string(),
            from_ms,
            to_ms,
            reply,
        });
        rx
    }

    /// Drop the spans written longest ago until `bytes` packed bytes are gone, and give the pages
    /// back to the disk — for a file opened with [`Self::open_evictable`]; another file keeps its
    /// length. A background caller waits for queue room; the worker logs what it freed.
    pub fn evict_oldest(&self, bytes: i64) {
        if bytes > 0 {
            let _ = self.tx.send(Op::Evict { bytes });
        }
    }

    /// The file's markets and time range — see [`Inventory`].
    ///
    /// Waits for the worker up to [`MAINTENANCE_TIMEOUT`]; `None` means the worker is gone or
    /// did not answer in time, `Some(Err)` that the read itself failed.
    pub fn inventory(&self) -> Option<rusqlite::Result<Inventory>> {
        self.inventory_start()
            .recv_timeout(MAINTENANCE_TIMEOUT)
            .ok()
    }

    /// Queue inventory without blocking the recorder on a maintenance operation.
    pub fn inventory_start(&self) -> mpsc::Receiver<rusqlite::Result<Inventory>> {
        let (reply, rx) = mpsc::channel();
        let _ = self.tx.send(Op::Inventory { reply });
        rx
    }

    /// Queue an atomic delete and compaction; disconnection means the worker is gone.
    pub fn trim_start(
        &self,
        keep: Arc<KeepMap>,
        compact: Compact,
        progress: Option<Arc<TrimProgress>>,
    ) -> mpsc::Receiver<rusqlite::Result<TrimOutcome>> {
        let (reply, rx) = mpsc::channel();
        let _ = self.tx.send(Op::TrimStart {
            keep,
            compact,
            progress,
            reply,
        });
        rx
    }

    /// Compact on the writer's connection, preserving every queued live write.
    /// A timeout leaves the queued operation running; disconnection means the worker is gone.
    pub fn compact_full(&self) -> Result<Result<(), CompactFail>, mpsc::RecvTimeoutError> {
        self.compact_full_with_timeout(MAINTENANCE_TIMEOUT)
    }

    /// Wait for queued compaction with an explicit deadline, including short test deadlines.
    fn compact_full_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Result<(), CompactFail>, mpsc::RecvTimeoutError> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .try_send(Op::CompactFull { reply })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => mpsc::RecvTimeoutError::Timeout,
                mpsc::TrySendError::Disconnected(_) => mpsc::RecvTimeoutError::Disconnected,
            })?;
        rx.recv_timeout(timeout)
    }

    /// Clip spans atomically and compact the committed cut, waiting for the worker.
    pub fn trim(&self, keep: Arc<KeepMap>) -> Option<rusqlite::Result<TrimReport>> {
        self.trim_start(keep, Compact::Full, None)
            .recv_timeout(MAINTENANCE_TIMEOUT)
            .ok()
            .map(|r| {
                r.map(|outcome| {
                    if let Err(failure) = outcome.compacted {
                        log::warn!("trade cache compaction after trim failed: {failure:?}");
                    }
                    outcome.report
                })
            })
    }

    /// Wait until every write queued before this call is on disk, at most `timeout` — for a
    /// process about to exit.
    ///
    /// Returns:
    ///     Whether the worker got there in time.
    pub fn sync(&self, timeout: Duration) -> bool {
        let (reply, rx) = mpsc::channel();
        let deadline = Instant::now() + timeout;
        let mut op = Op::Sync {
            reply,
            reservation: None,
        };
        loop {
            match self.tx.try_send(op) {
                Ok(()) => {
                    return rx
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .is_ok();
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
                Err(mpsc::TrySendError::Full(returned)) => {
                    op = returned;
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return false;
                    }
                    std::thread::park_timeout(remaining.min(Duration::from_millis(5)));
                }
            }
        }
    }

    /// Keep a reservation alive through the existing queue barrier, including writer failures.
    pub(crate) fn release_after_writes(&self, reservation: impl Send + 'static) {
        let (reply, _) = mpsc::channel();
        let _ = self.tx.send(Op::Sync {
            reply,
            reservation: Some(Box::new(reservation)),
        });
    }
}

/// The one process-wide cache, started on first use. `None` only
/// when its thread could not be spawned; a file that fails to open leaves the handle in place
/// with a dead channel behind it (see [`TradeCache::open`]), and the process does not retry —
/// it would only repeat the warning on every tick stage.
static CACHE: OnceLock<Option<TradeCache>> = OnceLock::new();
/// The cache shared by replay and maintenance; starts its worker on first use.
pub fn handle() -> Option<TradeCache> {
    CACHE
        .get_or_init(|| TradeCache::open(crate::config::paths::trades_db_path()))
        .clone()
}

/// Open both archive layouts and add range-read indexes without changing the archive version.
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
    // A first open of a large file builds the range indexes; say how long that took, once.
    let indexes_held: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name IN
         ('spans_updated', 'spans_market_to', 'packs_updated', 'packs_market_to')",
        [],
        |r| r.get(0),
    )?;
    let started = std::time::Instant::now();
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
        CREATE INDEX IF NOT EXISTS spans_market_to ON spans(exchange, market, to_ms);
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
        CREATE INDEX IF NOT EXISTS packs_updated ON packs(updated_ms);
        CREATE INDEX IF NOT EXISTS packs_market_to ON packs(exchange, market, to_ms);",
    )?;
    if indexes_held < 4 {
        log::info!(
            "trade cache: range indexes built in {} ms",
            started.elapsed().as_millis()
        );
    }
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
/// Victim deletes commit in bounded chunks; eviction supplies its own transaction separately.
///
/// Returns:
///     The packed bytes the file holds afterwards, for the worker to carry forward.
fn prune(conn: &rusqlite::Connection, ceiling: Option<i64>) -> rusqlite::Result<i64> {
    let held = held_bytes(conn)?;
    trim_in_chunks(conn, held, ceiling)
}

/// Bounds WAL growth and reader snapshot age per standalone trim transaction.
const TRIM_CHUNK: usize = 64;

/// Trims standalone callers in committed chunks, preserving oldest-first order across commits.
///
/// Args:
///     conn: A connection outside a transaction.
///     held: Packed bytes currently held.
///     ceiling: Packed bytes allowed, or `None` to keep everything.
/// Returns:
///     Held bytes after meeting the ceiling or exhausting progress.
/// Errors:
///     A SQLite failure rolls back the current chunk; earlier committed chunks remain removed.
fn trim_in_chunks(
    conn: &rusqlite::Connection,
    mut held: i64,
    ceiling: Option<i64>,
) -> rusqlite::Result<i64> {
    let Some(limit) = ceiling else {
        return Ok(held);
    };
    while held > limit {
        let tx = conn.unchecked_transaction()?;
        let after = trim_to_ceiling(&tx, held, ceiling, Some(TRIM_CHUNK))?;
        tx.commit()?;
        if after == held {
            break;
        }
        held = after;
    }
    Ok(held)
}

/// Drop the spans written longest ago until `held` fits under `ceiling` or the victim limit is met.
///
/// "Written", not "used": `updated_ms` is set by [`insert_span`] alone, a read leaves it as it
/// was, so the order is the order the prints were filed in — the oldest trades' prints go first.
///
/// The worker supplies the cap clamped by its cached probe-time disk budget; custom callers
/// supply their own packed limit without probing the filesystem.
/// This function never opens a transaction: standalone callers and eviction each own theirs.
/// Cached statements are dropped before returning so their caller can commit.
///
/// Args:
///     conn: The open connection.
///     held: Packed bytes the file holds now.
///     ceiling: Packed bytes allowed, or `None` for no ceiling.
///     max_victims: Maximum rows deleted by this call, or `None` for no chunk limit.
///
/// Returns:
///     Packed bytes held afterwards.
/// Errors:
///     A SQLite query or delete failure; rollback belongs to the caller's transaction.
fn trim_to_ceiling(
    conn: &rusqlite::Connection,
    mut held: i64,
    ceiling: Option<i64>,
    max_victims: Option<usize>,
) -> rusqlite::Result<i64> {
    let Some(ceiling) = ceiling else {
        return Ok(held);
    };
    let queries = Table::ALL.map(|table| (table.oldest_sql(), table.delete_sql()));
    let mut victims = 0;
    while held > ceiling && max_victims.is_none_or(|limit| victims < limit) {
        // One span at a time, oldest first: the excess is usually one wide harvest, and a
        // batch would take a fresh neighbour down with it. Each table answers its own oldest
        // off its `updated_ms` index; the older of the two goes.
        let mut oldest: Option<(&str, i64, i64, i64)> = None;
        for (select, delete) in &queries {
            let row: Option<(i64, i64, i64)> = conn
                .prepare_cached(select)?
                .query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?;
            let Some((rowid, bytes, updated_ms)) = row else {
                continue;
            };
            if oldest.is_none_or(|(.., held_updated)| updated_ms < held_updated) {
                oldest = Some((delete, rowid, bytes, updated_ms));
            }
        }
        let Some((delete, rowid, bytes, _)) = oldest else {
            break;
        };
        conn.prepare_cached(delete)?.execute([rowid])?;
        held -= bytes;
        victims += 1;
    }
    Ok(held)
}

/// A committed trim whose bounded compaction steps still owe the caller a reply.
struct PendingCompact {
    report: TrimReport,
    before: u64,
    reply: mpsc::Sender<rusqlite::Result<TrimOutcome>>,
}

impl PendingCompact {
    /// Reply as soon as a bounded step finishes, retaining ownership only while pages remain.
    fn step(self, conn: &rusqlite::Connection) -> Option<Self> {
        match trim::compact_step(conn) {
            Ok(true) => Some(self),
            result => {
                let _ = self.reply.send(Ok(TrimOutcome {
                    report: self.report,
                    compacted: result.map(|_| ()),
                    bytes_before: self.before,
                    bytes_after: trim::disk_bytes(conn),
                }));
                None
            }
        }
    }
}

/// Resolve the packed cap once for opening and both queue-serving paths.
fn resolved_ceiling(
    disk: &mut Option<DiskCeiling>,
    held: Option<i64>,
    ceiling: fn() -> Option<i64>,
) -> Option<i64> {
    match (disk.as_mut(), held) {
        (Some(disk), Some(held)) => disk.limit(held, ceiling()),
        _ => ceiling(),
    }
}

/// Ordinary operations finish immediately; incremental compaction resumes between queue items.
struct Served(Option<PendingCompact>);

/// Serve queued operations between bounded repacking and incremental-compaction batches.
fn run(
    conn: rusqlite::Connection,
    rx: mpsc::Receiver<Op>,
    mut held: i64,
    ceiling: fn() -> Option<i64>,
    mut disk: Option<DiskCeiling>,
) {
    let mut compacting: Option<PendingCompact> = None;
    let mut deferred = std::collections::VecDeque::new();
    let mut repack = match legacy_rows_left(&conn) {
        Ok(true) => Some(RepackTally::start()),
        Ok(false) => None,
        Err(e) => {
            log::warn!("trade cache: legacy table unreadable, not repacked: {e}");
            None
        }
    };
    loop {
        if let Some(pending) = compacting.take() {
            // One queued live op interleaves with every bounded vacuum step.
            match rx.try_recv() {
                Ok(op @ (Op::TrimStart { .. } | Op::CompactFull { .. })) => {
                    deferred.push_back(op);
                }
                Ok(op) => {
                    let snapshot = held;
                    serve(&conn, op, &mut held, || {
                        resolved_ceiling(&mut disk, Some(snapshot), ceiling)
                    });
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            compacting = pending.step(&conn);
            continue;
        }
        let op = if let Some(op) = deferred.pop_front() {
            op
        } else if let Some(tally) = repack.as_mut() {
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
        let op = batch_inserts(op, &rx, &mut deferred);
        let snapshot = held;
        compacting = serve(&conn, op, &mut held, || {
            resolved_ceiling(&mut disk, Some(snapshot), ceiling)
        })
        .0;
    }
}

/// Largest insert drain committed by one worker wake; a read never waits behind an unbounded batch.
const INSERT_BATCH: usize = 256;

/// Collect only consecutive inserts, leaving the first other operation ahead of later queue items.
fn batch_inserts(
    first: Op,
    rx: &mpsc::Receiver<Op>,
    deferred: &mut std::collections::VecDeque<Op>,
) -> Op {
    if !matches!(first, Op::Insert { .. }) {
        return first;
    }
    let mut ops = vec![first];
    while ops.len() < INSERT_BATCH {
        match rx.try_recv() {
            Ok(op @ Op::Insert { .. }) => ops.push(op),
            Ok(op) => {
                deferred.push_back(op);
                break;
            }
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
        }
    }
    if ops.len() == 1 {
        ops.pop().expect("one insert")
    } else {
        Op::InsertBatch(ops)
    }
}

/// Serve an op with its cap resolver, committing post-insert ceiling deletes in bounded chunks.
fn serve(
    conn: &rusqlite::Connection,
    op: Op,
    held: &mut i64,
    mut ceiling: impl FnMut() -> Option<i64>,
) -> Served {
    match op {
        op @ Op::Insert { .. } => {
            let limit = ceiling();
            serve_insert(conn, op, held, limit);
        }
        Op::InsertBatch(ops) => {
            let now = crate::util::time::now_unix_ms_i64();
            let res = conn.unchecked_transaction().and_then(|tx| {
                let mut batch_held = *held;
                let mut written = Vec::with_capacity(ops.len());
                for op in &ops {
                    let Op::Insert {
                        exchange,
                        market,
                        from_ms,
                        to_ms,
                        ticks,
                        source,
                    } = op
                    else {
                        unreachable!("insert batch contains only inserts")
                    };
                    let limit = ceiling();
                    let wrote =
                        insert_span(&tx, exchange, market, *from_ms, *to_ms, ticks, *source, now)?;
                    written.push(wrote);
                    batch_held += wrote;
                    // Per-insert eviction order matches independent commits; a later overlap must
                    // see the same gaps instead of reviving a span that batch-end trimming removed.
                    loop {
                        let after = trim_to_ceiling(&tx, batch_held, limit, Some(TRIM_CHUNK))?;
                        if after == batch_held {
                            break;
                        }
                        batch_held = after;
                    }
                }
                tx.commit()?;
                Ok((written, batch_held))
            });
            match res {
                Ok((written, batch_held)) => {
                    *held = batch_held;
                    for (op, wrote) in ops.iter().zip(written) {
                        let Op::Insert {
                            exchange, market, ..
                        } = op
                        else {
                            unreachable!()
                        };
                        if wrote > 0 {
                            filed::note(exchange, market);
                        }
                    }
                }
                Err(_) => {
                    // The batch rolled back: isolate any bad row instead of losing its valid neighbours.
                    for op in ops {
                        let limit = ceiling();
                        serve_insert(conn, op, held, limit);
                    }
                }
            }
        }
        Op::Read {
            exchange,
            market,
            from_ms,
            to_ms,
            reply,
        } => {
            let spans = read_raw(conn, &exchange, &market, from_ms, to_ms);
            if let Err(e) = &spans {
                log::warn!("trade cache read failed {exchange}/{market}: {e}");
            }
            let _ = reply.send(spans);
        }
        Op::DropBroken { row } => match drop_broken(conn, &row) {
            // A row that did not decode left the file: the ceiling must not keep counting it.
            Ok(true) => *held -= row.bytes,
            Ok(false) => {}
            Err(e) => log::warn!(
                "trade cache: dropping a broken span of {}/{} failed: {e}",
                row.exchange,
                row.market
            ),
        },
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
        Op::TrimStart {
            keep,
            compact,
            progress,
            reply,
        } => {
            let before = trim::disk_bytes(conn);
            let walked = Instant::now();
            if matches!(compact, Compact::Incremental) {
                match trim::trim_progress(conn, &keep, progress.as_deref()) {
                    Ok(report) => {
                        *held = held_bytes(conn).unwrap_or(*held);
                        log_trimmed(&report, walked);
                        return Served(
                            PendingCompact {
                                report,
                                before,
                                reply,
                            }
                            .step(conn),
                        );
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
                return Served(None);
            }
            let result = trim::trim_progress(conn, &keep, progress.as_deref()).map(|report| {
                *held = held_bytes(conn).unwrap_or(*held);
                log_trimmed(&report, walked);
                let compacted = if report.is_empty() {
                    Ok(())
                } else {
                    trim::compact(conn, compact)
                };
                TrimOutcome {
                    report,
                    compacted,
                    bytes_before: before,
                    bytes_after: trim::disk_bytes(conn),
                }
            });
            let _ = reply.send(result);
        }
        Op::Sync { reply, reservation } => {
            drop(reservation);
            let _ = reply.send(());
        }
        Op::Evict { bytes } => match evict::evict_oldest(conn, *held, bytes) {
            Ok(evict::Evicted::Freed {
                held: after,
                shrunk,
            }) => {
                log::info!(
                    "trade cache: {} bytes of the oldest prints evicted for disk space, {after} held{}",
                    *held - after,
                    if shrunk {
                        ""
                    } else {
                        "; a reader kept the file from shrinking yet"
                    }
                );
                *held = after;
            }
            Ok(evict::Evicted::NotShrinkable) => log::warn!(
                "trade cache: asked for disk space, but the file cannot give pages back (its \
                 one VACUUM failed): nothing evicted"
            ),
            Err(e) => {
                log::warn!("trade cache eviction failed: {e}");
                *held = held_bytes(conn).unwrap_or(*held);
            }
        },
        Op::CompactFull { reply } => {
            let _ = reply.send(trim::compact(conn, Compact::Full));
        }
    }
    Served(None)
}

/// Commit one insert and enforce its ceiling without recursively instantiating the operation server.
fn serve_insert(conn: &rusqlite::Connection, op: Op, held: &mut i64, limit: Option<i64>) {
    let Op::Insert {
        exchange,
        market,
        from_ms,
        to_ms,
        ticks,
        source,
    } = op
    else {
        unreachable!("single-insert helper received another operation")
    };
    let now = crate::util::time::now_unix_ms_i64();
    let res = conn.unchecked_transaction().and_then(|tx| {
        let wrote = insert_span(&tx, &exchange, &market, from_ms, to_ms, &ticks, source, now)?;
        tx.commit()?;
        Ok(wrote)
    });
    match res {
        Ok(wrote) => {
            // A reader that judged this market's rows unservable asks again once new tape lands.
            if wrote > 0 {
                filed::note(&exchange, &market);
            }
            *held += wrote;
            match trim_in_chunks(conn, *held, limit) {
                Ok(now_held) => *held = now_held,
                Err(e) => {
                    // The failed chunk rolled back; earlier committed chunks require a true count.
                    log::warn!("trade cache ceiling failed: {e}");
                    *held = held_bytes(conn).unwrap_or(*held);
                }
            }
        }
        Err(e) => log::warn!("trade cache insert failed {exchange}/{market}: {e}"),
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
/// Finite sorted stamps have monotone integer casts, so each gap selects a slice by two searches.
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
        let lo = sorted.partition_point(|t| (t.time_ms as i64) < gap_from);
        let hi = sorted.partition_point(|t| (t.time_ms as i64) <= gap_to);
        let inside = &sorted[lo..hi];
        let blob = codec::encode(inside);
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

/// The one line a committed retention walk leaves: what it removed, out of how many rows and
/// packed bytes the file held, how long the walk took and what the process held when it ended
/// (#1081 — the removed count alone said nothing about how large a file the walk had read).
fn log_trimmed(report: &TrimReport, walked: Instant) {
    log::info!(
        "trade cache trimmed: {} of {} span(s), {} of {} packed bytes, walk {:.1} s, {}",
        report.spans_dropped + report.spans_cut,
        report.spans_total,
        report.bytes_dropped,
        report.bytes_total,
        walked.elapsed().as_secs_f64(),
        crate::metrics::memory_now()
    );
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
    read_spans_dropping(conn, exchange, market, from_ms, to_ms, Some(&mut 0))
}

/// [`read_spans`], adding the blob bytes of every row it deleted to `dropped` — the worker
/// carries the file's byte count and must take them off it; `None` leaves a broken row where it
/// is and only skips it. The worker itself decodes on the reader's thread ([`TradeCache::read`]).
#[cfg(test)]
fn read_spans_dropping(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
    dropped: Option<&mut i64>,
) -> rusqlite::Result<Vec<StoredSpan>> {
    let (spans, broken) = decode_raw(
        read_raw(conn, exchange, market, from_ms, to_ms)?,
        exchange,
        market,
    );
    if let Some(dropped) = dropped {
        for row in broken {
            if drop_broken(conn, &row)? {
                *dropped += row.bytes;
            }
        }
    }
    Ok(spans)
}

/// Every row of the key intersecting `[from_ms, to_ms]`, from both tables, blobs undecoded.
fn read_raw(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<RawSpan>> {
    let mut out = Vec::new();
    for table in Table::ALL {
        let mut stmt = conn.prepare_cached(&table.read_sql())?;
        let mut rows = stmt.query(rusqlite::params![exchange, market, from_ms, to_ms])?;
        while let Some(r) = rows.next()? {
            out.push(RawSpan {
                table,
                rowid: r.get(0)?,
                from_ms: r.get(1)?,
                to_ms: r.get(2)?,
                blob: r.get_ref(3)?.as_blob()?.to_vec(),
                source: r.get(4)?,
            });
        }
    }
    Ok(out)
}

/// Decode raw rows into spans ascending by `from_ms`, and name every row that did not decode.
fn decode_raw(
    raw: Vec<RawSpan>,
    exchange: &str,
    market: &str,
) -> (Vec<StoredSpan>, Vec<BrokenRow>) {
    let mut out = Vec::with_capacity(raw.len());
    let mut broken = Vec::new();
    for row in raw {
        match row.table.decode(&row.blob) {
            Ok(ticks) => out.push(StoredSpan {
                from_ms: row.from_ms,
                to_ms: row.to_ms,
                ticks,
                source: TileSource::from_code(row.source),
            }),
            Err(e) => {
                log::warn!(
                    "trade cache: a {} span of {exchange}/{market} does not decode ({e}); dropped to be fetched again",
                    row.table.name()
                );
                broken.push(BrokenRow {
                    table: row.table,
                    rowid: row.rowid,
                    exchange: exchange.to_string(),
                    market: market.to_string(),
                    from_ms: row.from_ms,
                    bytes: row.blob.len() as i64,
                });
            }
        }
    }
    out.sort_by_key(|s| s.from_ms);
    (out, broken)
}

/// Delete a broken row if it is still the row that was read (its key and blob length match);
/// `true` exactly when this call removed it.
fn drop_broken(conn: &rusqlite::Connection, row: &BrokenRow) -> rusqlite::Result<bool> {
    let removed = conn.execute(
        &format!(
            "DELETE FROM {} WHERE rowid = ?1 AND exchange = ?2 AND market = ?3 AND from_ms = ?4
             AND LENGTH(ticks) = ?5",
            row.table.name()
        ),
        rusqlite::params![row.rowid, row.exchange, row.market, row.from_ms, row.bytes],
    )?;
    Ok(removed == 1)
}

/// The bounds read of [`read_span_bounds`], shared with its query-plan regression test.
const READ_SPAN_BOUNDS_SQL: &str = "SELECT from_ms, to_ms FROM spans
     WHERE exchange = ?1 AND market = ?2 AND to_ms >= ?3 AND from_ms <= ?4
     UNION ALL
     SELECT from_ms, to_ms FROM packs
     WHERE exchange = ?1 AND market = ?2 AND to_ms >= ?3 AND from_ms <= ?4
     ORDER BY 1";

/// The bounds of every span of a market intersecting `[from_ms, to_ms]`, from both tables,
/// ascending — the columns alone, never the blob.
fn read_span_bounds(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare_cached(READ_SPAN_BOUNDS_SQL)?;
    let rows = stmt.query_map(rusqlite::params![exchange, market, from_ms, to_ms], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests;
