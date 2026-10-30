//! `market_prices.sqlite`: the filed segments, their retention, and the one read function.
//!
//! A row is one segment of one `(exchange, market)`: the stretch `[from_ms, to_ms]` the lists
//! vouch for, and its points ([`super::codec`]). Two segments that abut (`to_ms + 1 == from_ms`)
//! are one unbroken stretch; ground between segments is a gap — the source was not listening —
//! and a reader must not draw across it.
//!
//! Its own file, not a table beside the tape: the tape lives in `trades.sqlite` on a terminal
//! and in `tape_recorder.sqlite` on a station, while this recorder is one module on both, and its
//! writer must not queue behind the tape's.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};

use super::codec;
use super::ring::Segment;
use crate::market::trade_replay::Coverage;

/// A rowid table, not one clustered by its key: the recorder inserts segments in time order, so
/// the oldest rows sit on the oldest pages, and a size trim by end time frees whole pages.
/// Clustered by `(exchange, market, from_ms)`, the oldest rows would be scattered over every
/// market's pages and a size trim would free almost none of them. Segments pulled from a station
/// are old but inserted late: they sit together on later pages, and the trim, by end time and not
/// by rowid, takes them first all the same.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS segments (
    exchange TEXT NOT NULL,
    market TEXT NOT NULL,
    from_ms INTEGER NOT NULL,
    to_ms INTEGER NOT NULL,
    data BLOB NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS segments_key ON segments (exchange, market, from_ms);
CREATE INDEX IF NOT EXISTS segments_to ON segments (to_ms);
";

/// Most rows one size pass deletes at once, so a pass never holds the writer for long.
const MAX_TRIM_ROWS: u64 = 20_000;
/// Size passes per trim. Index pages free less readily than table pages, so a pass can fall a
/// little short; the next one finishes, and an overshoot stays within one pass's estimate.
const SIZE_PASSES: usize = 4;

/// A segment already packed, on its way from the station to this file.
pub(super) struct Packed {
    pub exchange: String,
    pub market: String,
    pub from_ms: i64,
    pub to_ms: i64,
    pub data: Vec<u8>,
}

/// One segment on its way to the file.
pub(super) struct Filed {
    pub exchange: String,
    pub market: String,
    pub segment: Segment,
}

/// The writer's connection, owned by the recorder thread.
pub(super) struct Store {
    conn: Connection,
}

impl Store {
    /// Open or create the file at `path`.
    pub(super) fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// An in-memory store, for tests.
    #[cfg(test)]
    pub(super) fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// The connection, for tests that read back what was filed.
    #[cfg(test)]
    pub(super) fn conn(&self) -> &Connection {
        &self.conn
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        crate::db::wal::enable(&conn)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// File `segments` in one transaction. Returns the bytes of point data written.
    pub(super) fn write(&mut self, segments: &[Filed]) -> rusqlite::Result<usize> {
        if segments.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let mut bytes = 0;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO segments (exchange, market, from_ms, to_ms, data)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for f in segments {
                let data = codec::encode(&f.segment);
                bytes += data.len();
                stmt.execute(params![
                    f.exchange,
                    f.market,
                    f.segment.from_ms,
                    f.segment.to_ms,
                    data
                ])?;
            }
        }
        tx.commit()?;
        Ok(bytes)
    }

