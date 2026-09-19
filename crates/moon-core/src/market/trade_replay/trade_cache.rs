//! Persisted trade prints for replays: `trades.sqlite`, the disk under [`super::tick_tiles`].
//!
//! # What it is
//!
//! The tile store answers a neighbouring window inside one session; this file answers the same
//! question across a restart. Same shape — spans of `(from_ms, to_ms)` per exchange and market,
//! disjoint, holding the venue's RAW prints — persisted as one row per span, the prints packed
//! into a blob. The worker hydrates the tile store from here before it decides what to fetch, and
//! writes every harvest through after it filed it, so the two never disagree about what was
//! fetched: the disk is the tile store's memory, not a second cache with its own rules.
//!
//! # Why a separate file
//!
//! Not a table in `klines.sqlite`: prints are a different volume (a busy perpetual is a megabyte
//! per ten minutes where its bars are a kilobyte), keep their own retention, and are the one
//! thing the reader may want to switch off or delete without touching the bars. The Storage tab
//! shows this file on its own line with its own switch.
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
//! file and nothing is written to it, and the tile store lives in memory alone; a file this
//! session already opened while the switch was on stays open, idle, until the process exits —
//! the handle is a `OnceLock`, and closing it under a queued write is not worth a second state.
//! The live value is an atomic the Storage tab flips without a restart, initialised from the
//! file on first use.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use rusqlite::OptionalExtension;

use crate::feed::types::{Side, Tick};

/// Bytes per packed print: `i64 time_ms`, `f32 price`, `f32 qty`, `u32 side` (`0` buy, `1`
/// sell). The stamp is absolute, not an offset from the span's edge: a span is the position's
/// own focus, and a position can be held for months — an offset narrow enough to save four
/// bytes would wrap on exactly those.
const ROW_BYTES: usize = 20;

/// How long a read waits for the worker before answering `None`.
const READ_TIMEOUT: Duration = Duration::from_millis(250);

/// Spans untouched for longer than this are dropped at open.
///
/// A replay is opened from a report the reader is walking now, not a history browsed months
/// later; two weeks holds a week of trades opened twice. The bars keep 30 days at one minute.
const RETENTION_DAYS: i64 = 14;

/// Ceiling on the packed bytes the file may hold — checked at open, after the retention pass,
/// and again after every insert; past it the oldest spans go first. A day of busy replays is
/// tens of megabytes, so this is months of them.
const MAX_BYTES: i64 = 256 * 1024 * 1024;

const DAY_MS: i64 = 86_400_000;

/// The row format this build writes and reads. A file whose `user_version` differs is a cache
/// in a layout this build does not know — a print row of another width would unpack into
/// garbage stamps that `tick_tiles::insert` silently drops, filing the span as quiet — so the
/// table is dropped and started over: it is a cache, and refetching is the honest price.
const SCHEMA_VERSION: i32 = 2;

/// One persisted span, as read back.
#[derive(Clone, Debug)]
pub struct StoredSpan {
    pub from_ms: i64,
    pub to_ms: i64,
    /// Ascending by time, all inside `[from_ms, to_ms]`.
    pub ticks: Vec<Tick>,
}

enum Op {
    Insert {
        exchange: String,
        market: String,
        from_ms: i64,
        to_ms: i64,
        ticks: Vec<Tick>,
    },
    Read {
        exchange: String,
        market: String,
        from_ms: i64,
        to_ms: i64,
        reply: mpsc::Sender<Vec<StoredSpan>>,
    },
}

/// Cheaply cloneable handle to the one worker.
#[derive(Clone)]
pub struct TradeCache {
    tx: mpsc::Sender<Op>,
}

