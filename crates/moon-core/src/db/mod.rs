//! Local SQLite database for Orders reports.
//!
//! The PRIMARY path is a typed replica of the server database (moonproto
//! `Event::Report`, the `orders_rep` table; see [`rep`]): the core supplies the
//! schema, rows are upserted or deleted by `newRecID`, offline changes catch up
//! from a cursor, and `SyncComplete` reconciles the replica. Analytical fields
//! (deltas, pump, signaltype, and others) arrive with their real values.
//!
//! LEGACY table `closed_sell_reports` (PK `(core_uid, db_id)`) is READ-ONLY: the
//! terminal no longer consumes `Event::ClosedSellOrderReport`, so no new legacy
//! rows are written. The report window still UNION-s its existing rows while any
//! core lingers on it, and [`rep`]'s per-core purge drops the table once every
//! core is on typed replication (marker `legacy_dropped`).
//!
//! ONE writer thread performs writes; the Reports window reads through a separate
//! connection (WAL).

pub mod analytics;
pub mod coin_lists;
mod dates;
pub mod integrity;
pub mod maint;
pub mod metrics;
mod quote;
pub(crate) mod read_fail;
mod rep;
mod report_read;
pub mod report_recovery;
#[cfg(test)]
mod test_support;
pub mod tuner;
pub mod valuation;

pub use dates::{fmt_unix, fmt_unix_date, fmt_unix_secs, parse_ymd};
pub use quote::{
    ProfitScope, ProfitUnit, QuoteBreakdown, QuoteCurrency, QuoteScope, QuoteTotal, UsdtTotal,
    ValuationCoverage,
};
pub use read_fail::{FailKind, ReadFail, ReadResult};
pub use rep::{DbMsg, ReportSink};
pub(crate) use report_read::max_core_uid_in;
pub use report_read::{
    display_columns, distinct_cores, distinct_strategies, max_core_uid, query_reports,
    query_totals, ProfitMetric, ReportFilter, ReportStrategy, ReportStrategyKey, ReportTable,
    SideFilter, COLUMNS_ADDED_SINCE_V2, DISPLAY_COLUMNS, VALUATION_PROFIT_COLUMN,
    VALUATION_RATE_COLUMN, VALUATION_SOURCE_COLUMN,
};

use read_fail::read_fail;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;

use crate::config::paths;

/// Feed-to-writer sink for typed replication messages plus per-core start cursors.
pub type ReportTx = ReportSink;

/// Database handle containing the writer channel and post-commit publication state.
///
/// The generation is the source of truth; bounded dirty edges distinguish immediate live changes
/// from coalesced historical catch-up after successful commits.
pub struct ReportsHandle {
    /// Typed replication sink used by core feed sessions.
    pub tx: ReportTx,
    /// Monotonic committed report generation.
    pub generation: Arc<AtomicU64>,
    /// Coalescing edge for live report changes that must refresh UI immediately.
    pub immediate_commit_dirty: Arc<AtomicBool>,
    /// Coalescing edge for historical catch-up changes that may refresh UI in bulk.
    pub background_commit_dirty: Arc<AtomicBool>,
}

/// Initialize shared report metadata and any still-present read-only legacy table.
///
/// A fresh database creates no `closed_sell_reports` table. Existing compatible
/// tables receive only the indexes needed by report reads; the obsolete
/// `server_id` schema is dropped because no protocol-v4 writer can repopulate it.
fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let _ = conn.busy_timeout(Duration::from_secs(3));

    conn.execute(
        "CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )?;
    valuation::init_report_outbox(conn)?;

    // The legacy table was already dropped after the full typed-replica migration.
    // Do not recreate it: CREATE IF NOT EXISTS would bring the dead table back.
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

    // Very old runtime-`server_id` schema is incompatible with the reader (which
    // selects `core_uid`), so drop it — same as before. We no longer RECREATE the
    // table: legacy rows are not written any more, and the reader gates its query
    // on the table's existence.
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
        log::warn!("отчёты: старая схема (server_id) — таблица снесена");
    }

    // Report-window indexes on the legacy table — only when it still exists (cores
    // on the transition period). A fresh install has no legacy table, so there is
    // nothing to index; `CREATE INDEX` on a missing table would error.
    let legacy_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('closed_sell_reports')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if legacy_exists {
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_csr_closedate ON closed_sell_reports(closedate)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_csr_core ON closed_sell_reports(core_uid)",
            [],
        )?;
    }
    Ok(())
}

