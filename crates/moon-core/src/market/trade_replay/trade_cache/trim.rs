//! Cutting `trades.sqlite` down to what still claims it — the Storage tab's cleanup.
//!
//! A span knows its exchange, its market and its stretch of time, and nothing about the trade
//! it was fetched for: two trades on one market share the stretch between them, and a cluster
//! fetch or a neighbouring window may have filed a span no single trade asked for. So a cleanup
//! is not "delete this trade's prints" — it is a [`KeepMap`]: per `(exchange_key, market)`, the
//! union of the stretches the caller's set of trades still wants (their
//! [`super::super::ReplayWindow::focus_spans`] at the margin in force), built by the caller
//! from the report and from the live catalog. [`trim`] then clips every span to it: a span
//! outside the map's coverage goes whole, one that straddles an edge is cut to the pieces
//! inside, one inside stays as it is. Edges only — the stretch between two trades whose
//! stretches overlap is inside the union and is never touched.
//!
//! The same pass runs as a dry run for the tab's preview: it reads only the rows' bounds and
//! byte lengths, opens the blob of a straddling span alone, and writes nothing. The numbers it
//! reports are exact for prints and packed bytes; on disk the bytes come back only after the
//! `VACUUM` the apply pass ends with, which is why the tab words the size as approximate.

use std::collections::HashMap;

use super::super::coverage::Coverage;
use super::ROW_BYTES;

/// What to keep, per `(exchange_key, market)` as the file spells them; a key absent from the
/// map keeps nothing of that market.
pub type KeepMap = HashMap<(String, String), Coverage>;

/// What one pass counted — and, when it applied, did.
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

/// The file's markets and its time range.
pub(super) fn inventory(conn: &rusqlite::Connection) -> rusqlite::Result<Inventory> {
    let mut stmt = conn.prepare("SELECT DISTINCT exchange, market FROM spans ORDER BY 1, 2")?;
    let keys = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
    let range_ms: Option<(i64, i64)> =
        conn.query_row("SELECT MIN(from_ms), MAX(to_ms) FROM spans", [], |r| {
            Ok(
                match (r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?) {
                    (Some(from), Some(to)) => Some((from, to)),
                    _ => None,
                },
            )
        })?;
    Ok(Inventory { keys, range_ms })
}

/// One row's bounds, read without its blob.
struct RowMeta {
    rowid: i64,
    exchange: String,
    market: String,
    from_ms: i64,
    to_ms: i64,
    bytes: i64,
}

/// Clip every span to `keep`; with `apply`, rewrite the file to match, in one transaction.
///
/// Args:
///     conn: The open connection.
///     keep: What to keep, per market.
///     apply: `false` counts only; `true` deletes and rewrites.
///
/// Returns:
///     The counts — of what would go, or of what went.
pub(super) fn trim(
    conn: &rusqlite::Connection,
    keep: &KeepMap,
    apply: bool,
) -> rusqlite::Result<TrimReport> {
    let rows = {
        let mut stmt = conn
            .prepare("SELECT rowid, exchange, market, from_ms, to_ms, LENGTH(ticks) FROM spans")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(RowMeta {
                    rowid: r.get(0)?,
                    exchange: r.get(1)?,
                    market: r.get(2)?,
                    from_ms: r.get(3)?,
                    to_ms: r.get(4)?,
                    bytes: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let tx = if apply {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let mut report = TrimReport::default();
    for row in rows {
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
        if kept.is_empty() {
            report.spans_dropped += 1;
            report.bytes_dropped += row.bytes;
            report.prints_dropped += (row.bytes / ROW_BYTES as i64).max(0) as u64;
            if let Some(tx) = &tx {
                tx.execute("DELETE FROM spans WHERE rowid = ?1", [row.rowid])?;
            }
            continue;
        }
        let (blob, source, updated_ms): (Vec<u8>, i64, i64) = conn.query_row(
            "SELECT ticks, source, updated_ms FROM spans WHERE rowid = ?1",
            [row.rowid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        // The row goes first: the first piece usually starts where the row did, and the key
        // is `(exchange, market, from_ms)`.
        if let Some(tx) = &tx {
            tx.execute("DELETE FROM spans WHERE rowid = ?1", [row.rowid])?;
        }
        let mut kept_bytes = 0i64;
        for &(from_ms, to_ms) in kept.spans() {
            let piece = cut_blob(&blob, from_ms, to_ms);
            kept_bytes += piece.len() as i64;
            if let Some(tx) = &tx {
                tx.execute(
                    "INSERT INTO spans(exchange, market, from_ms, to_ms, ticks, source, updated_ms)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        row.exchange,
                        row.market,
                        from_ms,
                        to_ms,
                        piece,
                        source,
                        updated_ms
                    ],
                )?;
            }
        }
        let gone = (blob.len() as i64 - kept_bytes).max(0);
        report.spans_cut += 1;
        report.bytes_dropped += gone;
        report.prints_dropped += (gone / ROW_BYTES as i64) as u64;
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    Ok(report)
}

/// The packed prints of `blob` stamped inside `[from_ms, to_ms]`, in their stored order.
///
/// A linear pass, not a binary search: it runs only over the spans a cleanup actually cuts,
/// and it does not lean on the blob being sorted — `insert_span` sorts what it packs, but a
/// pass that would misfile a print on an unsorted row is not worth the microseconds.
fn cut_blob(blob: &[u8], from_ms: i64, to_ms: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(blob.len());
    for chunk in blob.chunks_exact(ROW_BYTES) {
        let time_ms = i64::from_le_bytes(chunk[0..8].try_into().expect("eight bytes"));
        if time_ms >= from_ms && time_ms <= to_ms {
            out.extend_from_slice(chunk);
        }
    }
    out
}

#[cfg(test)]
mod tests;