impl TradeCache {
    /// Start the worker; it opens the file, runs the schema and the retention pass, then serves
    /// the queue. The handle comes back at once.
    ///
    /// Everything that touches the disk happens on the cache's OWN thread, never on the
    /// caller's: `handle()` is called from the trade-replay worker, whose queue holds every
    /// other window's candle and tick job, and a retention pass over a file this size is a
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
                let held = match prune(&conn, crate::util::time::now_unix_ms_i64()) {
                    Ok(held) => held,
                    Err(e) => {
                        // The ceiling still needs a true count to work from: a zero here would
                        // hold the file's existing bytes exempt for the whole session.
                        log::warn!("trade cache retention failed {}: {e}", path.display());
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
    pub fn insert(&self, exchange: &str, market: &str, from_ms: i64, to_ms: i64, ticks: Vec<Tick>) {
        if from_ms > to_ms {
            return;
        }
        let _ = self.tx.send(Op::Insert {
            exchange: exchange.to_string(),
            market: market.to_string(),
            from_ms,
            to_ms,
            ticks,
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
}

/// The one process-wide cache, started on the first use that finds the switch on. `None` only
/// when its thread could not be spawned; a file that fails to open leaves the handle in place
/// with a dead channel behind it (see [`TradeCache::open`]), and the process does not retry —
/// it would only repeat the warning on every tick stage.
static CACHE: OnceLock<Option<TradeCache>> = OnceLock::new();
/// Live value of `[trade_replay] persist_trades`; the Storage tab flips it.
static ENABLED: AtomicBool = AtomicBool::new(true);
static ENABLED_INIT: OnceLock<()> = OnceLock::new();

fn ensure_enabled_loaded() {
    ENABLED_INIT.get_or_init(|| {
        let cfg = crate::config::storage::load();
        ENABLED.store(cfg.trade_replay.persist_trades, Ordering::Relaxed);
    });
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
    CACHE
        .get_or_init(|| TradeCache::open(crate::config::paths::trades_db_path()))
        .clone()
}

fn init_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        if version != 0 {
            log::info!("trade cache: layout {version} → {SCHEMA_VERSION}, spans dropped");
        }
        conn.execute_batch("DROP TABLE IF EXISTS spans;")?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS spans(
            exchange TEXT NOT NULL,
            market TEXT NOT NULL,
            from_ms INTEGER NOT NULL,
            to_ms INTEGER NOT NULL,
            ticks BLOB NOT NULL,
            updated_ms INTEGER NOT NULL,
            PRIMARY KEY(exchange, market, from_ms)
        );
        CREATE INDEX IF NOT EXISTS spans_updated ON spans(updated_ms);",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// Packed bytes the file holds, re-counted from the table.
fn held_bytes(conn: &rusqlite::Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(LENGTH(ticks)), 0) FROM spans",
        [],
        |r| r.get(0),
    )
}

/// Drop what retention no longer keeps, then the oldest spans past the byte ceiling.
///
/// Returns:
///     The packed bytes the file holds afterwards, for the worker to carry forward.
fn prune(conn: &rusqlite::Connection, now_ms: i64) -> rusqlite::Result<i64> {
    conn.execute(
        "DELETE FROM spans WHERE updated_ms < ?1",
        [now_ms - RETENTION_DAYS * DAY_MS],
    )?;
    trim_to_ceiling(conn, held_bytes(conn)?)
}

/// Drop the oldest spans until `held` packed bytes fit under [`MAX_BYTES`].
///
/// Args:
///     conn: The open connection.
///     held: Packed bytes the file holds now.
///
/// Returns:
///     Packed bytes held afterwards.
fn trim_to_ceiling(conn: &rusqlite::Connection, mut held: i64) -> rusqlite::Result<i64> {
    while held > MAX_BYTES {
        // One span at a time, oldest first: the excess is usually one wide harvest, and a
        // batch would take a fresh neighbour down with it.
        let oldest: Option<(i64, i64)> = conn
            .query_row(
                "SELECT rowid, LENGTH(ticks) FROM spans ORDER BY updated_ms ASC, rowid ASC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((rowid, bytes)) = oldest else {
            break;
        };
        conn.execute("DELETE FROM spans WHERE rowid = ?1", [rowid])?;
        held -= bytes;
    }
    Ok(held)
}

/// The worker: serves the queue, carrying the file's packed byte count so the ceiling is held
/// after every insert rather than only at open.
fn run(conn: rusqlite::Connection, rx: mpsc::Receiver<Op>, mut held: i64) {
    while let Ok(op) = rx.recv() {
        match op {
            Op::Insert {
                exchange,
                market,
                from_ms,
                to_ms,
                ticks,
            } => {
                let now = crate::util::time::now_unix_ms_i64();
                let res = conn.unchecked_transaction().and_then(|tx| {
                    let wrote = insert_span(&tx, &exchange, &market, from_ms, to_ms, &ticks, now)?;
                    tx.commit()?;
                    Ok(wrote)
                });
                match res {
                    Ok(wrote) => {
                        held += wrote;
                        match trim_to_ceiling(&conn, held) {
                            Ok(now_held) => held = now_held,
                            Err(e) => {
                                // Rows may already be gone: re-count rather than carry an
                                // overcount that would evict fresh spans on the next insert.
                                log::warn!("trade cache ceiling failed: {e}");
                                held = held_bytes(&conn).unwrap_or(held);
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
                let spans = match read_spans(&conn, &exchange, &market, from_ms, to_ms) {
                    Ok(spans) => spans,
                    Err(e) => {
                        log::warn!("trade cache read failed {exchange}/{market}: {e}");
                        Vec::new()
                    }
                };
                let _ = reply.send(spans);
            }
        }
    }
}

/// File `[from_ms, to_ms]` as one row per stretch of it not already stored, each holding the
/// prints of `ticks` inside that stretch. A span already covered whole writes nothing.
///
/// Returns:
///     Packed bytes written.
fn insert_span(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
    ticks: &[Tick],
    now_ms: i64,
) -> rusqlite::Result<i64> {
    let mut written = 0i64;
    let held: Vec<(i64, i64)> = read_spans(conn, exchange, market, from_ms, to_ms)?
        .into_iter()
        .map(|s| (s.from_ms, s.to_ms))
        .collect();
    let mut sorted: Vec<Tick> = ticks
        .iter()
        .copied()
        .filter(|t| t.time_ms.is_finite())
        .collect();
    sorted.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    for (gap_from, gap_to) in super::tick_tiles::gaps_between(&held, from_ms, to_ms) {
        let inside = sorted.iter().filter(|t| {
            let time_ms = t.time_ms as i64;
            time_ms >= gap_from && time_ms <= gap_to
        });
        let blob = pack(inside);
        written += blob.len() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO spans(exchange, market, from_ms, to_ms, ticks, updated_ms)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![exchange, market, gap_from, gap_to, blob, now_ms],
        )?;
    }
    Ok(written)
}

/// Every span of the key intersecting `[from_ms, to_ms]`, ascending by `from_ms`.
fn read_spans(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<StoredSpan>> {
    let mut stmt = conn.prepare_cached(
        "SELECT from_ms, to_ms, ticks FROM spans
         WHERE exchange = ?1 AND market = ?2 AND to_ms >= ?3 AND from_ms <= ?4
         ORDER BY from_ms",
    )?;
    let rows = stmt.query_map(rusqlite::params![exchange, market, from_ms, to_ms], |r| {
        let span_from: i64 = r.get(0)?;
        let span_to: i64 = r.get(1)?;
        let blob: Vec<u8> = r.get(2)?;
        Ok(StoredSpan {
            from_ms: span_from,
            to_ms: span_to,
            ticks: unpack(&blob),
        })
    })?;
    rows.collect()
}

/// Pack prints as [`ROW_BYTES`] rows.
fn pack<'a>(ticks: impl Iterator<Item = &'a Tick>) -> Vec<u8> {
    let mut out = Vec::new();
    for t in ticks {
        out.extend_from_slice(&(t.time_ms as i64).to_le_bytes());
        out.extend_from_slice(&t.price.to_le_bytes());
        out.extend_from_slice(&t.qty.to_le_bytes());
        let side: u32 = match t.side {
            Side::Buy => 0,
            Side::Sell => 1,
        };
        out.extend_from_slice(&side.to_le_bytes());
    }
    out
}

/// Inverse of [`pack`]; a trailing partial row is ignored.
fn unpack(blob: &[u8]) -> Vec<Tick> {
    let mut out = Vec::with_capacity(blob.len() / ROW_BYTES);
    for chunk in blob.chunks_exact(ROW_BYTES) {
        let u = |i: usize| u32::from_le_bytes(chunk[i..i + 4].try_into().expect("four bytes"));
        let f = |i: usize| f32::from_le_bytes(chunk[i..i + 4].try_into().expect("four bytes"));
        let time_ms = i64::from_le_bytes(chunk[0..8].try_into().expect("eight bytes"));
        out.push(Tick {
            time_ms: time_ms as f64,
            price: f(8),
            qty: f(12),
            side: match u(16) {
                0 => Side::Buy,
                _ => Side::Sell,
            },
        });
    }
    out
}

#[cfg(test)]
mod tests;
