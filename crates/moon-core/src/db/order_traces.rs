//! Local archive of closed trades' order traces: `data/order_traces.sqlite`.
//!
//! The core answers `request_traces(ReportUID)` from an archive it keeps for a bounded time, and
//! MoonProto keeps no cache of its own (`docs/reports.md`, "Archived Order Traces"): it asks the
//! terminal to store what it received, keyed by the report row's `ReportUID`, and to ask the core
//! only for what it does not have. This file is that store. It is NOT part of `reports.sqlite` for
//! the reason `strategies.sqlite` is not: the replica is rebuilt from the core on demand, while a
//! trace the core has aged out exists nowhere else.
//!
//! # One writer, three questions
//!
//! A single thread owns the write connection, like the strategy and report writers, and takes
//! three messages from the feed of any core:
//!
//! - [`TraceDbMsg::Answer`] — the core answered for one row; an EMPTY answer is filed too, as a
//!   row with no lines and the time it was checked, so the same old trade is not asked about on
//!   every start. Empty is not forever: the core may backfill its archive at its own startup, so
//!   an empty answer older than [`EMPTY_RECHECK`] counts as unknown again.
//! - [`TraceDbMsg::RowClosed`] — a report row just closed. The writer decides whether the core
//!   should be asked (nothing filed, or an empty answer past its recheck age) and hands the uid
//!   back through the feed's [`AskSink`]. The feed never consults the file itself: what is known
//!   lives in one place.
//! - [`TraceDbMsg::Backfill`] — the core's replica catch-up finished. The writer lists the closed
//!   rows of that core, newest first, that have no usable answer and closed within
//!   [`BACKFILL_DEPTH`], and hands them to the feed, which asks at its own pace.
//!
//! Reads ([`read_many`]) open their own read-only connection per call, off the UI thread, the way
//! the trade window reads the replica.
//!
//! # Keys
//!
//! `ReportUID` is a random 64-bit identity the core preserves across database copies; `0` is the
//! replica's local placeholder for rows received before the core reported the column and is never
//! a key here. `core_uid` stays in the key so a deleted core's rows are not another core's, and so
//! this store takes part in the "never reissue a core uid" floor like every other core-keyed file.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use super::read_fail::read_fail;
use super::{ReadFail, ReadResult};
use crate::config::paths;
use crate::feed::{ArchivedLineKind, ArchivedOrderTrace};
use crate::util::now_unix_ms_i64;

/// How far back the startup backfill reaches, by the row's close time.
///
/// The core's archive is bounded in time and an ask past its horizon only earns an empty answer;
/// the exact horizon is the core's to state, and until it does this is the working assumption.
pub const BACKFILL_DEPTH: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// After how long an EMPTY answer is asked about again.
///
/// The core uses the same empty answer for an archive-storage failure and can backfill older
/// trades when it starts, so "empty" is a fact with a date on it, not a permanent one.
pub const EMPTY_RECHECK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Most rows one backfill hands to the feed. Newest first, so a cap cuts the oldest — the ones
/// nearest the core's own horizon, where an empty answer is likeliest anyway.
pub const BACKFILL_LIMIT: usize = 400;

/// Slack added to the backfill cutoff for the replica clock: `closedate` is the core's LOCAL
/// wall clock in seconds (see `db::report_axis`), compared here against the terminal's UTC. One
/// day covers any time zone plus the ping offset, and costs nothing but a few more candidates.
const CLOCK_SLACK: Duration = Duration::from_secs(24 * 60 * 60);

/// Bound on queued writer messages. A closed trade is one message and a backfill is one message,
/// so the queue only ever holds a burst of closes; past it the feed drops the message and logs.
const QUEUE_CAP: usize = 1024;

/// Bound on queued asks travelling back to one feed. Per core, and a backfill is the only sender
/// of more than one uid at a time.
const ASK_CAP: usize = 64;

/// Schema revision written to `app_meta`; bumped only with a migration beside it.
const SCHEMA_VERSION: i64 = 1;

