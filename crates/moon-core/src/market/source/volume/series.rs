//! A market's BOUGHT and SOLD turnover as an ordered series of one-second slots, as deep as the
//! retained history reaches — the data behind the chart's `candle_volume_sides` band.
//!
//! # Resolution
//!
//! Trades still in the ring land in the slot of their own second, so the live tail has the
//! per-print edges Moonbot's band shows (its `Auto` goes down to two seconds). Mini-candles are
//! five-second aggregates and land whole in the slot of their open, so history older than the
//! ring steps every five seconds — the finest the retained history can say.
//!
//! # Why not the track
//!
//! [`super::track::MarketTrack`] answers "how much over the last period" and keeps one hour in a
//! ring addressed by bucket id. The chart draws a WINDOW — any hour of the last day, at any bucket
//! width — so it needs the buckets in order, back to wherever the mini-candles end, and it needs
//! them summed over a ROLLING window at every step of the way. That is a sorted vector, not a
//! ring, and it is only ever walked over the window being drawn.
//!
//! # Seeding and advancing
//!
//! The same two sources in the same order as the track, and for the same reason: the retained
//! mini-candles are built from trades EVICTED from the trade ring, so the two never overlap. The
//! series is seeded once from every mini-candle and the trades newer than the newest of them, then
//! advanced from a cursor over the trade ring alone. Two things rebuild it from scratch rather
//! than leave a hole: a cursor that fell behind retention (a terminal busy for longer than the
//! ring holds) reports itself as `clipped`, and the rows it missed are now mini-candles; and the
//! core's chart ARCHIVE arriving, which prepends rows OLDER than any cursor — exactly the case
//! `MarketRevisions::archive` documents for every cursor-keeping consumer. A chart is usually
//! open before its archive lands, so without the second the band would stay "since connect"
//! for as long as the pane lived.
//!
//! # Units
//!
//! Quote turnover only. A mini-candle carries `price × quantity` and no quantity, so a
//! coin-denominated series would be unknown over most of its length; the chart band is monetary
//! either way, like the candle band beside it.

use moonproto::MoonTime;
use moonproto::state::{MiniCandle, SeqRingCursor, SeqRingReader, TradeHistoryRow};

/// Width of one native slot, milliseconds: a second, so a trade keeps its own second and a
/// two-second interval can be summed honestly. Mini-candles (five-second aggregates) fill one
/// slot in five.
pub const SIDE_BUCKET_MS: i64 = 1_000;

/// Most rows one advance folds in — the same bound the track uses, for the same reason.
const DRAIN_LIMIT: usize = 20_000;

/// Most slots a series keeps: three days of one-second slots if every second traded, well past
/// what the retained mini-candles reach. The seed cannot exceed it, and a pane left open for a
/// week would otherwise grow one slot per traded second for the whole session — an advance drops
/// the oldest past this, so a market costs at most ~12 MB (the vector's capacity doubles on the
/// push that crosses the cap and is never shrunk) however long it is watched.
const MAX_SLOTS: usize = 262_144;

/// One bucket of bought and sold turnover, in the market's quote currency.
///
/// What the chart uploads. For the live series this is one SAMPLE of the rolling sums: `tf_ms` is
/// the sample's screen span (the sampling step), not the window the sums cover — the window is the
/// band's configured interval, which the pane knows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SideVolumeBucket {
    /// Bucket opening time, unix milliseconds, aligned to `tf_ms`.
    pub t_open_ms: i64,
    /// Bucket width, milliseconds.
    pub tf_ms: i64,
    /// Bought over the bucket, quote currency.
    pub buy_quote: f32,
    /// Sold over the bucket, quote currency.
    pub sell_quote: f32,
}

/// One native slot, keyed by `floor(time / SIDE_BUCKET_MS)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Slot {
    id: i64,
    buy: f64,
    sell: f64,
}

