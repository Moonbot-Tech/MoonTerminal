//! One filed segment's points as bytes.
//!
//! Layout, after a version byte: one record per moment, in time order —
//!
//! - the moment, as a LEB128 delta from the previous one (the first from the segment's own
//!   `from_ms`);
//! - a flags byte naming which values follow: bid, ask, mark, funding;
//! - those values: each price an `f64` little-endian, funding its rate (`f64`) and the next
//!   charge's Unix milliseconds (zigzag LEB128).
//!
//! A value that did not change since the previous record is left out; the first record carries
//! every price. Prices stay `f64`: a coin priced at `1e-8` and one at `1e5` both need every digit
//! the core sent.

use super::ring::Segment;

/// Version of the layout above.
const VERSION: u8 = 1;
const BID: u8 = 1;
const ASK: u8 = 2;
const MARK: u8 = 4;
const FUNDING: u8 = 8;

/// The state one record leaves behind: every price, and funding once a record carried it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Decoded {
    pub t_ms: i64,
    pub bid: f64,
    pub ask: f64,
    pub mark: f64,
    pub funding: Option<(f64, i64)>,
}

/// Why a blob could not be read back.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DecodeError {
    /// Written by a layout this build does not know.
    Version(u8),
    /// Ended inside a record.
    Truncated,
    /// The first record does not carry every price.
    NoState,
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Zigzag, so a small negative delta stays short.
fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Encode a segment. Points and funding are merged by moment; a moment that changes nothing
/// (funding re-sent unchanged) writes no record.
pub(super) fn encode(segment: &Segment) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + segment.points.len() * 20 + segment.funding.len() * 18);
    out.push(VERSION);
    let mut prev_t = segment.from_ms;
    let mut prev: Option<(f64, f64, f64)> = None;
    let mut prev_funding: Option<(f64, i64)> = None;
    let mut pending_funding: Option<(f64, i64)> = None;
    let (mut p, mut f) = (0, 0);
    while p < segment.points.len() || f < segment.funding.len() {
        let pt = segment.points.get(p).map(|x| x.t_ms);
        let ft = segment.funding.get(f).map(|x| x.t_ms);
        let t = match (pt, ft) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => break,
        };
        let mut flags = 0u8;
        let mut price = None;
        if pt == Some(t) {
            let point = segment.points[p];
            p += 1;
            let now = (point.bid, point.ask, point.mark);
            match prev {
                None => flags |= BID | ASK | MARK,
                Some((bid, ask, mark)) => {
                    if bid.to_bits() != now.0.to_bits() {
                        flags |= BID;
                    }
                    if ask.to_bits() != now.1.to_bits() {
                        flags |= ASK;
                    }
                    if mark.to_bits() != now.2.to_bits() {
                        flags |= MARK;
                    }
                }
            }
            price = Some(now);
        }
        if ft == Some(t) {
            let fund = segment.funding[f];
            f += 1;
            pending_funding = Some((fund.rate, fund.next_ms));
        }
        // Funding ahead of the first price has no record to ride on: it waits for that price.
        if prev.is_none() && price.is_none() {
            continue;
        }
        let mut funding = None;
        if let Some(now) = pending_funding.take() {
            let changed = prev_funding
                .is_none_or(|(rate, next)| rate.to_bits() != now.0.to_bits() || next != now.1);
            if changed {
                flags |= FUNDING;
                funding = Some(now);
            }
        }
        if flags == 0 {
            continue;
        }
        put_varint(&mut out, t.saturating_sub(prev_t).max(0) as u64);
        prev_t = t.max(prev_t);
        out.push(flags);
        if let Some(now) = price {
            if flags & BID != 0 {
                put_f64(&mut out, now.0);
            }
            if flags & ASK != 0 {
                put_f64(&mut out, now.1);
            }
            if flags & MARK != 0 {
                put_f64(&mut out, now.2);
            }
            prev = Some(now);
        }
        if let Some(now) = funding {
            put_f64(&mut out, now.0);
            put_varint(&mut out, zigzag(now.1));
            prev_funding = Some(now);
        }
    }
    out
}