/// Probe legacy-table columns (the reader maps legacy rows onto display columns).
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

/// Writer-channel capacity: backpressure instead of OOM.
///
/// About 16k messages consume tens of MB at peak. When the channel is full, the
/// core feed thread waits for the writer, naturally throttling catch-up.
const REPORT_QUEUE_CAP: usize = 16_384;

/// Maximum attempts for one owned writer batch before the writer fails closed.
const REPORT_BATCH_ATTEMPTS: usize = 4;

/// Maximum exhausted retry rounds before a non-corruption error closes the writer.
const REPORT_BATCH_MAX_FAILED_ROUNDS: u32 = 8;

/// Report-data publication urgency for one successfully committed writer batch.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ReportPublication {
    /// Historical catch-up data may be coalesced before waking analytical readers.
    Background,
    /// Live or user-visible data must wake analytical readers immediately.
    Immediate,
}

/// Merge one message's publication into the class selected for its writer batch.
///
/// Args:
///     batch: Publication already selected for earlier messages in the batch.
///     next: Publication required by the newly applied message, or `None` for maintenance.
///
/// Returns:
///     The highest urgency observed across the batch.
fn merge_report_publication(
    batch: Option<ReportPublication>,
    next: Option<ReportPublication>,
) -> Option<ReportPublication> {
    batch.max(next)
}

/// Publish one successfully committed report-changing writer batch.
///
/// Each dirty bit is a bounded one-slot signal. Publishing one class deliberately leaves the
/// opposite bit untouched because it may carry an unconsumed edge from an earlier batch.
///
/// Args:
///     generation: Monotonic report snapshot revision shared with readers.
///     immediate_dirty: Coalescing edge for live report mutations.
///     background_dirty: Coalescing edge for historical catch-up mutations.
///     publication: Urgency selected across the committed batch.
///
/// The function has no return value.
fn publish_report_commit(
    generation: &AtomicU64,
    immediate_dirty: &AtomicBool,
    background_dirty: &AtomicBool,
    publication: ReportPublication,
) {
    publish_after_generation(generation, || match publication {
        ReportPublication::Background => background_dirty.store(true, Ordering::Release),
        ReportPublication::Immediate => immediate_dirty.store(true, Ordering::Release),
    });
}

/// Advance the authoritative revision before exposing its causal wake edge.
fn publish_after_generation(generation: &AtomicU64, publish_edge: impl FnOnce()) {
    generation.fetch_add(1, Ordering::Relaxed);
    publish_edge();
}

/// Commit one stateful batch without exposing speculative in-memory mutations.
///
/// The callback receives the same owned batch indirectly on every attempt. A failed
/// begin, apply, or commit drops the transaction and candidate state, then retries
/// with bounded backoff. Exhaustion returns the last SQLite error so the caller can
/// enter recovery without acknowledging or skipping data.
fn commit_stateful_batch<S: Clone, T>(
    conn: &Connection,
    state: &mut S,
    mut apply: impl FnMut(&Connection, &mut S) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let mut last_error = None;
    for attempt in 0..REPORT_BATCH_ATTEMPTS {
        let transaction = match conn.unchecked_transaction() {
            Ok(transaction) => transaction,
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < REPORT_BATCH_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(25u64 << attempt));
                }
                continue;
            }
        };
        let mut candidate = state.clone();
        match apply(&transaction, &mut candidate) {
            Ok(effects) => match transaction.commit() {
                Ok(()) => {
                    *state = candidate;
                    return Ok(effects);
                }
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < REPORT_BATCH_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(25u64 << attempt));
        }
    }
    Err(last_error.unwrap_or(rusqlite::Error::InvalidQuery))
}

