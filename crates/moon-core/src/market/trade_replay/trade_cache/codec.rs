//! The two print layouts of `trades.sqlite`: the legacy fixed-width rows of the `spans` table and
//! the compressed columns of the `packs` table.
//!
//! # The packed layout
//!
//! ```text
//! varint n                      prints in the blob, outside the compressed stream
//! deflate(                      absent when n = 0
//!     n × zigzag varint         stamp minus the previous stamp (the first against 0), ms
//!     n × f32 LE                price
//!     n × f32 LE                size
//!     ceil(n/8) bytes           side, bit i%8 of byte i/8: 1 = sell
//! )
//! ```
//!
//! Columns, not rows: deflate finds the repeats inside one column — a price that holds for a
//! hundred prints, stamps a few milliseconds apart — and the row layout interleaves them away.
//! Measured on a 220 MB file of 10.8 M prints (2026-09-25, 800 sampled spans): 20 bytes a print
//! as rows, 5.6 compressed as rows, 3.9 as these columns. The side is a bit of its own rather
//! than the size's sign: a size that arrives negative must come back as it went in.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;

use crate::feed::types::{Side, Tick};

/// Bytes per print of the legacy layout: `i64 time_ms`, `f32 price`, `f32 qty`, `u32 side`
/// (`0` buy, `1` sell). The stamp is absolute, not an offset from the span's edge: a span is the
/// position's own focus, and a position can be held for months.
pub(super) const LEGACY_ROW_BYTES: usize = 20;

/// Deflate level for the packed layout. 6 (zlib's default) against 1: 8 % smaller for a quarter
/// of the encode speed, and encoding runs only on the cache's own thread, once per span.
const LEVEL: u32 = 6;

/// Most prints one packed blob may claim. A header past this is not a span this store wrote —
/// the busiest harvest is a few hundred thousand — and trusting it would size an allocation.
const MAX_PRINTS: u64 = 50_000_000;

/// Most bytes deflate can expand one compressed byte into (zlib's technical notes: a stream of
/// one repeated byte, 258-byte matches at two bits each, peaks at 1032:1). What bounds a blob's
/// inflated size by the blob's own length rather than by the header it carries.
const MAX_DEFLATE_RATIO: usize = 1032;

/// Bytes reserved up front per packed byte when inflating. The columns inflate to about three
/// times their packed size (11 bytes a print against 3.9); twice more leaves a real span one
/// allocation, and a blob that claims more grows the buffer only as its stream yields bytes.
const RESERVE_PER_PACKED_BYTE: usize = 8;

/// Why a packed blob did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DecodeError {
    /// The header varint is missing or runs past the blob.
    Header,
    /// The header claims more prints than any span this store writes.
    TooMany(u64),
    /// The header claims more prints than the blob's compressed stream can inflate to.
    Unholdable(u64),
    /// The compressed stream is damaged.
    Inflate(String),
    /// The stream decoded to the wrong number of bytes for its print count.
    Length,
    /// The stream inflates past the most its print count can take.
    TooLong,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header => write!(f, "no print count"),
            Self::TooMany(n) => write!(f, "claims {n} prints"),
            Self::Unholdable(n) => write!(f, "claims {n} prints, more than the blob can hold"),
            Self::Inflate(e) => write!(f, "inflate: {e}"),
            Self::Length => write!(f, "columns shorter than the print count"),
            Self::TooLong => write!(f, "columns longer than the print count"),
        }
    }
}

/// Pack prints in the legacy row layout. Kept for the tests that build a file an older build
/// wrote; nothing in the store writes it any more.
#[cfg(test)]
pub(super) fn encode_legacy(ticks: &[Tick]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ticks.len() * LEGACY_ROW_BYTES);
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