/// Byte reader over one blob.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, DecodeError> {
        let b = *self.data.get(self.at).ok_or(DecodeError::Truncated)?;
        self.at += 1;
        Ok(b)
    }

    fn varint(&mut self) -> Result<u64, DecodeError> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(DecodeError::Truncated)
    }

    fn f64(&mut self) -> Result<f64, DecodeError> {
        let bytes = self
            .data
            .get(self.at..self.at + 8)
            .ok_or(DecodeError::Truncated)?;
        self.at += 8;
        let mut raw = [0u8; 8];
        raw.copy_from_slice(bytes);
        Ok(f64::from_le_bytes(raw))
    }
}

/// Decode a blob written by [`encode`] for a segment that starts at `from_ms`: the full state
/// after every record.
pub(super) fn decode(from_ms: i64, data: &[u8]) -> Result<Vec<Decoded>, DecodeError> {
    let mut r = Reader { data, at: 0 };
    let version = r.byte()?;
    if version != VERSION {
        return Err(DecodeError::Version(version));
    }
    let mut out = Vec::new();
    let mut t = from_ms;
    let mut state: Option<Decoded> = None;
    while r.at < data.len() {
        t = t.saturating_add(i64::try_from(r.varint()?).unwrap_or(i64::MAX));
        let flags = r.byte()?;
        let mut next = match state {
            Some(s) => s,
            None if flags & (BID | ASK | MARK) == BID | ASK | MARK => Decoded {
                t_ms: t,
                bid: 0.0,
                ask: 0.0,
                mark: 0.0,
                funding: None,
            },
            None => return Err(DecodeError::NoState),
        };
        next.t_ms = t;
        if flags & BID != 0 {
            next.bid = r.f64()?;
        }
        if flags & ASK != 0 {
            next.ask = r.f64()?;
        }
        if flags & MARK != 0 {
            next.mark = r.f64()?;
        }
        if flags & FUNDING != 0 {
            let rate = r.f64()?;
            next.funding = Some((rate, unzigzag(r.varint()?)));
        }
        state = Some(next);
        out.push(next);
    }
    Ok(out)
}

/// Re-encode the part of a blob, filed for a segment that starts at `seg_from`, that speaks for
/// `[from_ms, to_ms]`, as a segment of its own: its first record is the state in force at
/// `from_ms`, stamped there, so the cut opens with every price as a segment must.
///
/// Returns:
///     The cut's bytes, with the moment it actually starts — later than `from_ms` only if the
///     blob's first record is — or `None` for a blob with no record or nothing up to `to_ms`.
pub(super) fn cut(
    seg_from: i64,
    data: &[u8],
    from_ms: i64,
    to_ms: i64,
) -> Result<Option<(i64, Vec<u8>)>, DecodeError> {
    let records = decode(seg_from, data)?;
    let opening = records.partition_point(|r| r.t_ms <= from_ms);
    let (start, state, rest) = match opening.checked_sub(1) {
        Some(at) => (from_ms, records[at], &records[opening..]),
        None => match records.first() {
            Some(&first) => (first.t_ms, first, &records[1..]),
            None => return Ok(None),
        },
    };
    if start > to_ms {
        return Ok(None);
    }
    let kept = rest.iter().take_while(|r| r.t_ms <= to_ms);
    let records: Vec<Decoded> = std::iter::once(Decoded {
        t_ms: start,
        ..state
    })
    .chain(kept.copied())
    .collect();
    let segment = Segment {
        from_ms: start,
        to_ms,
        points: records
            .iter()
            .map(|r| super::ring::Point {
                t_ms: r.t_ms,
                bid: r.bid,
                ask: r.ask,
                mark: r.mark,
            })
            .collect(),
        funding: records
            .iter()
            .filter_map(|r| {
                r.funding.map(|(rate, next_ms)| super::ring::Funding {
                    t_ms: r.t_ms,
                    rate,
                    next_ms,
                })
            })
            .collect(),
    };
    Ok(Some((start, encode(&segment))))
}

#[cfg(test)]
mod tests;