/// Keep one owned writer batch fail-closed until SQLite accepts it.
///
/// Each round retains the transaction-level retry policy from [`commit_stateful_batch`].
/// Exhausted transient rounds invoke `wait`, which applies production backoff without dropping the
/// batch. Confirmed damage or the bounded failed-round ceiling closes the writer without
/// acknowledging the owned batch, preventing an infinite retry from wedging the feed thread.
///
/// Args:
///     conn: Sole writer connection.
///     state: Last committed in-memory replica state.
///     apply: Rebuilds one candidate state and transaction on every attempt.
///     should_abort: Classifies a failed round that must stop immediately.
///     wait: Applies backoff after an exhausted retry round.
///
/// Returns:
///     Committed effects, or the last error after fail-closed termination.
fn commit_stateful_batch_until_success<S: Clone, T>(
    conn: &Connection,
    state: &mut S,
    mut apply: impl FnMut(&Connection, &mut S) -> rusqlite::Result<T>,
    mut should_abort: impl FnMut(&rusqlite::Error) -> bool,
    mut wait: impl FnMut(u32, &rusqlite::Error),
) -> rusqlite::Result<T> {
    let mut failed_rounds = 0u32;
    loop {
        match commit_stateful_batch(conn, state, |transaction, candidate| {
            apply(transaction, candidate)
        }) {
            Ok(effects) => return Ok(effects),
            Err(error) => {
                if should_abort(&error) {
                    return Err(error);
                }
                failed_rounds = failed_rounds.saturating_add(1);
                if failed_rounds >= REPORT_BATCH_MAX_FAILED_ROUNDS {
                    return Err(error);
                }
                wait(failed_rounds, &error);
            }
        }
    }
}

/// Result row returned by SQLite's live-safe passive WAL checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WalCheckpointStatus {
    busy: i64,
    log_frames: i64,
    checkpointed_frames: i64,
}

/// Checkpoint every currently available WAL frame without waiting for readers.
fn passive_wal_checkpoint(conn: &Connection) -> rusqlite::Result<WalCheckpointStatus> {
    conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
        Ok(WalCheckpointStatus {
            busy: row.get(0)?,
            log_frames: row.get(1)?,
            checkpointed_frames: row.get(2)?,
        })
    })
}