/// One market's ordered buckets and its place in the trade stream.
pub(super) struct SideSeries {
    /// Sorted by `id`, one slot per non-empty second.
    slots: Vec<Slot>,
    /// Next trade row to fold in.
    cursor: SeqRingCursor,
    /// Whether the slots have been filled at all.
    seeded: bool,
    /// `MarketRevisions::archive` the slots were seeded under; a different one is a rebuild.
    archive_rev: u64,
    /// Reusable drain buffer, so an advance that folds in forty trades allocates nothing.
    rows: Vec<TradeHistoryRow>,
    /// When this series was last read, for the prune that drops markets nobody charts any more.
    pub(super) used_ms: i64,
}

impl SideSeries {
    /// An empty series, before it has been seeded.
    pub(super) fn new(now: i64) -> Self {
        Self {
            slots: Vec::new(),
            cursor: SeqRingCursor::default(),
            seeded: false,
            archive_rev: 0,
            rows: Vec::new(),
            used_ms: now,
        }
    }

    /// Bring the series up to date, seeding it first if this is its first read.
    ///
    /// Args:
    ///     trades: The market's raw trade ring, if it has one.
    ///     minis: Its five-second aggregates, if it has any.
    ///     archive_rev: The market's `MarketRevisions::archive`; a change rebuilds the series.
    ///     now: Current unix time in milliseconds.
    pub(super) fn advance(
        &mut self,
        trades: Option<&SeqRingReader<TradeHistoryRow>>,
        minis: Option<&SeqRingReader<MiniCandle>>,
        archive_rev: u64,
        now: i64,
    ) {
        self.used_ms = now;
        if self.seeded && self.archive_rev != archive_rev {
            // The archive merged rows behind the cursor; only a rebuild sees them.
            self.slots.clear();
            self.seeded = false;
        }
        self.archive_rev = archive_rev;
        if !self.seeded {
            self.seed(trades, minis);
            return;
        }
        let Some(reader) = trades else {
            return;
        };
        let mut rows = std::mem::take(&mut self.rows);
        rows.clear();
        let meta = reader.drain_new_bounded(&mut self.cursor, DRAIN_LIMIT, &mut rows);
        if meta.clipped {
            // The ring moved past the cursor: the rows between are mini-candles now, and only a
            // rebuild sees them. Cheaper than a hole the band would draw as a quiet stretch.
            self.rows = rows;
            self.slots.clear();
            self.seed(trades, minis);
            return;
        }
        for row in &rows {
            self.add_trade(*row);
        }
        self.rows = rows;
        self.trim();
    }

    /// Drop the oldest slots past [`MAX_SLOTS`]; see there.
    fn trim(&mut self) {
        let excess = self.slots.len().saturating_sub(MAX_SLOTS);
        if excess > 0 {
            self.slots.drain(..excess);
        }
    }

    /// Fill the slots for the first time; see the module docs for the order and why.
    fn seed(
        &mut self,
        trades: Option<&SeqRingReader<TradeHistoryRow>>,
        minis: Option<&SeqRingReader<MiniCandle>>,
    ) {
        self.seeded = true;
        let mut newest_mini = i64::MIN;
        if let Some(reader) = minis {
            let cursor = reader.cursor_from_oldest();
            reader.with_from_cursor(cursor, reader.capacity().max(1), |view| {
                view.for_each(|row| {
                    let at = row.time.unix_millis();
                    newest_mini = newest_mini.max(at);
                    self.add_mini(*row);
                });
            });
        }
        let Some(reader) = trades else {
            return;
        };
        // The tail the aggregates do not cover. A mini-candle exists only because its trades were
        // evicted, so starting strictly after the newest one cannot count a trade twice.
        let mut cursor = match newest_mini > i64::MIN {
            true => reader.cursor_at_or_after_time(MoonTime::from_unix_millis(newest_mini + 1)),
            false => reader.cursor_from_oldest(),
        };
        let mut rows = std::mem::take(&mut self.rows);
        loop {
            rows.clear();
            let meta = reader.drain_new_bounded(&mut cursor, DRAIN_LIMIT, &mut rows);
            for row in &rows {
                self.add_trade(*row);
            }
            if meta.caught_up || meta.copied == 0 {
                break;
            }
        }
        self.rows = rows;
        self.cursor = cursor;
        self.trim();
    }