/// Stored ordinal of [`ArchivedLineKind::Entry`].
const KIND_ENTRY: i64 = 0;
/// Stored ordinal of [`ArchivedLineKind::Exit`].
const KIND_EXIT: i64 = 1;

/// What the store holds for one report row.
#[derive(Clone, Debug, PartialEq)]
pub enum TraceEntry {
    /// The core answered with lines, filed as received.
    Lines(Arc<[ArchivedOrderTrace]>),
    /// The core answered "no archive" at `checked_at_ms`. Whether that is still worth believing
    /// is the reader's call against [`EMPTY_RECHECK`]; the store reports the fact and the date.
    Empty { checked_at_ms: i64 },
}

impl TraceEntry {
    /// Whether this entry still answers the question, so the core need not be asked.
    ///
    /// Lines always do. An empty answer does until it is [`EMPTY_RECHECK`] old.
    ///
    /// Args:
    ///     now_ms: Terminal wall clock, Unix ms.
    pub fn is_current(&self, now_ms: i64) -> bool {
        match self {
            Self::Lines(_) => true,
            Self::Empty { checked_at_ms } => {
                now_ms.saturating_sub(*checked_at_ms) < EMPTY_RECHECK.as_millis() as i64
            }
        }
    }
}

/// The feed's side of the writer's answers: uids the core should be asked about.
///
/// Carried inside each message rather than registered once, so a reconnected feed with a fresh
/// channel cannot receive a list meant for the loop that died. `wake` is the feed loop's own wake
/// sender: the loop sleeps on its deadlines and would otherwise read the list only on the next
/// unrelated event.
#[derive(Clone)]
pub struct AskSink {
    asks: SyncSender<Vec<i64>>,
    wake: std::sync::mpsc::Sender<()>,
}

impl AskSink {
    /// Build the sink from the feed's receiving end and its wake channel.
    ///
    /// Args:
    ///     wake: The feed loop's wake sender.
    ///
    /// Returns:
    ///     The sink to put in messages, and the receiver the loop drains.
    pub fn new(wake: std::sync::mpsc::Sender<()>) -> (Self, Receiver<Vec<i64>>) {
        let (asks, rx) = std::sync::mpsc::sync_channel(ASK_CAP);
        (Self { asks, wake }, rx)
    }

