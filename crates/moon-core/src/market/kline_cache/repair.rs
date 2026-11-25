//! One-time repair of the `chunks_v2` rows written while the core's quote volume was taken for
//! base coins.
//!
//! # What was wrong
//!
//! Until 2026-11-25 `crate::venue::deep_volume_is_quote` named Binance Futures (code 4) alone,
//! and the core's 5-minute ring took the same predicate. Every other venue's deep rows, and every
//! venue's ring rows but code 4's, were therefore stored with the core's turnover in the `volume`
//! slot and `quote_volume = volume * OHLC4` — the turnover times the price. Measured on that day:
//! Bybit BTCUSDT's deep 5-minute rows came to a 225 000 B $ day against 3.3 B from the candles the
//! terminal folds from trades, and the chart's volume scale went to billions while the bars
//! flattened to nothing. The fixed predicates stop new bad rows; this repairs the stored ones.
//!
//! # What is done, by kind
//!
//! The rule is the cache's own (module doc, "which producer wrote a row"): a stored row says
//! nothing about which producer wrote it, so it is rewritten only where ONE producer can have.
//!
//! | rows | producers | action |
//! |---|---|---|
//! | ring kind ([`RING_5M_KIND`]), every code but 4, unknown codes included | the ring recorder | rewritten in place |
//! | kinds 30 / 60 / 240 / 1440 of a venue the deep table now names, but 4 | the deep write-back | rewritten in place |
//! | kinds 1 and 5 of those venues | deep write-back AND trade-replay REST (1) or the trade recorder (5) | deleted |
//! | code 4, unknown codes' timeframe kinds, unparsable keys | — | left as they are |
//!
//! A rewritten row's stored `volume` IS the wire figure (that is what the old `false` stored), so
//! it becomes the quote and the base becomes its OHLC4 estimate — `split_wire_volume(.., true)`,
//! the conversion every fresh row now gets. No magnitude is consulted anywhere.
//!
//! Kinds 1 and 5 hold rows from two producers side by side, the bad deep ones and the right ones
//! folded from trades or fetched from the exchange's REST. They cannot be told apart by their
//! bytes: the one signature tried (`quote` bit-exactly equal to `volume * OHLC4`) also matches
//! about 47 000 right trade-folded rows of single-price buckets in the measured cache. So both
//! are deleted. What that costs: up to 30 days of one-minute and 15 of five-minute candles of
//! those venues. The core sends a chart's deep history again on its next open (about 10 000 rows,
//! some 35 days of 5-minute candles); the trade recorder refolds the last day from the core's
//! trade rings on its first pass; a deal replay fetches its one-minute bars from the exchange
//! again. What does not come back: trade-folded 5-minute candles older than a day, of markets no
//! chart reopens, until the recorder has run that long again.
//!
//! # Once, and what an older build does
//!
//! Applied in ONE transaction together with its mark in the `repairs` table, so a failure leaves
//! both undone and the next open tries again, and a success is never applied twice — a second
//! rewrite would read a fixed row's estimated base as turnover. The mark is a table of its own, not
//! `PRAGMA user_version`: an older build sets that back to 2 on every open (`init_schema`), and
//! would make a newer one repeat the rewrite. The same older build, run after the repair, writes
//! bad rows again; those are not repaired, and age out with their kind's retention.

use rusqlite::{Connection, OptionalExtension, params};

use super::{RING_5M_KIND, ROW_BYTES_V2};
use crate::market::candles::split_wire_volume;

/// This repair's row in the `repairs` table.
const NAME: &str = "2026-11-25 core candle volume is quote";

/// The one platform code whose rows were already stored with the quote unit before this repair:
/// the only `true` of `crate::venue::deep_volume_is_quote` until 2026-11-25, for its deep rows and
/// its ring rows alike.
const QUOTE_BEFORE_REPAIR: u8 = 4;

/// The platform codes whose deep rows `crate::venue::deep_volume_is_quote` turned to quote on
/// 2026-11-25 — every code that day's [`crate::venue::venue`] identified, but 4.
///
/// Frozen here rather than read from the live predicate: a later build that names one more venue
/// quote, by its own measurement, must not reach back into a cache this repair never saw and
/// rewrite that venue's rows written before — they were right for the rule of their day.
fn deep_turned_quote(code: u8) -> bool {
    matches!(code, 2..=15) && code != QUOTE_BEFORE_REPAIR
}

/// The coarse kinds the deep write-back alone produces.
const COARSE_KINDS: [u32; 4] = [30, 60, 240, 1440];

/// What the repair does to the rows of one exchange key and kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Written in the right unit, or by a producer this repair cannot speak for.
    Keep,
    /// One producer, the wrong unit: the stored `volume` becomes the quote.
    Reinterpret,
    /// Two producers, one of them wrong, indistinguishable by row: removed.
    Delete,
}

/// The action for the `chunks_v2` rows stored under `exchange` (`"{code}:{dex}"`) at `kind` —
/// the module doc's table.
fn action(exchange: &str, kind: u32) -> Action {
    let Some(code) = exchange
        .split(':')
        .next()
        .and_then(|s| s.parse::<u8>().ok())
    else {
        return Action::Keep;
    };
    if code == QUOTE_BEFORE_REPAIR {
        return Action::Keep;
    }
    if kind == RING_5M_KIND {
        return Action::Reinterpret;
    }
    if !deep_turned_quote(code) {
        return Action::Keep;
    }
    match kind {
        1 | 5 => Action::Delete,
        k if COARSE_KINDS.contains(&k) => Action::Reinterpret,
        _ => Action::Keep,
    }
}

