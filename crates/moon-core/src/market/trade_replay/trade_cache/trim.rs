//! Cutting `trades.sqlite` down to what still claims it — the Storage tab's cleanup.
//!
//! A span knows its exchange, its market and its stretch of time, and nothing about the trade
//! it was fetched for: two trades on one market share the stretch between them, and a cluster
//! fetch or a neighbouring window may have filed a span no single trade asked for. So a cleanup
//! is not "delete this trade's prints" — it is a [`KeepMap`]: per `(exchange_key, market)`, the
//! union of the stretches the caller's set of trades still wants (their
//! [`super::super::ReplayWindow::focus_spans`] at the margin in force), built by the caller
//! from the report and from the live catalog. [`trim_progress`] clips every span to it: a span
//! outside the map's coverage goes whole, one that straddles an edge is cut to the pieces
//! inside, one inside stays as it is. Edges only — the stretch between two trades whose
//! stretches overlap is inside the union and is never touched.
//!
//! Both tables are clipped atomically ([`super`]'s "Layout"); cut pieces are written packed.
//! Compaction after the delete releases their unused disk pages separately.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::coverage::Coverage;
use super::{Table, codec};
use crate::feed::types::Tick;

/// What to keep, per `(exchange_key, market)` as the file spells them; a key absent from the
/// map keeps nothing of that market.
pub type KeepMap = HashMap<(String, String), Coverage>;

/// Compaction policy after a committed retention delete.
#[derive(Clone, Copy, Debug)]
pub enum Compact {
    Incremental,
    Full,
}

/// A committed delete can succeed even when releasing its disk pages must wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactFail {
    NoRoom,
    NotShrinkable,
    Deferred,
    Other,
}

/// Shared counters read without waiting for the cache worker.
#[derive(Default)]
pub struct TrimProgress {
    pub done: AtomicU64,
    pub total: AtomicU64,
}

/// Durable deletion and its separately reported physical compaction outcome.
#[derive(Debug)]
pub struct TrimOutcome {
    pub report: TrimReport,
    pub compacted: Result<(), CompactFail>,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

/// Read main and WAL lengths; zero means an in-memory or otherwise unknown file size.
pub(super) fn disk_bytes(conn: &rusqlite::Connection) -> u64 {
    let Some(path) = conn.path().filter(|path| !path.is_empty()) else {
        return 0;
    };
    [path.to_string(), format!("{path}-wal")]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// Release pages after commit; checkpoint contention is a retryable outcome.
pub(super) fn compact(conn: &rusqlite::Connection, policy: Compact) -> Result<(), CompactFail> {
    match policy {
        Compact::Full => {
            if !full_vacuum_has_room(conn) {
                return Err(CompactFail::NoRoom);
            }
            conn.execute_batch("VACUUM").map_err(compact_failure)?;
        }
        Compact::Incremental => incremental_vacuum(conn, "PRAGMA incremental_vacuum")?,
    }
    checkpoint(conn)
}

/// Release a bounded batch of pages; the cache queue may serve live work before the next step.
pub(super) fn compact_step(conn: &rusqlite::Connection) -> Result<bool, CompactFail> {
    incremental_vacuum(conn, "PRAGMA incremental_vacuum(128)")?;
    let remaining: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .map_err(compact_failure)?;
    if remaining != 0 {
        Ok(true)
    } else {
        checkpoint(conn).map(|()| false)
    }
}

/// Drain incremental-vacuum rows so both bounded and complete passes actually release pages.
fn incremental_vacuum(conn: &rusqlite::Connection, pragma: &str) -> Result<(), CompactFail> {
    let mode: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .map_err(|_| CompactFail::Other)?;
    if mode != 2 {
        return Err(CompactFail::NotShrinkable);
    }
    let vacuum = || -> rusqlite::Result<()> {
        let mut stmt = conn.prepare(pragma)?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
        Ok(())
    };
    vacuum().map_err(compact_failure)
}

/// Preserve the physical-compaction error categories shared by both vacuum paths.
fn compact_failure(error: rusqlite::Error) -> CompactFail {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DiskFull) => CompactFail::NoRoom,
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            CompactFail::Deferred
        }
        _ => CompactFail::Other,
    }
}