/// Start the sole report-replica writer and publish each report-changing commit.
///
/// Args:
///     permit: Private capability proving startup recovery and the process lease succeeded.
///
/// Returns:
///     The writer handle, or `None` when the database or writer thread cannot be initialized.
pub fn spawn_writer(_permit: report_recovery::ReportWritePermit) -> Option<ReportsHandle> {
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
    let immediate_commit_dirty = Arc::new(AtomicBool::new(false));
    let writer_immediate_dirty = immediate_commit_dirty.clone();
    let background_commit_dirty = Arc::new(AtomicBool::new(false));
    let writer_background_dirty = background_commit_dirty.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("reports-db".into())
        .spawn(move || {
            log::info!("отчёты: writer запущен ({})", path.display());
            // WAL path used by the lazy checkpoint based on the ACTUAL file size below.
            let wal_path = paths::reports_db_files()[1].clone();
            // Batch messages into one transaction: catch-up emits thousands of rows,
            // and per-row autocommit (fsync) would stretch catch-up into minutes.
            let mut thr_count: u64 = 0;
            let mut thr_started = std::time::Instant::now();
            // Backdate the value by 30 seconds so the first WAL size check becomes eligible
            // roughly 30 seconds after startup, on the next completed batch, rather than 60.
            let mut last_ckpt = std::time::Instant::now()
                .checked_sub(Duration::from_secs(30))
                .unwrap_or_else(std::time::Instant::now);
            loop {
                if integrity::writes_blocked() {
                    log::error!("reports: writer stopped because replica corruption was confirmed");
                    break;
                }
                let first = match rx.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                };
                if integrity::writes_blocked() {
                    log::error!(
                        "reports: writer stopped after receiving a batch because replica \
                         corruption was confirmed"
                    );
                    break;
                }
                let mut batch = vec![first];
                while batch.len() < 512 {
                    match rx.try_recv() {
                        Ok(m) => batch.push(m),
                        Err(_) => break,
                    }
                }
                thr_count += batch.len() as u64;
                // Keep the owned batch until one whole attempt commits. ACK indices are
                // recreated per attempt and become observable only after that commit.
                let (ack_indices, publication) = match commit_stateful_batch_until_success(
                    &conn,
                    &mut rep_state,
                    |transaction, candidate| {
                        let mut ack_indices = Vec::new();
                        let mut publication = None;
                        for (index, msg) in batch.iter().enumerate() {
                            let effect = apply_msg(transaction, candidate, msg)?;
                            publication = merge_report_publication(publication, effect.publication);
                            if effect.page_ack {
                                ack_indices.push(index);
                            }
                        }
                        Ok((ack_indices, publication))
                    },
                    integrity::writer_should_stop,
                    |failed_rounds, error| {
                        let delay =
                            Duration::from_secs(1u64 << failed_rounds.saturating_sub(1).min(5));
                        log::error!(
                            "reports: writer batch still blocked after {} attempts; retrying in \
                             {}s: {error}",
                            failed_rounds as usize * REPORT_BATCH_ATTEMPTS,
                            delay.as_secs()
                        );
                        std::thread::sleep(delay);
                    },
                ) {
                    Ok(indices) => indices,
                    Err(error) => {
                        log::error!(
                            "reports: writer stopped after repeated or permanent replica failure; \
                             owned batch was not acknowledged: {error}"
                        );
                        break;
                    }
                };
                let _ack_guard = integrity::writer_ack_guard();
                // The background scan can publish damage while this transaction is committing.
                // The publication barrier makes this check and every following ACK indivisible
                // from publishing a damage latch. Once damage becomes visible, no later ACK can
                // advance the core past uncertain data.
                if integrity::writes_blocked() {
                    log::error!(
                        "reports: writer stopped after commit because integrity damage was \
                         published; owned batch was not acknowledged"
                    );
                    break;
                }
                for msg in &batch {
                    if let DbMsg::SyncComplete { core_uid, done } = msg {
                        rep::commit_sync_complete(&rep_state, *core_uid, done);
                    }
                }
                if let Some(publication) = publication {
                    publish_report_commit(
                        &gen_writer,
                        &writer_immediate_dirty,
                        &writer_background_dirty,
                        publication,
                    );
                }
                for index in ack_indices {
                    let DbMsg::Page { ack, page, .. } = &batch[index] else {
                        continue;
                    };
                    if let Err(error) = ack.page_applied(page) {
                        log::warn!("reports(rep): page_applied failed: {error:?}");
                    }
                }
                drop(_ack_guard);
                // If this batch dropped the legacy table, run a one-off VACUUM outside the
                // transaction to reclaim its hundreds of MB. Only the writer is blocked.
                if rep_state.vacuum_pending {
                    let t = std::time::Instant::now();
                    match conn.execute("VACUUM", []) {
                        Ok(_) => {
                            rep_state.vacuum_pending = false;
                            log::info!(
                                "reports: post-legacy VACUUM completed in {}s",
                                t.elapsed().as_secs()
                            );
                        }
                        Err(e) => log::warn!("отчёты: VACUUM не удался: {e}"),
                    }
                }
                // A passive checkpoint never waits for an Analytics reader. Inspect its
                // returned progress instead of mistaking a busy result row for success.
                if last_ckpt.elapsed() >= Duration::from_secs(60) {
                    last_ckpt = std::time::Instant::now();
                    let wal_big = std::fs::metadata(&wal_path)
                        .map(|m| m.len() > 32 * 1024 * 1024)
                        .unwrap_or(false);
                    if wal_big {
                        match passive_wal_checkpoint(&conn) {
                            Ok(status) if status.checkpointed_frames < status.log_frames => {
                                log::debug!(
                                    "reports: WAL reader pins {} of {} frames (busy={})",
                                    status.log_frames - status.checkpointed_frames,
                                    status.log_frames,
                                    status.busy
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                log::debug!("reports: wal_checkpoint failed: {error}");
                            }
                        }
                    }
                }
                // Throughput diagnostics for a dense catch-up stream show whether the writer
                // keeps pace with input. The bounded channel prevents OOM either way.
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
            send_failed: Arc::new(AtomicBool::new(false)),
        },
        generation,
        immediate_commit_dirty,
        background_commit_dirty,
    })
}

