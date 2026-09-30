//! The tape as it travels between the station and a terminal (STATION.md §4.9): the station reads
//! its recorder's file without owning it, the prints cross the wire in the file's own packing, and
//! the terminal files what arrived as the station's.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::{StoredSpan, TradeCache, codec, read_spans_dropping};
use crate::feed::types::Tick;
use crate::market::trade_replay::tick_tiles::TileSource;

/// Pack prints for the wire: the file's own codec (~4 bytes a print), in base64.
///
/// Args:
///     ticks: Ascending by time.
pub fn encode_prints(ticks: &[Tick]) -> String {
    STANDARD.encode(codec::encode(ticks))
}

/// Unpack [`encode_prints`]; `None` when the text is not such a packing.
pub fn decode_prints(text: &str) -> Option<Vec<Tick>> {
    let bytes = STANDARD.decode(text).ok()?;
    codec::decode(&bytes).ok()
}

/// A tape file read by a process that does not own it — the station's API reading the recorder's
/// `tape_recorder.sqlite`. Read-only: a row that does not decode is skipped, never deleted; that is
/// the owner's to do.
pub struct TapeFile {
    conn: rusqlite::Connection,
}

impl TapeFile {
    /// Open `path` read-only; `Err` when it does not exist yet or is not such a file.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        Ok(Self { conn })
    }

    /// Every span of the market intersecting `[from_ms, to_ms]`, ascending — covered stretches
    /// with every print filed in them; an empty one is covered and quiet.
    pub fn spans(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> rusqlite::Result<Vec<StoredSpan>> {
        read_spans_dropping(&self.conn, exchange, market, from_ms, to_ms, None)
    }
}

impl TradeCache {
    /// Queue a span the station answered: `[from_ms, to_ms]` covered, with these prints. Filed
    /// like any source — only the stretches not yet held — under [`TileSource::Station`].
    pub fn insert_from_station(
        &self,
        exchange: &str,
        market: &str,
        from_ms: i64,
        to_ms: i64,
        ticks: Vec<Tick>,
    ) {
        self.insert(exchange, market, from_ms, to_ms, ticks, TileSource::Station);
    }
}

#[cfg(test)]
mod tests;