/// A busy checkpoint means deletion committed but physical compaction still needs a retry.
fn checkpoint(conn: &rusqlite::Connection) -> Result<(), CompactFail> {
    let busy: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
        .map_err(|_| CompactFail::Other)?;
    if busy == 0 {
        Ok(())
    } else {
        Err(CompactFail::Deferred)
    }
}

/// Counts of the spans, prints and packed bytes a committed retention pass removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrimReport {
    /// Spans the file held before the pass.
    pub spans_total: u64,
    /// Packed bytes the file held before the pass.
    pub bytes_total: i64,
    /// Spans removed whole.
    pub spans_dropped: u64,
    /// Spans cut down to the pieces inside the map.
    pub spans_cut: u64,
    /// Prints removed, whole spans and cut edges together.
    pub prints_dropped: u64,
    /// Packed bytes removed, whole spans and cut edges together.
    pub bytes_dropped: i64,
}

impl TrimReport {
    /// Whether the pass found nothing to remove.
    pub fn is_empty(&self) -> bool {
        self.spans_dropped == 0 && self.spans_cut == 0
    }
}

/// What the file holds, for the caller building a [`KeepMap`]: which markets, and how far in
/// time — so it reads only the report rows that can claim any of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    /// Every distinct `(exchange_key, market)` with at least one span.
    pub keys: Vec<(String, String)>,
    /// Earliest `from_ms` and latest `to_ms` over every span, or `None` for an empty file.
    pub range_ms: Option<(i64, i64)>,
}

/// The file's markets and its time range, over both tables.
pub(super) fn inventory(conn: &rusqlite::Connection) -> rusqlite::Result<Inventory> {
    let mut stmt = conn.prepare(
        "SELECT exchange, market FROM spans UNION SELECT exchange, market FROM packs ORDER BY 1, 2",
    )?;
    let keys = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
    let range_ms: Option<(i64, i64)> = conn.query_row(
        "SELECT MIN(f), MAX(t) FROM (
             SELECT from_ms AS f, to_ms AS t FROM spans
             UNION ALL SELECT from_ms, to_ms FROM packs
         )",
        [],
        |r| {
            Ok(
                match (r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?) {
                    (Some(from), Some(to)) => Some((from, to)),
                    _ => None,
                },
            )
        },
    )?;
    Ok(Inventory { keys, range_ms })
}

/// One row's bounds, read without its blob.
struct RowMeta {
    table: Table,
    rowid: i64,
    exchange: String,
    market: String,
    from_ms: i64,
    to_ms: i64,
    bytes: i64,
    prints: i64,
}

