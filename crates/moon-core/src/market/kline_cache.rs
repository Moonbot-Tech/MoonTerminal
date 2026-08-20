//! Local kline (candle) cache stored in a separate `klines.sqlite` database BESIDE the
//! other data, NOT in `reports.sqlite`. It addresses two moonproto limitations: each core
//! holds ONE candle timeframe, and `GetCoinCardCandles` cannot load incrementally because
//! its coin+kind request always returns the full ring. Persisting fetched history locally
//! preserves the depth of larger timeframes across restarts and while the core's slot is
//! occupied by a smaller timeframe, without repeated full exchange downloads that consume
//! rate-limit weight.
//!
//! Schema: one `chunks` table stores a packed blob of daily rows keyed by exchange,
//! market, kind, and day. The exchange key is a stable `ExchangeId` (code + DEX hash),
//! NOT a CoreId, so cores on the same exchange share the cache. Deduplication follows
//! naturally from the PRIMARY KEY plus a merge by `t_open` within each chunk, where
//! incoming rows override stored ones. Each row uses 24 bytes (`u32 offset_ms + 5×f32`),
//! so one day of 1-minute data is about 34 KB per coin. Startup retention varies by kind;
//! see `retention_days` (30 days for 1 minute, 15 for 5 minutes, 10 years for larger kinds).
//!
//! Database open, schema creation, and startup retention run synchronously. After that
//! setup, queued reads and writes run on a dedicated worker because `Connection` is not
//! `Sync`. Writes are nonblocking; reads use a reply channel with a timeout because an
//! empty result is preferable to a stalled prepare.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use super::candles::ChartCandle;

const DAY_MS: i64 = 86_400_000;
const ROW_BYTES: usize = 24;
/// Read-response timeout that avoids hanging when the cache thread is busy or dead.
const READ_TIMEOUT: Duration = Duration::from_millis(250);

/// Returns retention in days for a candle kind.
///
/// Small timeframes are expensive while larger ones are cheap. The background recorder
/// writes 5-minute data for ALL markets, about 15 MB/day for roughly 5,500 markets, so it
/// retains 15 days to keep the database near 250 MB instead of multiple gigabytes
/// (measured 2026-07-15). Distant small-timeframe history is unnecessary because larger
/// layers fill the tail. One-minute data comes from deep-history replies for open charts
/// and is retained for 30 days.
fn retention_days(kind_min: u32) -> i64 {
    match kind_min {
        0..=1 => 30,
        2..=5 => 15,
        _ => 3650,
    }
}

/// Batch-write item containing rows for one exchange, market, and kind.
///
/// The recorder accumulates these public items and sends them through one `merge_batch`,
/// so an entire cycle spanning thousands of markets uses ONE transaction rather than
/// thousands of small ones. Adjacent chunks on a shared page are consequently written to
/// the WAL once instead of once per commit.
pub struct MergeItem {
    pub exchange: String,
    pub market: String,
    pub kind_min: u32,
    pub rows: Vec<ChartCandle>,
}

enum Op {
    Merge {
        exchange: String,
        market: String,
        kind_min: u32,
        rows: Vec<ChartCandle>,
    },
    MergeBatch {
        items: Vec<MergeItem>,
        /// Signalled after the transaction settles, so a producer can wait for one chunk before
        /// queueing the next. `None` leaves the write fully nonblocking.
        done: Option<mpsc::Sender<()>>,
    },
    Read {
        exchange: String,
        market: String,
        kind_min: u32,
        from_ms: i64,
        to_ms: i64,
        reply: mpsc::Sender<Vec<ChartCandle>>,
    },
}

/// Releases a `merge_batch_blocking` caller when the worker's arm ends, however it ends.
///
/// A plain send at the bottom of that arm would leave the producer parked forever on any early
/// return, which is the one failure this whole handshake must not introduce.
struct SettledOnDrop(mpsc::Sender<()>);

impl Drop for SettledOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// Cheaply cloneable cache handle that sends queued reads and writes to the database worker.
#[derive(Clone)]
pub struct KlineCache {
    tx: mpsc::Sender<Op>,
}

