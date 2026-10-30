//! The price pull (STATION.md §4.9): the station lists and answers what its file holds, the
//! terminal works out what its own file lacks and files the station's answer beside its own.
//!
//! The station's file holds prices only around trades, so its listing is already the set of
//! trade windows; the terminal needs no trade list, catalog or core clock of its own to know what
//! to ask. What the terminal recorded itself is never replaced: it asks only for ground its file
//! does not hold, and files a piece only where its file still holds nothing of that stretch.
//!
//! Both sides read the file through a connection of their own. The terminal writes through one
//! too, beside the recorder's writer — WAL serialises them — and never near the live edge, where
//! the recorder may still file ([`PULL_EDGE_MS`]).

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rusqlite::{Connection, params};

use super::codec;
use super::store::{self, Packed, Store};
use crate::market::trade_replay::Coverage;
use crate::station_api::{
    PriceHeld, PriceIndex, PriceKey, PricePiece, Prices, TapeResume, TapeWant,
};

/// JSON around one piece's data: its item, bounds and field names.
const PIECE_OVERHEAD: usize = 96;
/// JSON of one listed stretch, both bounds at their widest.
const SPAN_JSON: usize = 44;
/// JSON around one listed market, without its names.
const MARKET_JSON: usize = 48;
/// Rows one index query reads at most before the next.
const INDEX_ROWS: i64 = 4_000;
/// How close to now the terminal pulls: the recorder files nothing older than its ring
/// (`ring_depth_ms`, at most the lookback cap and its spare) plus a flush and the settle of the
/// last list, so ground before this edge is never filed by both.
pub const PULL_EDGE_MS: i64 = super::LOOKBACK_CAP_MS
    + super::RING_SPARE_MS
    + 2 * super::FLUSH_EVERY.as_millis() as i64
    + super::SETTLE_MS;
/// Pieces the terminal files per transaction.
const IMPORT_BATCH: usize = 256;