    /// Fold one trade into its slot.
    fn add_trade(&mut self, row: TradeHistoryRow) {
        let qty = f64::from(row.quantity());
        let value = f64::from(row.price) * qty;
        if !value.is_finite() || value <= 0.0 {
            return;
        }
        let id = row.time.unix_millis().div_euclid(SIDE_BUCKET_MS);
        let (buy, sell) = match row.is_buy() {
            true => (value, 0.0),
            false => (0.0, value),
        };
        self.add(id, buy, sell);
    }

    /// Fold one five-second aggregate into the slot of its open second.
    fn add_mini(&mut self, row: MiniCandle) {
        let id = row.time.unix_millis().div_euclid(SIDE_BUCKET_MS);
        let buy = f64::from(row.buy_vol);
        let sell = f64::from(row.sell_vol);
        self.add(
            id,
            if buy.is_finite() { buy.max(0.0) } else { 0.0 },
            if sell.is_finite() { sell.max(0.0) } else { 0.0 },
        );
    }

    /// Add to the slot for `id`, creating it in order.
    ///
    /// Rows arrive in time order almost always, so the common case is the last slot or a new one
    /// after it; an out-of-order row pays a binary search rather than corrupting the order.
    fn add(&mut self, id: i64, buy: f64, sell: f64) {
        match self.slots.last_mut() {
            Some(last) if last.id == id => {
                last.buy += buy;
                last.sell += sell;
            }
            Some(last) if last.id < id => self.slots.push(Slot { id, buy, sell }),
            None => self.slots.push(Slot { id, buy, sell }),
            Some(_) => match self.slots.binary_search_by_key(&id, |s| s.id) {
                Ok(ix) => {
                    self.slots[ix].buy += buy;
                    self.slots[ix].sell += sell;
                }
                Err(ix) => self.slots.insert(ix, Slot { id, buy, sell }),
            },
        }
    }

    /// Sample the ROLLING sums over `[from_ms, to_ms]`: at every `step_ms` the turnover of each
    /// side over the `tf_ms` that ended there.
    ///
    /// This is what Moonbot's `Vol` draws — "the volume at each moment is the total over the
    /// interval" — and it is why its band is a continuous hill rather than a row of columns: a
    /// print stays in the picture for a whole interval after it happened, and a thin market
    /// reads as plateaus instead of lone bars. See [`rolling_slots`] for the sampling contract.
    ///
    /// Args:
    ///     tf_ms: Rolling window length, milliseconds; below the native width reads as native.
    ///     step_ms: Sample spacing, milliseconds — a multiple of the native width, at most one
    ///         sample per screen pixel; below the native width reads as native.
    ///     from_ms: Lower bound on sample open time; the first sample is aligned down to a whole
    ///         step and may therefore open up to one step before it.
    ///     to_ms: Inclusive upper bound on sample open time.
    ///     out: Reused buffer; cleared first.
    pub(super) fn rolling_into(
        &self,
        tf_ms: i64,
        step_ms: i64,
        from_ms: i64,
        to_ms: i64,
        out: &mut Vec<SideVolumeBucket>,
    ) {
        rolling_slots(&self.slots, tf_ms, step_ms, from_ms, to_ms, out);
    }
}

/// Round a width up to a whole number of native slots, never below one.
fn native_multiple(ms: i64) -> i64 {
    let n = (ms.max(1) + SIDE_BUCKET_MS - 1) / SIDE_BUCKET_MS;
    n.max(1) * SIDE_BUCKET_MS
}