impl KlineCache {
    /// Opens the database and initializes its schema synchronously, then starts its worker.
    ///
    /// Returns `None` when opening or initialization fails because charts can operate
    /// without this optional cache.
    pub fn open(path: PathBuf) -> Option<Self> {
        let conn = match rusqlite::Connection::open(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("kline cache open failed {}: {e}", path.display());
                return None;
            }
        };
        if let Err(e) = init_schema(&conn) {
            log::warn!("kline cache schema failed {}: {e}", path.display());
            return None;
        }
        let (tx, rx) = mpsc::channel::<Op>();
        std::thread::Builder::new()
            .name("kline-cache".into())
            .spawn(move || run(conn, rx))
            .ok()?;
        log::info!("kline cache открыт: {}", path.display());
        Some(Self { tx })
    }

    /// Enqueues a nonblocking row merge.
    ///
    /// Incoming rows override stored rows so newer server OHLC wins. Empty input is not
    /// queued; the worker skips rows with a non-finite or non-positive timestamp, or a
    /// `high` value that is not positive.
    pub fn merge(&self, exchange: String, market: String, kind_min: u32, rows: Vec<ChartCandle>) {
        if rows.is_empty() {
            return;
        }
        let _ = self.tx.send(Op::Merge {
            exchange,
            market,
            kind_min,
            rows,
        });
    }

    /// Enqueues rows for MANY exchange/market/kind groups in one transaction.
    ///
    /// Empty batches are not queued. For each queued item, the worker writes only rows that
    /// pass the same timestamp and `high` validation as `merge`; no database write occurs
    /// when none remain. This call is nonblocking.
    pub fn merge_batch(&self, items: Vec<MergeItem>) {
        if items.is_empty() {
            return;
        }
        let _ = self.tx.send(Op::MergeBatch { items, done: None });
    }

    /// Same as [`Self::merge_batch`], but returns only once the worker has settled the transaction.
    ///
    /// Reads and writes share ONE worker thread and ONE FIFO queue, and a read gives up after
    /// [`READ_TIMEOUT`]. A producer that queues many chunks back to back therefore parks every
    /// chart read behind ALL of them — the recorder's first pass after a restart covers thousands
    /// of markets at once, which is exactly when charts are being opened. Waiting per chunk keeps
    /// at most one transaction ahead of any read, and gives the unbounded queue the backpressure it
    /// otherwise has none of.
    ///
    /// For BACKGROUND writers only: it blocks the caller until the write lands.
    pub fn merge_batch_blocking(&self, items: Vec<MergeItem>) {
        if items.is_empty() {
            return;
        }
        let (done, rx) = mpsc::channel();
        if self
            .tx
            .send(Op::MergeBatch {
                items,
                done: Some(done),
            })
            .is_err()
        {
            return;
        }
        // A dropped sender resolves this immediately, so a dead worker cannot park the producer.
        let _ = rx.recv();
    }

    /// Reads rows whose `t_open` lies in the inclusive `[from_ms, to_ms]` range.
    ///
    /// Blocks for at most [`READ_TIMEOUT`]. `None` means the read did NOT happen — the worker was
    /// gone, or it was busy enough that the answer did not arrive in time — as opposed to
    /// `Some(vec![])`, which is an authoritative "the cache holds nothing there".
    ///
    /// The distinction is load-bearing for the caller, which CACHES this result and only rereads
    /// when the timeframe or left edge changes. Folding a timeout into an empty vector let one busy
    /// moment stick as an empty history prefix for as long as the user did not pan.
    pub fn read_range(
        &self,
        exchange: &str,
        market: &str,
        kind_min: u32,
        from_ms: i64,
        to_ms: i64,
    ) -> Option<Vec<ChartCandle>> {
        let (reply, rx) = mpsc::channel();
        if self
            .tx
            .send(Op::Read {
                exchange: exchange.to_string(),
                market: market.to_string(),
                kind_min,
                from_ms,
                to_ms,
                reply,
            })
            .is_err()
        {
            return None;
        }
        rx.recv_timeout(READ_TIMEOUT).ok()
    }
}

