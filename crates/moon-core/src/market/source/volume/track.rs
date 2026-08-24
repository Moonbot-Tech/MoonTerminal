//! A market's traded volume kept as five-second buckets, advanced by what arrived since last time.
//!
//! # Why this exists
//!
//! The straightforward read — "scan the trade ring back over the period" — is priced in ROWS, and a
//! busy market's ring holds tens of thousands of them. Asked four times a second, per pane, that is
//! a stall the user feels while dragging the chart: the sync runs on the thread that also serves
//! input, so the scan lands as a hitch every quarter second.
//!
//! The fix is the one MoonProto uses for its own five-minute figures: keep buckets, and only ever
//! fold in what is NEW. A read then costs a walk over at most [`TRACK_BUCKETS`] small structs —
//! bounded, and unrelated to how busy the market is.
//!
//! # What a bucket knows
//!
//! Value on both sides, quantity on both sides, and a trade count. Quantity is flagged rather than
//! assumed: a bucket seeded from a MINI-CANDLE carries `price × quantity` and no quantity of its
//! own, so a coin-denominated figure that touches such a bucket is reported inexact instead of
//! being invented. See [`super::VolumeSpanReadout::base_exact`].
//!
//! # Seeding
//!
//! A track is filled once, from the cheap source first: mini-candles are already five-second
//! aggregates, so an hour of them is at most [`TRACK_BUCKETS`] rows. Only the tail they do not
//! cover — the trades still in the ring, which have not been compacted yet — is walked as raw
//! rows, and only that once. Every later read drains from a cursor.

use moonproto::MoonTime;
use moonproto::state::{MiniCandle, SeqRingCursor, SeqRingReader, TradeHistoryRow};

use super::VolumeSpanReadout;

/// Width of one bucket, matching the quantization MoonProto's own rolling volumes use.
///
/// The same five seconds the mini-candles are built at, which is what lets the two seed each other
/// without a caption's figure jumping when a stretch changes source.
const BUCKET_MS: i64 = 5_000;

/// How many buckets a track keeps: one hour.
///
/// The ceiling on what this can answer, and it is chosen against what a chart asks for — the volume
/// block is read at a minute or a few, and an hour covers every fixed window up to and including
/// [`crate::config::LabelWindow::H1`]. Longer periods fall back to the retained aggregates, on a
/// slower clock, because nobody watches a three-day volume tick.
pub(super) const TRACK_BUCKETS: usize = 720;

/// The span those buckets cover, in milliseconds.
pub(super) const TRACK_SPAN_MS: i64 = BUCKET_MS * TRACK_BUCKETS as i64;

/// Most rows one advance folds in.
///
/// A bound on the catch-up after the terminal was busy elsewhere: past it the track simply reports
/// what it has and catches up on the next read, which keeps one slow moment from becoming a long
/// one. Well above what a busy market prints in a quarter of a second.
const DRAIN_LIMIT: usize = 20_000;

/// One five-second slice of a market's trading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Bucket {
    /// `floor(time / BUCKET_MS)`, or `i64::MIN` for a slot nothing has written.
    ///
    /// Stored IN the bucket because the array is addressed by `id % TRACK_BUCKETS`: a slot whose id
    /// does not match the one being asked for is an hour-old leftover, not this period's data.
    id: i64,
    buy_quote: f64,
    sell_quote: f64,
    buy_base: f64,
    sell_base: f64,
    trades: u32,
    /// Whether the quantities are real. `false` on a bucket seeded from a mini-candle.
    base_known: bool,
}

impl Bucket {
    /// Reset the slot to a fresh bucket for `id`.
    fn restart(&mut self, id: i64) {
        *self = Bucket {
            id,
            base_known: true,
            ..Bucket::default()
        };
    }
}

/// One market's rolling buckets and its place in the trade stream.
pub(super) struct MarketTrack {
    buckets: Vec<Bucket>,
    /// Next trade row to fold in.
    cursor: SeqRingCursor,
    /// Whether the buckets have been filled at all.
    seeded: bool,
    /// Earliest moment this track can answer for, in unix milliseconds.
    ///
    /// What a caption's "is this period covered" reads: a track seeded two minutes ago cannot speak
    /// for the quarter hour, however many buckets it has room for.
    earliest_ms: i64,
    /// Reusable drain buffer, so an advance that folds in forty trades allocates nothing.
    rows: Vec<TradeHistoryRow>,
    /// When this track was last read, for the prune that drops markets nobody charts any more.
    pub(super) used_ms: i64,
}

impl MarketTrack {
    /// An empty track, before it has been seeded.
    pub(super) fn new(now: i64) -> Self {
        Self {
            buckets: vec![
                Bucket {
                    id: i64::MIN,
                    ..Bucket::default()
                };
                TRACK_BUCKETS
            ],
            cursor: SeqRingCursor::default(),
            seeded: false,
            earliest_ms: now,
            rows: Vec::new(),
            used_ms: now,
        }
    }

    /// Bring the track up to date, seeding it first if this is its first read.
    ///
    /// Args:
    ///     trades: The market's raw trade ring, if it has one.
    ///     minis: Its five-second aggregates, if it has any.
    ///     now: Current unix time in milliseconds.
    pub(super) fn advance(
        &mut self,
        trades: Option<&SeqRingReader<TradeHistoryRow>>,
        minis: Option<&SeqRingReader<MiniCandle>>,
        now: i64,
    ) {
        self.used_ms = now;
        if !self.seeded {
            self.seed(trades, minis, now);
            return;
        }
        let Some(reader) = trades else {
            return;
        };
        let mut rows = std::mem::take(&mut self.rows);
        rows.clear();
        reader.drain_new_bounded(&mut self.cursor, DRAIN_LIMIT, &mut rows);
        for row in &rows {
            self.add_trade(*row);
        }
        self.rows = rows;
    }