/// Effects produced by one report-writer message.
struct ApplyEffect {
    /// Whether a catch-up page may be acknowledged after the surrounding commit.
    page_ack: bool,
    /// Report publication required after the surrounding commit, absent for maintenance.
    publication: Option<ReportPublication>,
}

impl ApplyEffect {
    /// Build an immediately published report mutation.
    ///
    /// Args:
    ///     page_ack: Whether the surrounding commit may acknowledge one catch-up page.
    ///
    /// Returns:
    ///     Immediate report effect with optional page acknowledgement.
    const fn immediate(page_ack: bool) -> Self {
        Self {
            page_ack,
            publication: Some(ReportPublication::Immediate),
        }
    }

    /// Build a background report mutation whose UI refresh may be coalesced.
    ///
    /// Args:
    ///     page_ack: Whether the surrounding commit may acknowledge one catch-up page.
    ///
    /// Returns:
    ///     Background report effect with optional page acknowledgement.
    const fn background(page_ack: bool) -> Self {
        Self {
            page_ack,
            publication: Some(ReportPublication::Background),
        }
    }

    /// Build an internal maintenance effect that must not wake report readers.
    ///
    /// Returns:
    ///     Non-report-changing effect.
    const fn maintenance() -> Self {
        Self {
            page_ack: false,
            publication: None,
        }
    }
}

/// Apply one typed-replica writer message and stage its durable valuation change.
///
/// Args:
///     conn: Active transaction receiving the message and valuation outbox mutation.
///     rep_state: Candidate replication state committed with the transaction.
///     msg: Typed replication or valuation-maintenance message to apply.
///
/// Returns:
///     Effect describing report publication and page acknowledgement after the surrounding
///     transaction commits.
fn apply_msg(
    conn: &Connection,
    rep_state: &mut rep::RepState,
    msg: &DbMsg,
) -> rusqlite::Result<ApplyEffect> {
    match msg {
        DbMsg::Schema { core_uid, schema } => {
            rep::apply_schema(conn, rep_state, *core_uid, schema.clone())?;
            Ok(ApplyEffect::background(false))
        }
        DbMsg::Upsert {
            core_uid,
            core_name,
            row,
        } => {
            rep::apply_upsert(conn, rep_state, *core_uid, core_name, row)?;
            valuation::stage_row(conn, valuation::TradeSource::Typed, *core_uid, row.rec_id)?;
            Ok(ApplyEffect::immediate(false))
        }
        DbMsg::Delete { core_uid, rec_id } => {
            rep::apply_delete(conn, *core_uid, *rec_id)?;
            valuation::stage_delete(conn, valuation::TradeSource::Typed, *core_uid, *rec_id)?;
            Ok(ApplyEffect::immediate(false))
        }
        DbMsg::SetDeleted { core_uid, change } => {
            rep::apply_set_deleted(conn, rep_state, *core_uid, change)?;
            Ok(ApplyEffect::immediate(false))
        }
        DbMsg::Page {
            core_uid,
            core_name,
            page,
            ..
        } => {
            rep::apply_page(conn, rep_state, *core_uid, core_name, page)?;
            if page.database_recreated {
                valuation::stage_rescan_core(conn, *core_uid)?;
            }
            for row in page.rows.iter() {
                valuation::stage_row(conn, valuation::TradeSource::Typed, *core_uid, row.rec_id)?;
            }
            Ok(ApplyEffect::background(true))
        }
        DbMsg::SyncComplete { core_uid, done: _ } => {
            rep::apply_sync_complete(conn, rep_state, *core_uid)?;
            valuation::stage_legacy_purge(conn, *core_uid)?;
            Ok(ApplyEffect::background(false))
        }
        DbMsg::ValuationAck { through_seq } => {
            valuation::ack_outbox(conn, *through_seq)?;
            Ok(ApplyEffect::maintenance())
        }
    }
}