fn init_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chunks(
            exchange TEXT NOT NULL,
            market TEXT NOT NULL,
            kind INTEGER NOT NULL,
            day INTEGER NOT NULL,
            rows BLOB NOT NULL,
            updated_ms INTEGER NOT NULL,
            PRIMARY KEY(exchange, market, kind, day)
        );",
    )?;
    // At startup, delete daily chunks older than the retention limit for their kind.
    let today = now_unix_ms() / DAY_MS;
    let mut del = conn.prepare("DELETE FROM chunks WHERE kind = ?1 AND day < ?2")?;
    for kind in [0u32, 1, 5, 30, 60, 240, 1440] {
        let _ = del.execute(rusqlite::params![kind, today - retention_days(kind)]);
    }
    Ok(())
}

/// Level for the per-key merge trace.
///
/// A constant so the decision is data one test can hold, and so the two emit sites cannot drift.
///
/// `Debug` because of what this cost at `info`, measured 2026-08-15: the set that deduplicates it
/// lives in the writer thread, so "the first merge for each key" starts over on every launch —
/// 6042 distinct keys became **32 803 lines a day**, arriving in bursts of ~4700 within minutes of
/// each start, which made this the largest single producer in the log.
const MERGE_TRACE_LEVEL: log::Level = log::Level::Debug;

/// Announce, once per writer, that the cache has committed something.
///
/// Distinct from the line `open` writes: that one says the file was opened, this one says data is
/// actually reaching it, which is what the per-key flood was really being read for. No counts: the
/// caller knows only that something was stored, and a number that described submitted rows would
/// read as stored ones.
///
/// Returns whether it announced, so "once" is a fact a test can observe rather than a claim.
///
/// Args:
///     announced: Per-writer latch, set here on the first call.
///
/// Returns:
///     `true` on the call that wrote the line, `false` for every call after it.
fn log_active_once(announced: &mut bool) -> bool {
    if *announced {
        return false;
    }
    *announced = true;
    log::info!("kline cache: first merge committed");
    true
}

/// Records that `key` was traced, and returns whether this is the first time.
///
/// The membership set is populated ONLY when `enabled`, and that ordering is the point:
/// `HashSet::insert` takes its key by value, so the previous unconditional form allocated two
/// `String`s for every merged item — roughly 5500 markets per recorder cycle — purely to decide
/// whether to write a log line nobody was reading. With the guard first, a disabled trace costs
/// nothing and the set stays empty.
///
/// Bounded per key rather than per merge on purpose: a cycle covers thousands of markets, so an
/// unbounded trace would overrun the Log panel's 5000-record ring the moment it was switched on.
///
/// `enabled` is a parameter rather than a `log_enabled!` inside, so both branches are testable
/// without a unit test installing a process-global logger.
///
/// Args:
///     seen: Keys this writer has already traced.
///     key: Exchange, market and kind just merged.
///     enabled: Whether the trace level is live.
///
/// Returns:
///     Whether this key should be logged now.
fn trace_first_merge(
    seen: &mut std::collections::HashSet<(String, String, u32)>,
    key: TraceKey<'_>,
    enabled: bool,
) -> bool {
    if !enabled {
        return false;
    }
    seen.insert((key.0.to_string(), key.1.to_string(), key.2))
}

/// Exchange, market and kind identifying one cache key for the trace.
type TraceKey<'a> = (&'a str, &'a str, u32);