/// Clip rows atomically while publishing progress for a non-blocking caller.
pub(super) fn trim_progress(
    conn: &rusqlite::Connection,
    keep: &KeepMap,
    progress: Option<&TrimProgress>,
) -> rusqlite::Result<TrimReport> {
    let mut rows = Vec::new();
    for table in Table::ALL {
        let mut stmt = conn.prepare(&format!(
            "SELECT rowid, exchange, market, from_ms, to_ms, LENGTH(ticks), {} FROM {}",
            table.prints_sql(),
            table.name()
        ))?;
        let found = stmt
            .query_map([], |r| {
                Ok(RowMeta {
                    table,
                    rowid: r.get(0)?,
                    exchange: r.get(1)?,
                    market: r.get(2)?,
                    from_ms: r.get(3)?,
                    to_ms: r.get(4)?,
                    bytes: r.get(5)?,
                    prints: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.extend(found);
    }
    if let Some(p) = progress {
        p.total.store(rows.len() as u64, Ordering::Relaxed);
    }
    let tx = conn.unchecked_transaction()?;
    let mut report = TrimReport::default();
    for row in rows {
        if let Some(p) = progress {
            p.done.fetch_add(1, Ordering::Relaxed);
        }
        report.spans_total += 1;
        report.bytes_total += row.bytes;
        let own = Coverage::one((row.from_ms, row.to_ms));
        let kept = keep
            .get(&(row.exchange.clone(), row.market.clone()))
            .map(|wanted| own.clip(wanted))
            .unwrap_or_default();
        if kept == own {
            continue;
        }
        let delete = format!("DELETE FROM {} WHERE rowid = ?1", row.table.name());
        let whole = |report: &mut TrimReport| {
            report.spans_dropped += 1;
            report.bytes_dropped += row.bytes;
            report.prints_dropped += row.prints.max(0) as u64;
        };
        if kept.is_empty() {
            whole(&mut report);
            tx.prepare_cached(&delete)?.execute([row.rowid])?;
            continue;
        }
        let (blob, source, updated_ms): (Vec<u8>, i64, i64) = conn
            .prepare_cached(&format!(
                "SELECT ticks, source, updated_ms FROM {} WHERE rowid = ?1",
                row.table.name()
            ))?
            .query_row([row.rowid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        // The row goes first: the first piece usually starts where the row did, and the key
        // is `(exchange, market, from_ms)`.
        tx.prepare_cached(&delete)?.execute([row.rowid])?;
        let ticks = match row.table.decode(&blob) {
            Ok(ticks) => ticks,
            Err(e) => {
                // Unreadable is unkeepable: it goes whole, and the next request refetches.
                log::warn!(
                    "trade cache cleanup: a {} span of {}/{} does not decode ({e}), dropped whole",
                    row.table.name(),
                    row.exchange,
                    row.market
                );
                whole(&mut report);
                continue;
            }
        };
        let mut kept_bytes = 0i64;
        let mut kept_prints = 0i64;
        for &(from_ms, to_ms) in kept.spans() {
            let inside = ticks_within(&ticks, from_ms, to_ms);
            kept_prints += inside.len() as i64;
            let piece = codec::encode(&inside);
            kept_bytes += piece.len() as i64;
            tx.prepare_cached(
                "INSERT INTO packs(exchange, market, from_ms, to_ms, ticks, prints, source, updated_ms)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")?.execute(
                rusqlite::params![
                    row.exchange,
                    row.market,
                    from_ms,
                    to_ms,
                    piece,
                    inside.len() as i64,
                    source,
                    updated_ms
                ],
            )?;
        }
        report.spans_cut += 1;
        report.bytes_dropped += (row.bytes - kept_bytes).max(0);
        report.prints_dropped += (ticks.len() as i64 - kept_prints).max(0) as u64;
    }
    tx.commit()?;
    Ok(report)
}

/// The prints of `ticks` stamped inside `[from_ms, to_ms]`, in their stored order.
///
/// A linear pass, not a binary search: it runs only over the spans a cleanup actually cuts,
/// and it does not lean on the prints being sorted — `insert_span` sorts what it packs, but a
/// pass that would misfile a print on an unsorted row is not worth the microseconds.
fn ticks_within(ticks: &[Tick], from_ms: i64, to_ms: i64) -> Vec<Tick> {
    ticks
        .iter()
        .copied()
        .filter(|t| {
            let time_ms = t.time_ms as i64;
            time_ms >= from_ms && time_ms <= to_ms
        })
        .collect()
}

#[cfg(test)]
mod tests;

/// Apply the shared two-copy VACUUM budget to file databases; memory databases need no disk.
fn full_vacuum_has_room(conn: &rusqlite::Connection) -> bool {
    let Some(path) = conn
        .path()
        .filter(|path| !path.is_empty())
        .map(std::path::Path::new)
    else {
        return true;
    };
    crate::disk_reserve::vacuum_has_room(path)
}