/// One `chunks_v2` blob with every row's stored `volume` taken as the quote it really was.
///
/// Prices and offsets are copied unchanged; the base becomes the OHLC4 estimate of that quote.
/// Trailing bytes short of a whole row, which no reader decodes, are copied as they are.
fn reinterpret_blob(blob: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(blob.len());
    let mut rows = blob.chunks_exact(ROW_BYTES_V2);
    for row in rows.by_ref() {
        let f = |i: usize| f32::from_le_bytes([row[i], row[i + 1], row[i + 2], row[i + 3]]);
        let (open, high, low, close, wire) = (f(4), f(8), f(12), f(16), f(20));
        let (base, quote) = split_wire_volume(wire, open, high, low, close, true);
        out.extend_from_slice(&row[..20]);
        out.extend_from_slice(&base.to_le_bytes());
        out.extend_from_slice(&quote.to_le_bytes());
    }
    out.extend_from_slice(rows.remainder());
    out
}

/// What one application of the repair changed, for its log line and its tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Report {
    /// Daily chunks removed (kinds 1 and 5).
    pub deleted_chunks: usize,
    /// Daily chunks rewritten in place (ring and coarse kinds).
    pub rewritten_chunks: usize,
}

/// Apply the repair unless its mark is already in the file, logging what it did.
///
/// Never fails the cache: an error is logged, the transaction rolls back, and the next open tries
/// again.
///
/// Returns:
///     Whether the cache may be written this session: `true` when the repair is in the file
///     (applied now or before), `false` when it failed — the caller then keeps the cache
///     read-only, because a fresh row merged into a day chunk beside unrepaired ones would be
///     rewritten into a wrong one by the next open's retry.
pub(super) fn apply_once(conn: &Connection) -> bool {
    let started = std::time::Instant::now();
    match apply(conn) {
        Ok(Some(report)) => {
            log::info!(
                "kline cache repair '{NAME}': deleted {} chunks of kinds 1/5, rewrote {} chunks, {} ms",
                report.deleted_chunks,
                report.rewritten_chunks,
                started.elapsed().as_millis()
            );
            true
        }
        Ok(None) => true,
        Err(e) => {
            log::warn!(
                "kline cache repair '{NAME}' failed; the cache is read-only this session and the repair is retried on the next open: {e}"
            );
            false
        }
    }
}

/// The repair itself: `None` when it was applied before, otherwise what it changed.
///
/// The mark is looked for twice. First without any lock, so a cache repaired before — every open
/// but the first — never contends for the write lock and never turns read-only because another
/// process holds it. Then again inside a transaction that takes the write lock FIRST
/// (`IMMEDIATE`), so two terminals opening one unrepaired file at once cannot both find it absent
/// and both rewrite: the second waits out rusqlite's default 5-second busy timeout, then finds the
/// first's mark, or fails and stays read-only for its session.
pub(super) fn apply(conn: &Connection) -> rusqlite::Result<Option<Report>> {
    let marked = |c: &Connection| -> rusqlite::Result<bool> {
        Ok(
            c.query_row("SELECT 1 FROM repairs WHERE name = ?1", [NAME], |_| Ok(()))
                .optional()?
                .is_some(),
        )
    };
    let has_table = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'repairs'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if has_table && marked(conn)? {
        return Ok(None);
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS repairs(
            name TEXT PRIMARY KEY,
            applied_ms INTEGER NOT NULL
        );",
    )?;
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if marked(&tx)? {
        return Ok(None);
    }
    let exchanges: Vec<String> = {
        let mut stmt = tx.prepare("SELECT DISTINCT exchange FROM chunks_v2")?;
        let listed = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        listed
    };
    let mut report = Report::default();
    for exchange in &exchanges {
        for kind in [1, 5] {
            if action(exchange, kind) == Action::Delete {
                report.deleted_chunks += tx.execute(
                    "DELETE FROM chunks_v2 WHERE exchange = ?1 AND kind = ?2",
                    params![exchange, kind],
                )?;
            }
        }
        for kind in COARSE_KINDS.into_iter().chain([RING_5M_KIND]) {
            if action(exchange, kind) == Action::Reinterpret {
                report.rewritten_chunks += rewrite_kind(&tx, exchange, kind)?;
            }
        }
    }
    tx.execute(
        "INSERT INTO repairs(name, applied_ms) VALUES(?1, ?2)",
        params![NAME, super::now_unix_ms()],
    )?;
    tx.commit()?;
    Ok(Some(report))
}

/// Rewrite every daily chunk of one exchange key and kind with [`reinterpret_blob`], returning
/// how many there were.
///
/// The keys are listed first and each chunk is then read and written by its primary key, so no
/// statement walks the table while another one updates it.
fn rewrite_kind(conn: &Connection, exchange: &str, kind: u32) -> rusqlite::Result<usize> {
    let keys: Vec<(String, i64)> = {
        let mut stmt =
            conn.prepare("SELECT market, day FROM chunks_v2 WHERE exchange = ?1 AND kind = ?2")?;
        let listed = stmt
            .query_map(params![exchange, kind], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        listed
    };
    let mut get = conn.prepare_cached(
        "SELECT rows FROM chunks_v2 WHERE exchange = ?1 AND market = ?2 AND kind = ?3 AND day = ?4",
    )?;
    let mut put = conn.prepare_cached(
        "UPDATE chunks_v2 SET rows = ?5 WHERE exchange = ?1 AND market = ?2 AND kind = ?3 AND day = ?4",
    )?;
    for (market, day) in &keys {
        let blob: Vec<u8> = get.query_row(params![exchange, market, kind, day], |r| r.get(0))?;
        put.execute(params![
            exchange,
            market,
            kind,
            day,
            reinterpret_blob(&blob)
        ])?;
    }
    Ok(keys.len())
}

#[cfg(test)]
mod tests;