/// A piece's packing on the wire.
pub fn encode_piece(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// The station's answer to `prices.index`: the stretches its file holds that end at or after
/// `since_ms`, after `after`, as far as `budget` bytes of JSON go. Empty when there is no file.
pub fn index(
    since_ms: i64,
    after: Option<&PriceKey>,
    budget: usize,
) -> rusqlite::Result<PriceIndex> {
    match store::open_read(&crate::config::paths::market_prices_db_path())? {
        Some(conn) => index_conn(&conn, since_ms, after, budget),
        None => Ok(PriceIndex::default()),
    }
}

/// [`index`] over an open connection.
///
/// A page ends after a whole segment: its key is the `resume`, and the next page starts just
/// after it, so a market cut by the budget continues there and abutting segments on both sides of
/// the cut still read as one stretch once the client adds them up. The first segment always goes
/// out, so asking again always gets further.
pub(super) fn index_conn(
    conn: &Connection,
    since_ms: i64,
    after: Option<&PriceKey>,
    budget: usize,
) -> rusqlite::Result<PriceIndex> {
    let mut stmt = conn.prepare_cached(
        "SELECT exchange, market, from_ms, to_ms FROM segments
         WHERE (exchange, market, from_ms) > (?1, ?2, ?3) AND to_ms >= ?4
         ORDER BY exchange, market, from_ms LIMIT ?5",
    )?;
    let mut out = PriceIndex::default();
    let mut used = 0usize;
    let mut cursor = after.cloned().unwrap_or(PriceKey {
        exchange: String::new(),
        market: String::new(),
        from_ms: i64::MIN,
    });
    loop {
        let rows: Vec<(String, String, i64, i64)> = stmt
            .query_map(
                params![
                    cursor.exchange,
                    cursor.market,
                    cursor.from_ms,
                    since_ms,
                    INDEX_ROWS
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?
            .collect::<rusqlite::Result<_>>()?;
        let full = rows.len() as i64 == INDEX_ROWS;
        for (exchange, market, from_ms, to_ms) in rows {
            let same_market = out
                .markets
                .last()
                .is_some_and(|m| m.exchange == exchange && m.market == market);
            let abuts = same_market
                && out
                    .markets
                    .last()
                    .and_then(|m| m.spans.last())
                    .is_some_and(|&(_, last_to)| from_ms <= last_to.saturating_add(1));
            let cost = match (same_market, abuts) {
                (_, true) => 0,
                (true, false) => SPAN_JSON,
                (false, _) => MARKET_JSON + exchange.len() + market.len() + SPAN_JSON,
            };
            if used + cost > budget && !out.markets.is_empty() {
                out.resume = Some(cursor);
                return Ok(out);
            }
            used += cost;
            if abuts {
                let span = out
                    .markets
                    .last_mut()
                    .and_then(|m| m.spans.last_mut())
                    .expect("an abutting segment follows a listed one");
                span.1 = span.1.max(to_ms);
            } else if same_market {
                let held = out.markets.last_mut().expect("the same market is listed");
                held.spans.push((from_ms, to_ms));
            } else {
                out.markets.push(PriceHeld {
                    exchange: exchange.clone(),
                    market: market.clone(),
                    spans: vec![(from_ms, to_ms)],
                });
            }
            cursor = PriceKey {
                exchange,
                market,
                from_ms,
            };
        }
        if !full {
            return Ok(out);
        }
    }
}

/// The station's answer to `prices.fetch`: what its file holds inside the wanted stretches, each
/// filed segment cut to them, as far as `budget` bytes go. Empty when there is no file.
pub fn fetch(items: &[TapeWant], budget: usize) -> rusqlite::Result<Prices> {
    match store::open_read(&crate::config::paths::market_prices_db_path())? {
        Some(conn) => fetch_conn(&conn, items, budget),
        None => Ok(Prices::default()),
    }
}

/// [`fetch`] over an open connection.
///
/// One piece per filed segment inside a stretch; a piece that does not fit ends the answer, which
/// says to go on from where that piece starts. A piece that alone does not fit an empty answer is
/// left out with a warning — its frame would be refused whole — and one that does not decode is
/// left out too: the stretch it spoke for stays a gap.
pub(super) fn fetch_conn(
    conn: &Connection,
    items: &[TapeWant],
    budget: usize,
) -> rusqlite::Result<Prices> {
    let mut stmt = conn.prepare_cached(
        "SELECT from_ms, to_ms, data FROM segments
         WHERE exchange = ?1 AND market = ?2 AND from_ms <= ?4 AND to_ms >= ?3
         ORDER BY from_ms",
    )?;
    let mut out = Prices::default();
    let mut used = 0usize;
    for (item, want) in items.iter().enumerate() {
        let item = item as u32;
        for &(from, to) in &want.spans {
            // Row by row: an answer that stops at its budget reads no further, so a long stretch
            // paged over many answers costs each one its own page, not the whole remainder.
            let mut rows = stmt.query(params![want.exchange, want.market, from, to])?;
            while let Some(row) = rows.next()? {
                let (seg_from, seg_to): (i64, i64) = (row.get(0)?, row.get(1)?);
                let data: Vec<u8> = row.get(2)?;
                let (a, b) = (from.max(seg_from), to.min(seg_to));
                if a > b {
                    continue;
                }
                let (start, bytes) = match codec::cut(seg_from, &data, a, b) {
                    Ok(Some(cut)) => cut,
                    Ok(None) => continue,
                    Err(e) => {
                        log::warn!(
                            "price pull: unreadable segment {} {} at {seg_from}: {e:?}",
                            want.exchange,
                            want.market
                        );
                        continue;
                    }
                };
                let data = encode_piece(&bytes);
                let size = data.len() + PIECE_OVERHEAD;
                if used + size > budget {
                    if !out.pieces.is_empty() {
                        out.resume = Some(TapeResume { item, from_ms: a });
                        return Ok(out);
                    }
                    log::warn!(
                        "price pull: the segment {} {} at {seg_from} ({size} bytes) does not fit one answer; left out",
                        want.exchange,
                        want.market
                    );
                    continue;
                }
                used += size;
                out.pieces.push(PricePiece {
                    item,
                    from_ms: start,
                    to_ms: b,
                    data,
                });
            }
        }
    }
    Ok(out)
}

/// What the terminal's file lacks of what the station listed, cut at [`PULL_EDGE_MS`] before
/// `now_ms`; one want per market, its stretches ascending.
///
/// Args:
///     held: The station's listing, every page; a market may come on several.
///     now_ms: Terminal wall clock, Unix ms.
pub fn lacking(held: Vec<PriceHeld>, now_ms: i64) -> rusqlite::Result<Vec<TapeWant>> {
    let path = crate::config::paths::market_prices_db_path();
    let conn = store::open_read(&path)?;
    lacking_with(
        held,
        now_ms,
        |exchange, market, from_ms, to_ms| match &conn {
            Some(conn) => store::held_bounds(conn, exchange, market, from_ms, to_ms),
            None => Ok(Vec::new()),
        },
    )
}

/// [`lacking`] against `local`: `(exchange, market, from_ms, to_ms)` → the bounds this file holds.
pub(super) fn lacking_with(
    held: Vec<PriceHeld>,
    now_ms: i64,
    mut local: impl FnMut(&str, &str, i64, i64) -> rusqlite::Result<Vec<(i64, i64)>>,
) -> rusqlite::Result<Vec<TapeWant>> {
    let edge = Coverage::one((i64::MIN, now_ms.saturating_sub(PULL_EDGE_MS)));
    let mut wanted: BTreeMap<(String, String), Coverage> = BTreeMap::new();
    for market in held {
        let need = wanted
            .entry((market.exchange, market.market))
            .or_insert_with(Coverage::none);
        for span in market.spans {
            if span.0 <= span.1 {
                need.add(span);
            }
        }
    }
    let mut wants = Vec::new();
    for ((exchange, market), need) in wanted {
        let need = need.clip(&edge);
        let Some((from_ms, to_ms)) = need.hull() else {
            continue;
        };
        let have = Coverage::from_spans(local(&exchange, &market, from_ms, to_ms)?);
        let lacking = need.minus(&have);
        if !lacking.is_empty() {
            wants.push(TapeWant {
                exchange,
                market,
                spans: lacking.spans().to_vec(),
            });
        }
    }
    Ok(wants)
}

/// The terminal's writer for what the station answered: its own connection to the price file,
/// filing in batches.
pub struct Importer {
    store: Store,
    pending: Vec<Packed>,
    /// Pieces filed so far.
    pub filed: usize,
    /// Pieces refused as not a packing this build reads.
    pub refused: usize,
    /// Pieces left out because the file already held part of their stretch — the recorder filed
    /// it meanwhile.
    pub overlapped: usize,
}

impl Importer {
    /// Open (or create) the terminal's price file.
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(&crate::config::paths::market_prices_db_path())
    }

    /// [`Importer::open`] at `path`.
    pub(super) fn open_at(path: &Path) -> rusqlite::Result<Self> {
        Ok(Self {
            store: Store::open(path)?,
            pending: Vec::new(),
            filed: 0,
            refused: 0,
            overlapped: 0,
        })
    }

    /// Take one piece of `want`; filed once a batch is full, or by [`Importer::finish`]. A piece
    /// whose data does not decode, or speaks outside its own bounds, is refused and counted.
    pub fn add(&mut self, want: &TapeWant, piece: &PricePiece) -> rusqlite::Result<()> {
        match checked(piece) {
            Some(data) => self.pending.push(Packed {
                exchange: want.exchange.clone(),
                market: want.market.clone(),
                from_ms: piece.from_ms,
                to_ms: piece.to_ms,
                data,
            }),
            None => {
                self.refused += 1;
                log::warn!(
                    "price pull: refused a piece of {} {} at {}: not a segment of its bounds",
                    want.exchange,
                    want.market,
                    piece.from_ms
                );
            }
        }
        if self.pending.len() >= IMPORT_BATCH {
            self.flush()?;
        }
        Ok(())
    }

    /// File what is still pending.
    pub fn finish(&mut self) -> rusqlite::Result<()> {
        self.flush()
    }

    fn flush(&mut self) -> rusqlite::Result<()> {
        let batch = std::mem::take(&mut self.pending);
        let filed = self.store.insert_packed(&batch)?;
        self.filed += filed;
        self.overlapped += batch.len() - filed;
        Ok(())
    }
}

/// A piece's bytes, when they are a segment that opens at its `from_ms` and ends by its `to_ms`.
fn checked(piece: &PricePiece) -> Option<Vec<u8>> {
    if piece.from_ms > piece.to_ms {
        return None;
    }
    let data = STANDARD.decode(&piece.data).ok()?;
    let records = codec::decode(piece.from_ms, &data).ok()?;
    let opens = records.first()?.t_ms == piece.from_ms;
    let inside = records.last()?.t_ms <= piece.to_ms;
    (opens && inside).then_some(data)
}

#[cfg(test)]
mod tests;