fn run(conn: rusqlite::Connection, rx: mpsc::Receiver<Op>) {
    // One INFO line per writer says data is reaching the cache; the per-key detail is a trace whose
    // deduplication set stays EMPTY unless that trace is on.
    //
    // The batch arm marks keys traced inside the loop, before its commit: a rolled-back cycle
    // therefore loses those "first rows" lines rather than repeating them. Acceptable for a
    // debug-only breadcrumb on a path that already warns about the failed commit.
    let mut announced = false;
    let mut seen: std::collections::HashSet<(String, String, u32)> =
        std::collections::HashSet::new();
    while let Ok(op) = rx.recv() {
        match op {
            Op::Merge {
                exchange,
                market,
                kind_min,
                rows,
            } => {
                let now = now_unix_ms();
                let res = conn.unchecked_transaction().and_then(|tx| {
                    let wrote = upsert_one(&tx, &exchange, &market, kind_min, &rows, now)?;
                    tx.commit()?;
                    Ok(wrote)
                });
                match res {
                    Err(e) => {
                        log::warn!("kline cache merge failed {exchange}/{market}/{kind_min}: {e}");
                    }
                    // Committed AND non-empty. A merge whose rows were all filtered out stored
                    // nothing, and announcing it would burn the one-shot latch on a write that
                    // never happened.
                    Ok(wrote) => {
                        if wrote {
                            let _ = log_active_once(&mut announced);
                            let enabled = log::log_enabled!(MERGE_TRACE_LEVEL);
                            let key = (exchange.as_str(), market.as_str(), kind_min);
                            if trace_first_merge(&mut seen, key, enabled) {
                                log::log!(
                                    MERGE_TRACE_LEVEL,
                                    "kline cache: first rows {exchange}/{market}/kind{kind_min}: {}",
                                    rows.len()
                                );
                            }
                        }
                    }
                }
            }
            Op::MergeBatch { items, done } => {
                // Released however this arm ends, including the early `continue` below, so a
                // blocking producer is never parked by a transaction that failed to open.
                let _settled = done.map(SettledOnDrop);
                let now = now_unix_ms();
                let tx = match conn.unchecked_transaction() {
                    Ok(tx) => tx,
                    Err(e) => {
                        log::warn!("kline cache batch tx failed ({} items): {e}", items.len());
                        continue;
                    }
                };
                // An item failure does not abort the SQLite transaction. Log it, write the
                // remaining items, then commit the entire cycle once.
                let enabled = log::log_enabled!(MERGE_TRACE_LEVEL);
                let mut merged_any = false;
                for it in &items {
                    match upsert_one(&tx, &it.exchange, &it.market, it.kind_min, &it.rows, now) {
                        Ok(wrote) => {
                            merged_any |= wrote;
                            let key = (it.exchange.as_str(), it.market.as_str(), it.kind_min);
                            if wrote && trace_first_merge(&mut seen, key, enabled) {
                                log::log!(
                                    MERGE_TRACE_LEVEL,
                                    "kline cache: first rows {}/{}/kind{}: {}",
                                    it.exchange,
                                    it.market,
                                    it.kind_min,
                                    it.rows.len()
                                );
                            }
                        }
                        Err(e) => log::warn!(
                            "kline cache batch merge {}/{}/{}: {e}",
                            it.exchange,
                            it.market,
                            it.kind_min
                        ),
                    }
                }
                // Announced only AFTER the commit succeeds. Announcing beside the loop would claim
                // a write for a cycle a failed commit rolled back — and, because the latch is
                // one-shot, would then stay silent for the first cycle that really landed.
                //
                // `merged_any` follows the items that reported a write. An item that errors partway
                // through its days has already written the earlier ones into this transaction and
                // is not counted, so a cycle can commit rows without announcing; the latch is only
                // ever SET on success, so the next clean cycle says it instead.
                match tx.commit() {
                    Ok(()) => {
                        if merged_any {
                            let _ = log_active_once(&mut announced);
                        }
                    }
                    Err(e) => log::warn!(
                        "kline cache batch commit failed ({} items): {e}",
                        items.len()
                    ),
                }
            }
            Op::Read {
                exchange,
                market,
                kind_min,
                from_ms,
                to_ms,
                reply,
            } => {
                let rows = read_rows(&conn, &exchange, &market, kind_min, from_ms, to_ms)
                    .unwrap_or_else(|e| {
                        log::warn!("kline cache read failed {exchange}/{market}/{kind_min}: {e}");
                        Vec::new()
                    });
                let _ = reply.send(rows);
            }
        }
    }
}