// ============================================================================
//  Reads for the Reports window
// ============================================================================

/// Map a database path to a read outcome BEFORE opening it: a genuinely absent file becomes
/// `NotReady`, while any other metadata error becomes `Failed`.
///
/// NOT `Path::exists`: it reports false for a permission or metadata error too, which would
/// take the silent `NotReady` branch and tell the user the replica is merely not synced yet —
/// only a genuine absence may say that. The check still has to happen because
/// `Connection::open` would otherwise CREATE an empty database in place of the missing one.
/// `ctx` names the caller for the failure log. Shared by every path that opens a replica or
/// the strategy database read-only (the callers are this module and its descendants, so a
/// plain-private item reaches all of them without widening visibility to the crate root).
fn metadata_gate(path: &std::path::Path, ctx: &'static str) -> ReadResult<()> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ReadFail::NotReady),
        Err(e) => Err(read_fail::io_fail(ctx, &e)),
    }
}

/// Main report-schema page-cache budget for one reader connection, as SQLite's
/// negative `cache_size` form (KiB rather than pages, so it does not move with
/// `page_size`).
///
/// The default is ~2 MiB against a replica of hundreds of MB, so a reader starts
/// evicting pages inside a single period scan and re-reads them for the next
/// statement of the same batch — and one Analytics read is a batch: the summary
/// alone runs seven statements over the same rows.
///
/// The budget is per connection, and concurrent readers are not bounded. Peak
/// page-cache memory therefore scales as 16 MiB times the number of overlapping
/// reads; raising this constant raises that peak proportionally.
const READER_CACHE_KIB: i64 = -16_384;

/// Apply the shared tuning of a read-only report connection.
///
/// Both settings are advisory: a failure changes how fast the connection is, never
/// what it returns, so neither is allowed to fail opening a reader.
///
/// Intentionally omitted:
/// - `temp_store = MEMORY` would keep the sorter's temporary b-tree off disk, but
///   with unbounded concurrent readers it converts a successful file spill into an
///   out-of-memory kill — a behaviour change, which is exactly what this must not be.
/// - `mmap_size` would serve pages through a memory mapping, where truncation can
///   surface as an OS-level fault instead of an `SQLITE_IOERR`/`SQLITE_CORRUPT`
///   that [`read_fail`] can classify and present with repair guidance.
///
/// Args:
///     conn: Freshly opened read-only connection to the report replica.
fn tune_reader(conn: &Connection) {
    let _ = conn.busy_timeout(Duration::from_secs(3));
    let _ = conn.pragma_update(None, "cache_size", READER_CACHE_KIB);
}