    /// File packed segments in one transaction, each only where the file holds nothing of its
    /// market over its stretch: a reader merges segments in time order and must never meet two
    /// that speak for the same moment, and what this host recorded itself stays. Returns the
    /// segments filed.
    pub(super) fn insert_packed(&mut self, segments: &[Packed]) -> rusqlite::Result<usize> {
        if segments.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let mut filed = 0;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO segments (exchange, market, from_ms, to_ms, data)
                 SELECT ?1, ?2, ?3, ?4, ?5
                 WHERE NOT EXISTS (SELECT 1 FROM segments
                     WHERE exchange = ?1 AND market = ?2 AND from_ms <= ?4 AND to_ms >= ?3)",
            )?;
            for p in segments {
                filed += stmt.execute(params![p.exchange, p.market, p.from_ms, p.to_ms, p.data])?;
            }
        }
        tx.commit()?;
        Ok(filed)
    }

    /// Delete the segments that ended before `edge_ms`, then the oldest until what the file holds
    /// fits `max_bytes`. Freed pages are reused by later writes, so the file stops growing at the
    /// ceiling rather than shrinking below it. Returns the rows deleted.
    pub(super) fn trim(&mut self, edge_ms: Option<i64>, max_bytes: u64) -> rusqlite::Result<usize> {
        let mut deleted = 0;
        if let Some(edge) = edge_ms {
            deleted += self
                .conn
                .execute("DELETE FROM segments WHERE to_ms < ?1", [edge])?;
        }
        for _ in 0..SIZE_PASSES {
            let used = self.used_bytes()?;
            if used <= max_bytes {
                break;
            }
            let rows: i64 = self
                .conn
                .query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0))?;
            let Ok(rows) = u64::try_from(rows) else {
                break;
            };
            if rows == 0 {
                break;
            }
            let per_row = (used / rows).max(1);
            let n = ((used - max_bytes) / per_row + 1)
                .min(MAX_TRIM_ROWS)
                .min(rows);
            // By end time, not rowid: a station's old segments are filed after this host's new
            // ones. Segments flushed together share an end, so a pass may take a few more.
            let cut: i64 = self.conn.query_row(
                "SELECT to_ms FROM segments ORDER BY to_ms LIMIT 1 OFFSET ?1",
                [i64::try_from(n - 1).unwrap_or(0)],
                |r| r.get(0),
            )?;
            deleted += self
                .conn
                .execute("DELETE FROM segments WHERE to_ms <= ?1", [cut])?;
        }
        Ok(deleted)
    }

    /// Drop of `market` on `exchange` what falls in the interior gaps of `keep` — ground the tape
    /// keeps on both sides of, not ground before its first kept stretch, which the age trim owns:
    /// a segment wholly inside a gap goes, one across a gap's edge is replaced by its parts
    /// outside it ([`codec::cut`]). Each gap is one index seek, bounded below by `max_segment_ms`
    /// (no segment is longer), so a pass costs its gaps, not the file. Returns the segments
    /// removed or cut.
    pub(super) fn keep_only(
        &mut self,
        exchange: &str,
        market: &str,
        keep: &Coverage,
        max_segment_ms: i64,
    ) -> rusqlite::Result<usize> {
        let Some(hull) = keep.hull() else {
            return Ok(0);
        };
        let gaps = Coverage::one(hull).minus(keep);
        if gaps.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let mut changed = 0;
        {
            let mut bounds: Vec<(i64, i64, i64)> = Vec::new();
            {
                let mut stmt = tx.prepare_cached(
                    "SELECT rowid, from_ms, to_ms FROM segments
                     WHERE exchange = ?1 AND market = ?2 AND from_ms >= ?3 AND from_ms <= ?4
                       AND to_ms >= ?5",
                )?;
                for &(a, b) in gaps.spans() {
                    let rows = stmt.query_map(
                        params![exchange, market, a.saturating_sub(max_segment_ms), b, a],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )?;
                    for row in rows {
                        bounds.push(row?);
                    }
                }
            }
            // A segment across two gaps is met twice; it is handled once.
            bounds.sort_unstable();
            bounds.dedup();
            let mut data = tx.prepare_cached("SELECT data FROM segments WHERE rowid = ?1")?;
            let mut delete = tx.prepare_cached("DELETE FROM segments WHERE rowid = ?1")?;
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO segments (exchange, market, from_ms, to_ms, data)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (rowid, from, to) in bounds {
                if keep.contains((from, to)) {
                    continue;
                }
                let kept = Coverage::one((from, to)).clip(keep);
                let blob: Option<Vec<u8>> = if kept.is_empty() {
                    None
                } else {
                    Some(data.query_row([rowid], |r| r.get(0))?)
                };
                delete.execute([rowid])?;
                changed += 1;
                let Some(blob) = blob else {
                    continue;
                };
                for &(a, b) in kept.spans() {
                    // A part that does not decode is dropped with the segment it came from.
                    if let Ok(Some((start, bytes))) = codec::cut(from, &blob, a, b) {
                        insert.execute(params![exchange, market, start, b, bytes])?;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Bytes of the file's pages, free ones included: its size on disk, the WAL aside.
    pub(super) fn file_bytes(&self) -> rusqlite::Result<u64> {
        let pragma = |name: &str| -> rusqlite::Result<i64> {
            self.conn
                .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
        };
        let file = pragma("page_count")?.max(0) * pragma("page_size")?;
        Ok(u64::try_from(file).unwrap_or(0))
    }

    /// Bytes of pages in use: the file minus its free list.
    fn used_bytes(&self) -> rusqlite::Result<u64> {
        let pragma = |name: &str| -> rusqlite::Result<i64> {
            self.conn
                .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
        };
        let used =
            (pragma("page_count")? - pragma("freelist_count")?).max(0) * pragma("page_size")?;
        Ok(u64::try_from(used).unwrap_or(0))
    }
}

/// Funding as the list carried it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FundingRate {
    /// The rate, as the core sends it (`0.0001` = 0.01 %).
    pub rate: f64,
    /// The next charge, the core's wire time as Unix milliseconds.
    pub next_ms: i64,
}

/// The market's prices from `t_ms` until the next sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PriceSample {
    pub t_ms: i64,
    pub bid: f64,
    pub ask: f64,
    pub mark: f64,
    /// `None` until a list carried funding for the market.
    pub funding: Option<FundingRate>,
}

/// One unbroken stretch: the samples in it hold from `from_ms` through `to_ms`, and nothing is
/// known of the market just outside it.
#[derive(Clone, Debug, PartialEq)]
pub struct PriceSpan {
    pub from_ms: i64,
    pub to_ms: i64,
    pub samples: Vec<PriceSample>,
}

/// What the file holds of `market` on `exchange_key` over `[from_ms, to_ms]`, as unbroken
/// stretches in time order. Whole segments are returned, so the first and the last stretch may
/// reach a few seconds past the bounds asked. An empty answer when there is no file yet.
///
/// Opens a read-only connection per call; call it off the UI thread.
pub fn read(
    exchange_key: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<PriceSpan>> {
    read_at(
        &crate::config::paths::market_prices_db_path(),
        exchange_key,
        market,
        from_ms,
        to_ms,
    )
}

/// [`read`] against the file at `path`.
pub(super) fn read_at(
    path: &Path,
    exchange_key: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<PriceSpan>> {
    match open_read(path)? {
        Some(conn) => read_conn(&conn, exchange_key, market, from_ms, to_ms),
        None => Ok(Vec::new()),
    }
}

/// A read-only connection to the file at `path`; `None` when there is no file yet, which a read
/// never creates.
pub(super) fn open_read(path: &Path) -> rusqlite::Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(3))?;
    Ok(Some(conn))
}

/// The bounds of the segments filed for `market` on `exchange_key` that touch
/// `[from_ms, to_ms]`, in time order — whether they decode or not.
pub(super) fn held_bounds(
    conn: &Connection,
    exchange_key: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT from_ms, to_ms FROM segments
         WHERE exchange = ?1 AND market = ?2 AND from_ms <= ?4 AND to_ms >= ?3
         ORDER BY from_ms",
    )?;
    let bounds = stmt
        .query_map(params![exchange_key, market, from_ms, to_ms], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect();
    bounds
}

/// [`read`] over an open connection.
pub(super) fn read_conn(
    conn: &Connection,
    exchange_key: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<PriceSpan>> {
    let mut stmt = conn.prepare_cached(
        "SELECT from_ms, to_ms, data FROM segments
         WHERE exchange = ?1 AND market = ?2 AND from_ms <= ?4 AND to_ms >= ?3
         ORDER BY from_ms",
    )?;
    let mut rows = stmt.query(params![exchange_key, market, from_ms, to_ms])?;
    let mut spans: Vec<PriceSpan> = Vec::new();
    // A segment that cannot be read breaks the stretch, like a gap: nothing may be drawn across it.
    let mut broken = false;
    while let Some(row) = rows.next()? {
        let seg_from: i64 = row.get(0)?;
        let seg_to: i64 = row.get(1)?;
        let data: Vec<u8> = row.get(2)?;
        let decoded = match codec::decode(seg_from, &data) {
            Ok(decoded) => decoded,
            Err(e) => {
                log::warn!(
                    "price recorder: unreadable segment {exchange_key} {market} at {seg_from}: {e:?}"
                );
                broken = true;
                continue;
            }
        };
        let samples = decoded.into_iter().map(|d| PriceSample {
            t_ms: d.t_ms,
            bid: d.bid,
            ask: d.ask,
            mark: d.mark,
            funding: d
                .funding
                .map(|(rate, next_ms)| FundingRate { rate, next_ms }),
        });
        match spans.last_mut() {
            Some(last) if !broken && seg_from <= last.to_ms.saturating_add(1) => {
                last.to_ms = last.to_ms.max(seg_to);
                // Each segment opens with the state it starts from: the previous one's last sample
                // again, unless something changed right at the seam.
                let mut samples = samples.peekable();
                let repeats = match (last.samples.last(), samples.peek()) {
                    (Some(prev), Some(first)) => {
                        prev.bid.to_bits() == first.bid.to_bits()
                            && prev.ask.to_bits() == first.ask.to_bits()
                            && prev.mark.to_bits() == first.mark.to_bits()
                            && prev.funding == first.funding
                    }
                    _ => false,
                };
                if repeats {
                    samples.next();
                }
                last.samples.extend(samples);
            }
            _ => spans.push(PriceSpan {
                from_ms: seg_from,
                to_ms: seg_to,
                samples: samples.collect(),
            }),
        }
        broken = false;
    }
    Ok(spans)
}

#[cfg(test)]
mod tests;