/// Writes rows for one exchange, market, and kind through an ALREADY OPEN transaction.
///
/// `conn` may be a regular `Connection` or a dereferenced `Transaction`. The caller owns
/// transaction and commit management, allowing both a single merge and a batch of
/// thousands to use the caller's chosen commit granularity.
///
/// Returns:
///     Whether any chunk was written. `false` means every row failed the timestamp/`high` filter,
///     which is a successful call that stored nothing — the distinction the liveness line needs,
///     and one a bare `Ok(())` hid from its caller.
fn upsert_one(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    kind_min: u32,
    rows: &[ChartCandle],
    now: i64,
) -> rusqlite::Result<bool> {
    // Group by day and merge by t_open within each day; BTreeMap preserves ordering.
    let mut by_day: BTreeMap<i64, Vec<&ChartCandle>> = BTreeMap::new();
    for r in rows {
        if !(r.t_open_ms.is_finite() && r.t_open_ms > 0.0) || !(r.high > 0.0) {
            continue;
        }
        by_day
            .entry(r.t_open_ms as i64 / DAY_MS)
            .or_default()
            .push(r);
    }
    // Nothing survived the filter, so nothing is written — reported to the caller rather than
    // hidden behind `Ok`, because "the write succeeded" and "the write happened" are the two
    // different things the liveness line is read to tell apart.
    if by_day.is_empty() {
        return Ok(false);
    }
    for (day, day_rows) in by_day {
        let existing: Option<Vec<u8>> = conn
            .query_row(
                "SELECT rows FROM chunks WHERE exchange=?1 AND market=?2 AND kind=?3 AND day=?4",
                rusqlite::params![exchange, market, kind_min, day],
                |r| r.get(0),
            )
            .ok();
        let day_start = day * DAY_MS;
        let mut merged: BTreeMap<u32, ChartCandle> = BTreeMap::new();
        if let Some(blob) = existing {
            for c in unpack_rows(&blob, day_start) {
                merged.insert((c.t_open_ms as i64 - day_start) as u32, c);
            }
        }
        for r in day_rows {
            merged.insert((r.t_open_ms as i64 - day_start) as u32, r.clone());
        }
        let blob = pack_rows(merged.values(), day_start);
        conn.execute(
            "INSERT OR REPLACE INTO chunks(exchange, market, kind, day, rows, updated_ms)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![exchange, market, kind_min, day, blob, now],
        )?;
    }
    Ok(true)
}

fn read_rows(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    kind_min: u32,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<ChartCandle>> {
    let mut stmt = conn.prepare(
        "SELECT day, rows FROM chunks
         WHERE exchange=?1 AND market=?2 AND kind=?3 AND day BETWEEN ?4 AND ?5
         ORDER BY day",
    )?;
    let mut out = Vec::new();
    let mut q = stmt.query(rusqlite::params![
        exchange,
        market,
        kind_min,
        from_ms / DAY_MS,
        to_ms / DAY_MS
    ])?;
    while let Some(row) = q.next()? {
        let day: i64 = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        for c in unpack_rows(&blob, day * DAY_MS) {
            let t = c.t_open_ms as i64;
            if t >= from_ms && t <= to_ms {
                out.push(c);
            }
        }
    }
    Ok(out)
}

fn pack_rows<'a>(rows: impl Iterator<Item = &'a ChartCandle>, day_start: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for r in rows {
        out.extend_from_slice(&((r.t_open_ms as i64 - day_start) as u32).to_le_bytes());
        for v in [r.open, r.high, r.low, r.close, r.volume] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn unpack_rows(blob: &[u8], day_start: i64) -> Vec<ChartCandle> {
    let mut out = Vec::with_capacity(blob.len() / ROW_BYTES);
    for chunk in blob.chunks_exact(ROW_BYTES) {
        let off = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let f = |i: usize| f32::from_le_bytes(chunk[i..i + 4].try_into().unwrap());
        out.push(ChartCandle {
            t_open_ms: (day_start + off as i64) as f64,
            open: f(4),
            high: f(8),
            low: f(12),
            close: f(16),
            volume: f(20),
        });
    }
    out
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests;