/// Open a reader while distinguishing an absent replica from a SQLite failure.
///
/// A genuinely absent file maps to `NotReady`; metadata and SQLite open errors
/// map to `Failed` so callers cannot present them as an empty period. Access also
/// fails when this process does not own the recovery lease.
///
/// Returns:
///     Tuned report connection for the sole current process.
pub fn open_reader() -> ReadResult<Connection> {
    report_recovery::ensure_access().map_err(|error| ReadFail::Failed {
        kind: FailKind::Other,
        msg: Arc::from(error.to_string()),
    })?;
    let path = paths::reports_db_path();
    metadata_gate(&path, "отчёты(reader): доступ к файлу")?;
    let conn = Connection::open(&path).map_err(|e| read_fail("отчёты(reader)", e))?;
    // Before the ATTACH below: `cache_size` applies to one schema, and `main` is the
    // one every period scan reads.
    tune_reader(&conn);
    // The strategy database rides along on EVERY reader.
    //
    // `unified_from` is shared by Analytics readers, so attaching where the connection is born
    // keeps strategy-aware filtering consistent across all of them. It must happen before any
    // transaction is opened because SQLite does not allow ATTACH inside a transaction.
    analytics::attach_strategies(&conn);
    // Historical valuations are derived but correctness-sensitive: an existing unreadable cache
    // is a read failure, never silently interpreted as zero coverage. Startup initializes the file
    // before report-derived views can open readers; an absent file remains a normal not-ready state.
    let _ = valuation::attach(&conn)?;
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

/// Run a multi-query read inside one pinned SQLite snapshot and one pinned current-rate snapshot.
///
/// Args:
///     conn: Reader connection with every required database already attached.
///     read: Query batch that must observe one committed report generation and one rate snapshot.
///
/// Returns:
///     The batch result, or a classified snapshot/query failure.
pub(in crate::db) fn with_read_snapshot<T>(
    conn: &Connection,
    read: impl FnOnce(&Connection) -> ReadResult<T>,
) -> ReadResult<T> {
    // The rate snapshot is pinned alongside the row snapshot, for the same reason: everything one
    // batch shows the user must describe ONE state of the world. Without it a worker publication
    // landing mid-batch would convert the preflight and the scan at different rates.
    let _rates = valuation::pin_current_rates();
    let snapshot = read_snapshot(conn)?;
    read(&snapshot)
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

/// Upsert one `app_meta` key/value pair. The single place the shared settings table is written.
/// Plain-private: every caller is this module or a descendant (`rep`).
fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

pub fn save_sort(conn: &Connection, key: &str, desc: bool) {
    let _ = meta_set(conn, "sort_key", key);
    let _ = meta_set(conn, "sort_desc", if desc { "1" } else { "0" });
}

/// Storage key for the Report comment pane in one host.
///
/// The `:dock` / `:win` split MIRRORS `table_persist::ctx_id` in the UI crate, which keeps the
/// Report's visible columns and widths apart for a docked tab and a detached window. This
/// preference belongs to the same table, so it follows the same rule; the suffix is spelled out
/// here because `moon-core` cannot reach that helper.
fn comment_pane_key(detached: bool) -> &'static str {
    if detached {
        "report_comment_pane:win"
    } else {
        "report_comment_pane:dock"
    }
}

/// Load whether the Report shows the comment pane under its table, for one host.
///
/// Args:
///     conn: Open report metadata connection.
///     detached: Whether the caller is a detached or standalone Report window.
///
/// Returns:
///     The stored preference, or `None` when this host has never changed it. A value written by a
///     newer build, or a hand-edited row, counts as anything other than `"0"` being on — the pane
///     is a display choice, so an unreadable preference must not hide data.
pub fn load_comment_pane(conn: &Connection, detached: bool) -> Option<bool> {
    let value: String = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key=?1",
            [comment_pane_key(detached)],
            |r| r.get(0),
        )
        .ok()?;
    Some(value != "0")
}

/// Store whether the Report shows the comment pane under its table, for one host.
pub fn save_comment_pane(conn: &Connection, detached: bool, shown: bool) {
    let _ = meta_set(
        conn,
        comment_pane_key(detached),
        if shown { "1" } else { "0" },
    );
}

/// The key a save writes. Named rather than indexed, so appending a row cannot redirect the save.
const VISIBLE_KEY_CURRENT: &str = "report_visible_v3";