    /// Fill the buckets for the first time; see the module docs for the order and why.
    fn seed(
        &mut self,
        trades: Option<&SeqRingReader<TradeHistoryRow>>,
        minis: Option<&SeqRingReader<MiniCandle>>,
        now: i64,
    ) {
        self.seeded = true;
        let from_ms = now - TRACK_SPAN_MS;
        let mut newest_mini = i64::MIN;
        let mut oldest_seen = i64::MAX;
        if let Some(reader) = minis {
            let cursor = reader.cursor_at_or_after_time(MoonTime::from_unix_millis(from_ms));
            let (ends, _) = reader.scan_from_cursor(
                cursor,
                TRACK_BUCKETS,
                (i64::MIN, i64::MAX),
                |(newest, oldest), row| {
                    let at = row.time.unix_millis();
                    (newest.max(at), oldest.min(at))
                },
            );
            // Folded in a second pass rather than inside the scan: the accumulator a scan carries is
            // a value, and threading the whole bucket array through it would copy it per row.
            let cursor = reader.cursor_at_or_after_time(MoonTime::from_unix_millis(from_ms));
            reader.with_from_cursor(cursor, TRACK_BUCKETS, |view| {
                view.for_each(|row| self.add_mini(*row));
            });
            newest_mini = ends.0;
            oldest_seen = ends.1;
        }
        let Some(reader) = trades else {
            self.earliest_ms = match oldest_seen == i64::MAX {
                true => now,
                false => oldest_seen,
            };
            return;
        };
        // The tail the aggregates do not cover: trades still in the ring. A mini-candle exists only
        // because its trades were evicted, so starting strictly after the newest one cannot count a
        // trade twice — the same rule the one-shot read uses.
        let tail_from = match newest_mini > i64::MIN {
            true => newest_mini + 1,
            false => from_ms,
        };
        let mut cursor = reader.cursor_at_or_after_time(MoonTime::from_unix_millis(tail_from));
        let mut rows = std::mem::take(&mut self.rows);
        loop {
            rows.clear();
            let meta = reader.drain_new_bounded(&mut cursor, DRAIN_LIMIT, &mut rows);
            if let Some(first) = rows.first() {
                oldest_seen = oldest_seen.min(first.time.unix_millis());
            }
            for row in &rows {
                self.add_trade(*row);
            }
            if meta.caught_up || meta.copied == 0 {
                break;
            }
        }
        self.rows = rows;
        self.cursor = cursor;
        self.earliest_ms = match oldest_seen == i64::MAX {
            // Nothing retained at all: the track can speak for this instant and no further back.
            true => now,
            false => oldest_seen.max(from_ms),
        };
    }

    /// Fold one trade into its bucket.
    fn add_trade(&mut self, row: TradeHistoryRow) {
        let at = row.time.unix_millis();
        let id = at.div_euclid(BUCKET_MS);
        let slot = self.slot_for(id);
        let qty = f64::from(row.quantity());
        let value = f64::from(row.price) * qty;
        if row.is_buy() {
            slot.buy_quote += value;
            slot.buy_base += qty;
        } else {
            slot.sell_quote += value;
            slot.sell_base += qty;
        }
        slot.trades = slot.trades.saturating_add(1);
    }

    /// Fold one five-second aggregate into its bucket.
    fn add_mini(&mut self, row: MiniCandle) {
        let id = row.time.unix_millis().div_euclid(BUCKET_MS);
        let slot = self.slot_for(id);
        slot.buy_quote += f64::from(row.buy_vol);
        slot.sell_quote += f64::from(row.sell_vol);
        slot.trades = slot.trades.saturating_add(row.cnt.max(0) as u32);
        // The aggregate carries no quantity, so nothing in this bucket can state one.
        slot.base_known = false;
    }

    /// The slot this bucket id owns, restarted when it holds an older period.
    fn slot_for(&mut self, id: i64) -> &mut Bucket {
        let ix = id.rem_euclid(TRACK_BUCKETS as i64) as usize;
        let slot = &mut self.buckets[ix];
        if slot.id != id {
            slot.restart(id);
        }
        slot
    }

    /// Sum the buckets covering the last `span_ms`.
    ///
    /// Args:
    ///     span_ms: Length of the period, which must not exceed [`TRACK_SPAN_MS`].
    ///     now: Current unix time in milliseconds.
    ///
    /// Returns:
    ///     The figures, with `complete` answering whether the track reaches back that far.
    pub(super) fn read(&self, span_ms: i64, now: i64) -> VolumeSpanReadout {
        let from_ms = now - span_ms;
        let first_id = from_ms.div_euclid(BUCKET_MS);
        let last_id = now.div_euclid(BUCKET_MS);
        let mut out = VolumeSpanReadout {
            base_exact: true,
            complete: self.earliest_ms <= from_ms,
            ..VolumeSpanReadout::default()
        };
        for slot in &self.buckets {
            if slot.id < first_id || slot.id > last_id {
                continue;
            }
            out.buy_quote += slot.buy_quote;
            out.sell_quote += slot.sell_quote;
            out.buy_base += slot.buy_base;
            out.sell_base += slot.sell_base;
            out.trades = out.trades.saturating_add(slot.trades);
            out.base_exact &= slot.base_known;
        }
        out
    }
}

#[cfg(test)]
mod tests;