/// The sampling behind [`SideSeries::rolling_into`], on a bare slot slice so it can be tested.
///
/// A sample opening at `t` covers the screen span `[t, t + step)` and carries the sums over the
/// slots opening in `[t + step - tf, t + step)` — the window that ENDS where the sample ends, so
/// the picture at a pixel is what traded in the interval up to that pixel. Only samples with
/// something in them are emitted: a stretch quiet for longer than the window costs nothing.
///
/// Two pointers walk the sorted slots once, so the cost is the samples plus the slots inside the
/// range, never the whole series.
fn rolling_slots(
    slots: &[Slot],
    tf_ms: i64,
    step_ms: i64,
    from_ms: i64,
    to_ms: i64,
    out: &mut Vec<SideVolumeBucket>,
) {
    out.clear();
    if from_ms > to_ms {
        return;
    }
    let step = native_multiple(step_ms);
    // Never narrower than the step: a window shorter than the gap between samples would leave
    // prints between two windows in no sample at all — dropped, not undersampled. A far zoom
    // therefore reads each sample as the total over its own step, which is the honest picture.
    let tf = native_multiple(tf_ms).max(step);
    let first = from_ms.div_euclid(step) * step;
    // Slots at or after the first window's start; everything older never enters a window.
    let window_start = first + step - tf;
    let mut lo = slots.partition_point(|s| s.id * SIDE_BUCKET_MS < window_start);
    let mut hi = lo;
    let (mut buy, mut sell) = (0.0f64, 0.0f64);
    let mut t = first;
    while t <= to_ms {
        let end = t + step;
        // Take in the slots that entered the window, drop the ones that left it.
        while hi < slots.len() && slots[hi].id * SIDE_BUCKET_MS < end {
            buy += slots[hi].buy;
            sell += slots[hi].sell;
            hi += 1;
        }
        while lo < hi && slots[lo].id * SIDE_BUCKET_MS < end - tf {
            buy -= slots[lo].buy;
            sell -= slots[lo].sell;
            lo += 1;
        }
        // A window that emptied out leaves rounding dust behind the subtractions; clear it so
        // an empty sample is empty and a run of them uploads nothing.
        if lo == hi {
            buy = 0.0;
            sell = 0.0;
        }
        if buy > 0.0 || sell > 0.0 {
            out.push(SideVolumeBucket {
                t_open_ms: t,
                tf_ms: step,
                buy_quote: buy.max(0.0) as f32,
                sell_quote: sell.max(0.0) as f32,
            });
        }
        t = end;
    }
}

/// A bench's stand-in for the split: each candle's turnover divided by its direction, spread
/// evenly over the candle's seconds, then sampled exactly as the live series is.
///
/// The fixture holds candles and no trade history, so it has no sides to serve. This gives the
/// band something to draw on the bench — a rising bar leans to buying, a falling one to selling —
/// and is NOT a measurement: `docs-internal/FIXTURES.md` lists it among what the bench makes up.
/// Spread rather than lumped at the candle's open so a rolling interval narrower than a candle
/// still reads as a continuous hill, the way live slots do.
pub fn synthetic_sides(
    candles: &[crate::market::ChartCandle],
    tf_ms: i64,
    step_ms: i64,
    from_ms: i64,
    to_ms: i64,
) -> Vec<SideVolumeBucket> {
    let mut slots: Vec<Slot> = Vec::new();
    for c in candles {
        if !(c.quote_volume.is_finite() && c.quote_volume > 0.0 && c.t_open_ms.is_finite()) {
            continue;
        }
        // The bench serves minutes, spread over the minute's slots.
        let candle_ms = 60_000i64;
        let parts = (candle_ms / SIDE_BUCKET_MS).max(1);
        let buy_share: f64 = if c.close >= c.open { 0.62 } else { 0.38 };
        let total = f64::from(c.quote_volume) / parts as f64;
        let first = (c.t_open_ms as i64).div_euclid(SIDE_BUCKET_MS);
        for i in 0..parts {
            slots.push(Slot {
                id: first + i,
                buy: total * buy_share,
                sell: total * (1.0 - buy_share),
            });
        }
    }
    slots.sort_by_key(|s| s.id);
    slots.dedup_by(|b, a| {
        if a.id == b.id {
            a.buy += b.buy;
            a.sell += b.sell;
            true
        } else {
            false
        }
    });
    let mut out = Vec::new();
    rolling_slots(&slots, tf_ms, step_ms, from_ms, to_ms, &mut out);
    out
}

#[cfg(test)]
mod tests;