    /// Hand a list of uids to the feed and wake it. A full or closed channel drops the list: the
    /// feed is gone or swamped, and the next close or backfill produces the list again.
    fn send(&self, uids: Vec<i64>) {
        if uids.is_empty() {
            return;
        }
        match self.asks.try_send(uids) {
            Ok(()) => {
                let _ = self.wake.send(());
            }
            Err(TrySendError::Full(list)) => {
                log::warn!(
                    "[x] order traces: {} ask(s) dropped, the feed's queue is full",
                    list.len()
                );
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

/// What the feed sends the writer.
pub enum TraceDbMsg {
    /// The core's answer for one closed row, empty included. A FAILED request is never filed.
    Answer {
        core_uid: u64,
        report_uid: i64,
        lines: Arc<[ArchivedOrderTrace]>,
    },
    /// A report row of this core closed; ask the core unless the store already knows.
    RowClosed {
        core_uid: u64,
        report_uid: i64,
        ask: AskSink,
    },
    /// This core's replica catch-up completed; list what closed recently and is still unknown.
    Backfill { core_uid: u64, ask: AskSink },
}

/// Channel to the writer plus its commit generation.
#[derive(Clone)]
pub struct TraceSink {
    tx: SyncSender<TraceDbMsg>,
    generation: Arc<AtomicU64>,
}

impl TraceSink {
    /// Queue one message for the writer without blocking the feed.
    ///
    /// A full queue drops the message with a warning rather than stalling the feed thread behind
    /// disk: an answer lost this way is asked for again by the next backfill, and a close hook
    /// lost this way is covered the same way.
    ///
    /// Args:
    ///     msg: The message.
    pub fn send(&self, msg: TraceDbMsg) {
        if let Err(error) = self.tx.try_send(msg) {
            let what = match error {
                TrySendError::Full(_) => "queue full",
                TrySendError::Disconnected(_) => "writer gone",
            };
            log::warn!("[x] order traces: message dropped ({what})");
        }
    }

    /// Number of committed answer writes since the writer started.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

static SINK: OnceLock<Option<TraceSink>> = OnceLock::new();

/// Channel to the writer; `None` when the database could not be opened.
pub fn sink() -> Option<TraceSink> {
    SINK.get_or_init(spawn_writer).clone()
}

/// Create the tables, or verify a file written by an earlier build.
///
/// Args:
///     conn: Write connection.
///
/// Errors:
///     Any SQLite failure; a schema version this build does not know is one too.
fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let _ = conn.busy_timeout(Duration::from_secs(3));
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS trace_answers (
             core_uid INTEGER NOT NULL,
             report_uid INTEGER NOT NULL,
             checked_at_ms INTEGER NOT NULL,
             line_count INTEGER NOT NULL,
             PRIMARY KEY (core_uid, report_uid)
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS trace_lines (
             core_uid INTEGER NOT NULL,
             report_uid INTEGER NOT NULL,
             seq INTEGER NOT NULL,
             own INTEGER NOT NULL,
             kind INTEGER NOT NULL,
             stop_price REAL,
             stop_time_ms INTEGER,
             points BLOB NOT NULL,
             PRIMARY KEY (core_uid, report_uid, seq)
         ) WITHOUT ROWID;",
    )?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match stored.as_deref().map(str::parse::<i64>) {
        None => {
            conn.execute(
                "INSERT INTO app_meta (key, value) VALUES ('schema_version', ?1)",
                [SCHEMA_VERSION.to_string()],
            )?;
        }
        Some(Ok(version)) if version == SCHEMA_VERSION => {}
        Some(other) => {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
                Some(format!(
                    "order_traces.sqlite schema_version {other:?}, this build knows {SCHEMA_VERSION}"
                )),
            ));
        }
    }
    Ok(())
}

/// Open the file, create the schema, and start the writer thread.
fn spawn_writer() -> Option<TraceSink> {
    let path = paths::order_traces_db_path();
    let conn = match Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            log::error!("order traces: cannot open {}: {e}", path.display());
            return None;
        }
    };
    if let Err(e) = init(&conn) {
        log::error!("order traces: schema init failed: {e}");
        return None;
    }
    let (tx, rx): (SyncSender<TraceDbMsg>, Receiver<TraceDbMsg>) =
        std::sync::mpsc::sync_channel(QUEUE_CAP);
    let generation = Arc::new(AtomicU64::new(0));
    let gen_writer = generation.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("order-traces-db".into())
        .spawn(move || {
            log::info!("order traces: writer started ({})", path.display());
            let traces_path = path.clone();
            while let Ok(msg) = rx.recv() {
                match msg {
                    TraceDbMsg::Answer {
                        core_uid,
                        report_uid,
                        lines,
                    } => match store_answer(&conn, core_uid, report_uid, &lines, now_unix_ms_i64())
                    {
                        Ok(()) => {
                            gen_writer.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => log::error!(
                            "[x] order traces: core {core_uid} uid={report_uid} not written: {e}"
                        ),
                    },
                    TraceDbMsg::RowClosed {
                        core_uid,
                        report_uid,
                        ask,
                    } => match needs_ask(&conn, core_uid, report_uid, now_unix_ms_i64()) {
                        Ok(true) => ask.send(vec![report_uid]),
                        Ok(false) => {}
                        Err(e) => log::warn!(
                            "[x] order traces: core {core_uid} uid={report_uid} lookup failed: {e}"
                        ),
                    },
                    TraceDbMsg::Backfill { core_uid, ask } => {
                        match backfill_candidates(&traces_path, core_uid, now_unix_ms_i64()) {
                            Ok(uids) => {
                                log::info!(
                                    "order traces: core {core_uid} backfill — {} closed row(s) without a trace",
                                    uids.len()
                                );
                                ask.send(uids);
                            }
                            Err(ReadFail::NotReady) => {}
                            Err(e) => log::warn!(
                                "[x] order traces: core {core_uid} backfill listing failed: {e}"
                            ),
                        }
                    }
                }
            }
            log::info!("order traces: writer stopped");
        })
    {
        log::error!("order traces: writer thread did not start: {e}");
        return None;
    }
    Some(TraceSink { tx, generation })
}

/// File one answer: replace the row's lines and stamp the check time.
///
/// Args:
///     conn: Write connection.
///     core_uid: Core that recorded the trade.
///     report_uid: The row's `ReportUID`.
///     lines: What the core answered; empty files an empty answer.
///     now_ms: Terminal wall clock, Unix ms.
///
/// Errors:
///     Any SQLite failure; the transaction is rolled back.
fn store_answer(
    conn: &Connection,
    core_uid: u64,
    report_uid: i64,
    lines: &[ArchivedOrderTrace],
    now_ms: i64,
) -> rusqlite::Result<()> {
    let core = core_uid as i64;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM trace_lines WHERE core_uid=?1 AND report_uid=?2",
        params![core, report_uid],
    )?;
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO trace_lines (core_uid, report_uid, seq, own, kind, stop_price, \
             stop_time_ms, points) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (seq, line) in lines.iter().enumerate() {
            let kind = match line.kind {
                ArchivedLineKind::Entry => KIND_ENTRY,
                ArchivedLineKind::Exit => KIND_EXIT,
            };
            insert.execute(params![
                core,
                report_uid,
                seq as i64,
                i64::from(line.own),
                kind,
                line.stop_price,
                line.stop_time_ms.map(|ms| ms as i64),
                encode_points(&line.points),
            ])?;
        }
    }
    tx.execute(
        "INSERT INTO trace_answers (core_uid, report_uid, checked_at_ms, line_count) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(core_uid, report_uid) DO UPDATE SET \
         checked_at_ms=excluded.checked_at_ms, line_count=excluded.line_count",
        params![core, report_uid, now_ms, lines.len() as i64],
    )?;
    tx.commit()
}