/// Unpack the legacy row layout; a trailing partial row is ignored.
pub(super) fn decode_legacy(blob: &[u8]) -> Vec<Tick> {
    let mut out = Vec::with_capacity(blob.len() / LEGACY_ROW_BYTES);
    for chunk in blob.chunks_exact(LEGACY_ROW_BYTES) {
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

/// Pack prints in the packed layout, in the order given. Stamps are whole milliseconds, as the
/// legacy layout stored them.
pub(super) fn encode(ticks: &[Tick]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, ticks.len() as u64);
    if ticks.is_empty() {
        return out;
    }
    let n = ticks.len();
    let mut raw = Vec::with_capacity(n * 11 + n / 8 + 1);
    let mut prev = 0i64;
    for t in ticks {
        let time_ms = t.time_ms as i64;
        put_varint(&mut raw, zigzag(time_ms.wrapping_sub(prev)));
        prev = time_ms;
    }
    for t in ticks {
        raw.extend_from_slice(&t.price.to_le_bytes());
    }
    for t in ticks {
        raw.extend_from_slice(&t.qty.to_le_bytes());
    }
    let mut sides = vec![0u8; n.div_ceil(8)];
    for (i, t) in ticks.iter().enumerate() {
        if t.side == Side::Sell {
            sides[i / 8] |= 1 << (i % 8);
        }
    }
    raw.extend_from_slice(&sides);
    let mut enc = DeflateEncoder::new(out, Compression::new(LEVEL));
    enc.write_all(&raw)
        .expect("deflate into a Vec cannot fail to write");
    enc.finish().expect("deflate into a Vec cannot fail")
}

/// Unpack the packed layout.
///
/// Errors:
///     [`DecodeError`] for a blob this module did not write, or one damaged since.
pub(super) fn decode(blob: &[u8]) -> Result<Vec<Tick>, DecodeError> {
    let (n, head) = get_varint(blob).ok_or(DecodeError::Header)?;
    if n > MAX_PRINTS {
        return Err(DecodeError::TooMany(n));
    }
    let n = n as usize;
    if n == 0 {
        return Ok(Vec::new());
    }
    // Nothing below is sized off the header alone: a row damaged after it was written may claim
    // any count up to `MAX_PRINTS`. The columns take at least one stamp byte, eight fixed bytes
    // and a side bit per print, at most ten stamp bytes; and the stream cannot inflate past what
    // deflate makes of this blob's own length. A count those bounds cannot hold is refused
    // outright; the inflate stops one byte past the most the count can take; and the buffer is
    // reserved off the blob's length, growing only as the stream actually yields bytes.
    let side_bytes = n.div_ceil(8);
    let least = n * 9 + side_bytes;
    let most = n * 18 + side_bytes;
    let packed = blob.len() - head;
    if least > packed.saturating_mul(MAX_DEFLATE_RATIO) {
        return Err(DecodeError::Unholdable(n as u64));
    }
    let mut raw = Vec::with_capacity(
        (n * 11 + side_bytes).min(packed.saturating_mul(RESERVE_PER_PACKED_BYTE)),
    );
    DeflateDecoder::new(&blob[head..])
        .take(most as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| DecodeError::Inflate(e.to_string()))?;
    if raw.len() > most {
        return Err(DecodeError::TooLong);
    }
    if raw.len() < least {
        return Err(DecodeError::Length);
    }
    let mut pos = 0usize;
    let mut stamps = Vec::with_capacity(n);
    let mut prev = 0i64;
    for _ in 0..n {
        let (z, used) = get_varint(&raw[pos..]).ok_or(DecodeError::Length)?;
        pos += used;
        prev = prev.wrapping_add(unzigzag(z));
        stamps.push(prev);
    }
    let fixed = n * 8 + n.div_ceil(8);
    if raw.len() != pos + fixed {
        return Err(DecodeError::Length);
    }
    let f32_at = |i: usize| {
        f32::from_le_bytes(
            raw[i..i + 4]
                .try_into()
                .expect("four bytes inside the length check"),
        )
    };
    let prices = pos;
    let sizes = prices + n * 4;
    let sides = sizes + n * 4;
    Ok(stamps
        .into_iter()
        .enumerate()
        .map(|(i, time_ms)| Tick {
            time_ms: time_ms as f64,
            price: f32_at(prices + i * 4),
            qty: f32_at(sizes + i * 4),
            side: if raw[sides + i / 8] & (1 << (i % 8)) != 0 {
                Side::Sell
            } else {
                Side::Buy
            },
        })
        .collect())
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(z: u64) -> i64 {
    ((z >> 1) as i64) ^ -((z & 1) as i64)
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A LEB128 varint at the head of `bytes`: its value and the bytes it took, or `None` for one
/// that runs past the end or past 64 bits.
fn get_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, &b) in bytes.iter().enumerate().take(10) {
        v |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests;