/// Every `app_meta` visible-column key with the columns introduced by its schema, newest first.
///
/// Saved sets are explicit, so reading a key also restores the columns declared by every newer
/// row encountered before it. Keeping each column on one schema row prevents the restoration
/// rules for older keys from drifting apart.
const VISIBLE_KEYS: &[(&str, &[&str])] = &[
    (VISIBLE_KEY_CURRENT, report_read::COLUMNS_ADDED_SINCE_V2),
    ("report_visible_v2", &[report_read::PROFIT_PERCENT_COLUMN]),
    ("report_visible", &[]),
];

/// Load visible Report columns, restoring any column added since the saved set was written.
///
/// Args:
///     conn: Open report metadata connection.
///
/// Returns:
///     Current or migrated column names, or `None` when no preference exists.
pub fn load_visible(conn: &Connection) -> Option<Vec<String>> {
    // Newest key first; everything passed on the way down was introduced after the key that
    // answers, and so is missing from the set it holds.
    let mut introduced_since: Vec<&str> = Vec::new();
    let stored = VISIBLE_KEYS.iter().find_map(|(key, introduced_by)| {
        let found = conn
            .query_row("SELECT value FROM app_meta WHERE key=?1", [*key], |r| {
                r.get::<_, String>(0)
            })
            .ok();
        if found.is_none() {
            introduced_since.extend_from_slice(introduced_by);
        }
        found
    })?;
    let mut columns: Vec<String> = stored
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    // Collected newest-schema-first; restore them oldest-first so the recovered set reads in the
    // order the columns were actually introduced.
    introduced_since.reverse();
    for column in introduced_since {
        if !columns.iter().any(|saved| saved == column) {
            columns.push(column.to_string());
        }
    }
    Some(columns)
}

/// Save visible Report columns under the current schema key.
///
/// Args:
///     conn: Open report metadata connection.
///     cols: Runtime column names in display order.
///
/// Returns:
///     Nothing; metadata write failures remain non-fatal like the other Report preferences.
pub fn save_visible(conn: &Connection, cols: &[&str]) {
    let _ = meta_set(conn, VISIBLE_KEY_CURRENT, &cols.join(","));
}

/// Report read source: a table, its columns, and a legacy flag.
///
/// Query the typed replica and the legacy table, while it exists, SEPARATELY so
/// each gets its own WHERE, ORDER, and LIMIT clauses and can use its indexes, then
/// merge in Rust. SQLite does not flatten a UNION ALL subquery with NULL projection,
/// so its filter was not pushed into the branches and every refresh fully scanned
/// the hundreds-of-MB legacy database (measured at about 400 ms for "Today").
/// During typed catch-up, a core can temporarily have rows in both tables until
/// `SyncComplete` purges its legacy rows. Readers merge both sources as-is, so any
/// overlap can temporarily contribute both copies to results.
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
    // An empty probe means the legacy table is absent. Omitting that source keeps
    // `query_reports` from generating a SELECT against a missing table.
    if !legacy_cols.is_empty() {
        out.push(ReadSource {
            table: "closed_sell_reports",
            cols: legacy_cols,
            legacy: true,
        });
    }
    Ok(out)
}

/// Open the replica strictly read-only, for a probe that must not own the file.
///
/// [`open_reader`] opens read-WRITE despite its name, which is harmless for its own callers
/// because the writer connection is alive by then. A probe that runs BEFORE `spawn_writer` would
/// instead be the only connection, and closing the last read-write connection makes SQLite run
/// its final WAL checkpoint and delete `-wal`/`-shm` — a WAL left by a killed run reaches
/// hundreds of MB, so that would land as a synchronous copy on the pre-window startup path. A
/// read-only last connection does not perform that close-time checkpoint or cleanup.
pub fn open_readonly() -> ReadResult<Connection> {
    let path = paths::reports_db_path();
    // Same reasoning as `open_reader`: only a genuine absence may report `NotReady`.
    metadata_gate(&path, "отчёты(ro): доступ к файлу")?;
    Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| read_fail("отчёты(ro)", e))
}

#[cfg(test)]
mod tests;