/// Whether a just-closed row should be asked about: nothing filed, or an empty answer past its
/// recheck age.
fn needs_ask(
    conn: &Connection,
    core_uid: u64,
    report_uid: i64,
    now_ms: i64,
) -> rusqlite::Result<bool> {
    let filed: Option<(i64, i64)> = conn
        .query_row(
            "SELECT checked_at_ms, line_count FROM trace_answers WHERE core_uid=?1 AND report_uid=?2",
            params![core_uid as i64, report_uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match filed {
        None => true,
        Some((_, count)) if count > 0 => false,
        Some((checked_at_ms, _)) => !TraceEntry::Empty { checked_at_ms }.is_current(now_ms),
    })
}

/// Points as stored: `f64` Unix ms then `f64` price, little-endian, 16 bytes per point.
///
/// The price keeps the wire's width on purpose — the protocol asks that the saved archive not be
/// downcast — and the time is already an `f64` in the domain type.
fn encode_points(points: &[(f64, f64)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(points.len() * 16);
    for &(time_ms, price) in points {
        out.extend_from_slice(&time_ms.to_le_bytes());
        out.extend_from_slice(&price.to_le_bytes());
    }
    out
}

/// Inverse of [`encode_points`]. A trailing partial point is dropped, never guessed at.
fn decode_points(blob: &[u8]) -> Vec<(f64, f64)> {
    blob.chunks_exact(16)
        .map(|chunk| {
            let mut time = [0u8; 8];
            let mut price = [0u8; 8];
            time.copy_from_slice(&chunk[..8]);
            price.copy_from_slice(&chunk[8..]);
            (f64::from_le_bytes(time), f64::from_le_bytes(price))
        })
        .collect()
}

/// Read-only connection to the store, or `None` when there is no file yet.
fn open_ro(path: &std::path::Path) -> ReadResult<Option<Connection>> {
    const CTX: &str = "order traces: open";
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| read_fail(CTX, e))?;
    let _ = conn.busy_timeout(Duration::from_secs(3));
    super::trace::install_on(&conn);
    Ok(Some(conn))
}

/// What the store holds for each of `uids` on `core_uid`. A uid it holds nothing for is absent.
///
/// Opens a read-only connection per call: the callers run this off the UI thread and ask once
/// per window open or per history load, not per frame.
///
/// Args:
///     core_uid: Core that recorded the trades.
///     uids: `ReportUID`s to look up; zeros and duplicates are harmless.
///
/// Returns:
///     Found entries by uid. Empty when the file does not exist yet.
///
/// Errors:
///     Open or query failures.
pub fn read_many(core_uid: u64, uids: &[i64]) -> ReadResult<HashMap<i64, TraceEntry>> {
    let Some(conn) = open_ro(&paths::order_traces_db_path())? else {
        return Ok(HashMap::new());
    };
    read_many_on(&conn, core_uid, uids)
}

/// [`read_many`] on an open connection, for the reader and its tests.
fn read_many_on(
    conn: &Connection,
    core_uid: u64,
    uids: &[i64],
) -> ReadResult<HashMap<i64, TraceEntry>> {
    const CTX: &str = "order traces: read";
    let core = core_uid as i64;
    let mut out = HashMap::with_capacity(uids.len());
    let mut answer = conn
        .prepare_cached(
            "SELECT checked_at_ms, line_count FROM trace_answers WHERE core_uid=?1 AND report_uid=?2",
        )
        .map_err(|e| read_fail(CTX, e))?;
    let mut lines = conn
        .prepare_cached(
            "SELECT own, kind, stop_price, stop_time_ms, points FROM trace_lines \
             WHERE core_uid=?1 AND report_uid=?2 ORDER BY seq",
        )
        .map_err(|e| read_fail(CTX, e))?;
    for &uid in uids {
        if uid == 0 || out.contains_key(&uid) {
            continue;
        }
        let filed: Option<(i64, i64)> = answer
            .query_row(params![core, uid], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| read_fail(CTX, e))?;
        let Some((checked_at_ms, count)) = filed else {
            continue;
        };
        if count == 0 {
            out.insert(uid, TraceEntry::Empty { checked_at_ms });
            continue;
        }
        let rows = lines
            .query_map(params![core, uid], |r| {
                let own: i64 = r.get(0)?;
                let kind: i64 = r.get(1)?;
                let stop_price: Option<f64> = r.get(2)?;
                let stop_time_ms: Option<i64> = r.get(3)?;
                let blob: Vec<u8> = r.get(4)?;
                Ok((own, kind, stop_price, stop_time_ms, blob))
            })
            .map_err(|e| read_fail(CTX, e))?;
        let mut traces = Vec::new();
        for row in rows {
            let (own, kind, stop_price, stop_time_ms, blob) = row.map_err(|e| read_fail(CTX, e))?;
            let kind = match kind {
                KIND_ENTRY => ArchivedLineKind::Entry,
                KIND_EXIT => ArchivedLineKind::Exit,
                other => {
                    log::warn!("[x] order traces: uid={uid} line kind {other} unknown, skipped");
                    continue;
                }
            };
            let points = decode_points(&blob);
            if points.is_empty() {
                continue;
            }
            traces.push(ArchivedOrderTrace {
                own: own != 0,
                kind,
                stop_price,
                stop_time_ms: stop_time_ms.map(|ms| ms as f64),
                points,
            });
        }
        // The answer said it had lines; a row whose lines all failed to decode is reported as
        // what it is on disk — an answer with lines — so the core is not asked again for it.
        out.insert(uid, TraceEntry::Lines(Arc::from(traces)));
    }
    Ok(out)
}

/// Closed rows of `core_uid` in the replica that the store has no current answer for.
///
/// Opens the REPORT reader and attaches this file: the readiness and integrity gates of the
/// replica apply, and an unavailable replica is `NotReady` rather than an empty list.
///
/// Args:
///     traces_path: This store's file.
///     core_uid: Core whose catch-up completed.
///     now_ms: Terminal wall clock, Unix ms.
///
/// Returns:
///     `ReportUID`s newest close first, at most [`BACKFILL_LIMIT`].
fn backfill_candidates(
    traces_path: &std::path::Path,
    core_uid: u64,
    now_ms: i64,
) -> ReadResult<Vec<i64>> {
    const CTX: &str = "order traces: attach";
    let conn = super::open_reader()?;
    if !traces_path.exists() {
        return Ok(Vec::new());
    }
    conn.execute(
        "ATTACH DATABASE ?1 AS traces",
        [traces_path.to_string_lossy().as_ref()],
    )
    .map_err(|e| read_fail(CTX, e))?;
    backfill_candidates_on(&conn, core_uid, now_ms)
}

/// [`backfill_candidates`] on a report reader with the store attached as `traces`.
///
/// `closedate` is compared raw, in the core's own clock, against a UTC cutoff widened by
/// [`CLOCK_SLACK`]; the column is never routed through the report axis here for the reason the
/// valuation worker gives — it is a bound, not a displayed instant, and a day of slack is cheaper
/// than a second derivation of the offset.
fn backfill_candidates_on(conn: &Connection, core_uid: u64, now_ms: i64) -> ReadResult<Vec<i64>> {
    const CTX: &str = "order traces: backfill listing";
    let cols = replica_columns(conn)?;
    if !["reportuid", "closedate"]
        .iter()
        .all(|c| cols.iter().any(|have| have == c))
    {
        return Err(ReadFail::NotReady);
    }
    let deleted_filter = if cols.iter().any(|c| c == "deleted") {
        "AND COALESCE(r.deleted, 0) = 0 "
    } else {
        ""
    };
    let cutoff_secs = (now_ms / 1000)
        .saturating_sub(BACKFILL_DEPTH.as_secs() as i64)
        .saturating_sub(CLOCK_SLACK.as_secs() as i64);
    // Strictly newer than this is current — the same `age < EMPTY_RECHECK` as
    // `TraceEntry::is_current`, so the two paths agree at the boundary.
    let recheck_before_ms = now_ms.saturating_sub(EMPTY_RECHECK.as_millis() as i64);
    let sql = format!(
        "SELECT r.reportuid FROM orders_rep r \
         WHERE r.core_uid = ?1 AND r.reportuid IS NOT NULL AND r.reportuid <> 0 \
         AND r.closedate IS NOT NULL AND r.closedate > 0 AND r.closedate >= ?2 \
         {deleted_filter}\
         AND NOT EXISTS (SELECT 1 FROM traces.trace_answers a \
             WHERE a.core_uid = r.core_uid AND a.report_uid = r.reportuid \
             AND (a.line_count > 0 OR a.checked_at_ms > ?3)) \
         ORDER BY r.closedate DESC LIMIT ?4"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(
            params![
                core_uid as i64,
                cutoff_secs,
                recheck_before_ms,
                BACKFILL_LIMIT as i64
            ],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Column names of the replica table on this reader; empty when the table is absent.
fn replica_columns(conn: &Connection) -> ReadResult<Vec<String>> {
    const CTX: &str = "order traces: PRAGMA table_info(orders_rep)";
    let mut stmt = conn
        .prepare("PRAGMA table_info(orders_rep)")
        .map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for name in rows {
        out.push(name.map_err(|e| read_fail(CTX, e))?.to_lowercase());
    }
    Ok(out)
}

/// Highest `core_uid` this store has ever recorded, for the startup uid floor.
///
/// Like the strategy store's: a missing file is `Ok(None)`; a file that fails to open or read is
/// an error the caller reports, not a silent zero that would let a uid be reissued.
pub fn max_core_uid() -> ReadResult<Option<u64>> {
    const CTX: &str = "order traces: max_core_uid";
    let Some(conn) = open_ro(&paths::order_traces_db_path())? else {
        return Ok(None);
    };
    super::max_core_uid_in(&conn, "trace_answers", CTX)
}

#[cfg(test)]
mod tests;
