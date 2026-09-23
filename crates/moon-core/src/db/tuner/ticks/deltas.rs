//! Live coin deltas — the core's `d1m … d24h` re-evaluated along a trade's window the way the
//! core evaluates them, where the report keeps ONE snapshot per trade.
//!
//! What the core computes (`docs-internal/STRATEGY_FORMULAS/deltas.md`: the FAQ, `data/faqru.tsv`
//! :1052, and moonproto's parity port of the core, `state/history_store/derived.rs`):
//!
//! - every coin delta is a RANGE, `(max / min − 1) · 100` over a window — never negative;
//! - the long ones run over the core's five-minute candles, stamped at their END and kept while
//!   younger than the window, so a window reaches up to one candle past its name: d15m, d1h, and
//!   "d3h" over candles younger than four hours (the FAQ counts that "3ч55м"; the oldest candle
//!   began up to 4h05m ago, and on 52 MoonShot trades the report exceeded the range of 3h55m + one
//!   candle five times, of 4h + one candle twice) and "d24h" over twenty-five, both never under
//!   d1h;
//! - the short ones, d1m and d5m, run over five-second buckets of trades;
//! - the core refreshes them on those five-second buckets. A value holds from one bucket boundary
//!   to the next and is computed over what printed BEFORE the boundary: the MoonShot report's
//!   d1m is the range of the prints up to the last completed bucket before the fill (12 exact
//!   matches of 12 found, 2026-09-23), never the range at the fill itself.
//!
//! Inputs: the tape where it is held, the one-minute bars the tuner's own candle stage keeps in
//! `klines.sqlite` (six hours before every window it fetched), and the recorder's five-minute bars
//! where no minute bar lies. A window the bars do not reach back for is not live and keeps the
//! report's snapshot — d24h as a rule, whose twenty-five hours the cache seldom holds whole.
//!
//! Anchored to the report: at the moment the report stamped its deltas ([`snapshot_ms`]) the track
//! equals the report exactly, and elsewhere it moves by what the tape and the bars moved. The
//! offset absorbs what those inputs cannot see — the phase of the core's candles, and a core that
//! follows the averaged price instead of raw trades (its `DeltasByTrades` switch off). Without
//! the anchor there is no track ([`track_for`]): evaluated alone over the whole replica
//! (2026-09-23, 1 140 trades) the bars and the tape met the report's snapshot to a median of
//! 0.03 pp on d1h, d3h and d24h but 0.15–0.16 pp on d1m, d5m and d15m, which the modifiers'
//! coefficients multiply into the levels.
//!
//! What is not live: the BTC, market, mark-price and price-bug deltas (the report's snapshot —
//! there is no history of them here), and d5s, Pump1h and Dump1h.

use std::fmt;
use std::sync::Arc;

use super::entry::KIND_MOONSHOT;
use super::{Deal, Deltas};
use crate::feed::types::Tick;
use crate::market::kline_cache::KlineCache;
use crate::market::trade_replay::Coverage;

/// The core's refresh step of the coin deltas, milliseconds — see the module doc. The track's
/// points sit on multiples of it, so a caller can tell two moments of one value apart by
/// `t_ms.div_euclid(STEP_MS)` alone.
pub const STEP_MS: i64 = 5_000;

const MINUTE_MS: i64 = 60_000;

/// How far a window over the core's candles reaches past its name: one candle.
const CANDLE_MS: i64 = 5 * MINUTE_MS;

/// The longest hole between two bars still read as continuous history. A venue may skip a
/// minute nobody traded in; a hole a candle wide is where the history really stops.
const MAX_BAR_GAP_MS: i64 = CANDLE_MS;

/// How far before a window the widest delta reaches: "d24h", twenty-five hours of candles plus
/// one.
pub const LOOKBACK_MS: i64 = 25 * 60 * MINUTE_MS + CANDLE_MS;

/// The coin deltas the track re-evaluates, per cent, as `orders_rep` names them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CoinDeltas {
    pub d1m: f64,
    pub d5m: f64,
    pub d15m: f64,
    pub d1h: f64,
    pub d3h: f64,
    pub d24h: f64,
}

impl CoinDeltas {
    fn of(d: &Deltas) -> Self {
        Self {
            d1m: d.d1m,
            d5m: d.d5m,
            d15m: d.d15m,
            d1h: d.d1h,
            d3h: d.d3h,
            d24h: d.d24h,
        }
    }

    fn fields_mut(&mut self) -> [&mut f64; 6] {
        [
            &mut self.d1m,
            &mut self.d5m,
            &mut self.d15m,
            &mut self.d1h,
            &mut self.d3h,
            &mut self.d24h,
        ]
    }

    fn fields(&self) -> [f64; 6] {
        [self.d1m, self.d5m, self.d15m, self.d1h, self.d3h, self.d24h]
    }
}

/// The ranges the core keeps, in the order the evaluation walks them. "d3h" and "d24h" are not
/// windows of their own: each is the wider range, never under d1h.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Window {
    M1,
    M5,
    M15,
    H1,
    H4,
    H25,
}

const WINDOWS: [Window; 6] = [
    Window::M1,
    Window::M5,
    Window::M15,
    Window::H1,
    Window::H4,
    Window::H25,
];

impl Window {
    /// How far back from an evaluation moment the window takes what printed. The short ones are
    /// whole five-second buckets, so their reach is their name; a window over the candles
    /// reaches one candle further (the module doc; measured on 86 trades, d15m's median error
    /// fell from 0.21 pp to 0.02 pp with the extra candle).
    fn reach_ms(self) -> i64 {
        match self {
            Self::M1 => MINUTE_MS,
            Self::M5 => 5 * MINUTE_MS,
            Self::M15 => 15 * MINUTE_MS + CANDLE_MS,
            Self::H1 => 60 * MINUTE_MS + CANDLE_MS,
            Self::H4 => 4 * 60 * MINUTE_MS + CANDLE_MS,
            Self::H25 => LOOKBACK_MS,
        }
    }
}

/// One bar of history: the extremes printed over `[from_ms, to_ms)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub from_ms: i64,
    pub to_ms: i64,
    pub high: f64,
    pub low: f64,
}

/// What printed over a stretch — a bar, or one print — for the sliding windows.
#[derive(Clone, Copy, Debug)]
struct Item {
    /// Exclusive end: the item is complete, and joins a window, at a boundary not before it.
    end_ms: i64,
    high: f64,
    low: f64,
}

/// The track over one covered stretch of the tape.
#[derive(Clone, Debug, PartialEq)]
struct Segment {
    /// The first evaluation moment, a multiple of [`STEP_MS`]; point `i` holds from
    /// `first_ms + i · STEP_MS` for one step.
    first_ms: i64,
    values: Vec<CoinDeltas>,
    /// Per field of [`CoinDeltas`]: whether the history reached back far enough for it to be
    /// live here. A field that is not keeps the report's snapshot.
    live: [bool; 6],
}

impl Segment {
    fn point(&self, t_ms: i64) -> Option<&CoinDeltas> {
        if t_ms < self.first_ms {
            return None;
        }
        self.values
            .get(usize::try_from((t_ms - self.first_ms).div_euclid(STEP_MS)).ok()?)
    }
}

/// The coin deltas of one trade along its window — see the module doc. Built once per trade by
/// the caller that holds its tape ([`track_for`]) and read by the models at every moment they
/// place something ([`Deal::deltas_at`]).
#[derive(Clone, PartialEq)]
pub struct DeltaTrack {
    segments: Vec<Segment>,
}

impl fmt::Debug for DeltaTrack {
    /// A summary: the points themselves are thousands of numbers nobody reads in a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let points: usize = self.segments.iter().map(|s| s.values.len()).sum();
        f.debug_struct("DeltaTrack")
            .field("segments", &self.segments.len())
            .field("points", &points)
            .finish()
    }
}

impl DeltaTrack {
    /// Evaluate the coin deltas over every covered stretch of the tape.
    ///
    /// Args:
    ///     bars: History bars, ascending by start. A bar that overlaps the tape's coverage is
    ///         left out — the prints there are the truth, and a bar would carry prints from after
    ///         the moment it is read at.
    ///     ticks: The tape, ascending.
    ///     covered: The spans the tape covers, ascending; the track is evaluated inside them.
    ///     eval: The stretch the models read deltas over (see [`eval_span`]); the covered spans
    ///         are clipped to it, which is what bounds the track's size on a wide margin.
    ///     anchor: The report's snapshot and the moment it was stamped ([`snapshot_ms`]); the
    ///         track is shifted to equal it there. `None` leaves the evaluation as it is.
    ///
    /// Returns:
    ///     The track, or `None` when no field is live anywhere — the caller keeps the snapshot.
    pub fn build(
        bars: &[Bar],
        ticks: &[Tick],
        covered: &[(i64, i64)],
        eval: (i64, i64),
        anchor: Option<(i64, &Deltas)>,
    ) -> Option<Self> {
        let overlaps_tape = |bar: &Bar| {
            covered
                .iter()
                .any(|&(from, to)| bar.from_ms < to && bar.to_ms > from)
        };
        let history: Vec<Bar> = bars
            .iter()
            .filter(|b| b.low > 0.0 && b.high >= b.low && b.to_ms > b.from_ms && !overlaps_tape(b))
            .copied()
            .collect();
        let mut items: Vec<Item> = history
            .iter()
            .map(|b| Item {
                end_ms: b.to_ms,
                high: b.high,
                low: b.low,
            })
            .collect();
        items.extend(ticks.iter().filter_map(|t| {
            let price = f64::from(t.price);
            (price.is_finite() && price > 0.0).then_some(Item {
                end_ms: t.time_ms as i64 + 1,
                high: price,
                low: price,
            })
        }));
        items.sort_by_key(|i| i.end_ms);
        let continuous = continuous_spans(&history, covered);

        let mut windows = WINDOWS.map(|w| Extremes::new(w.reach_ms()));
        let mut next = 0usize;
        let mut segments = Vec::new();
        for &(span_from, span_to) in covered {
            let (from, to) = (span_from.max(eval.0), span_to.min(eval.1));
            let first_ms = ceil_step(from);
            if first_ms > to {
                continue;
            }
            // How far the history reaches back unbroken from this stretch.
            let history_from = continuous
                .iter()
                .find(|&&(a, b)| a <= span_from && span_to <= b)
                .map_or(span_from, |&(a, _)| a);
            let reaches = WINDOWS.map(|w| history_from <= first_ms - w.reach_ms());
            let [m1, m5, m15, h1, h4, h25] = reaches;
            let live = [m1, m5, m15, h1, h1 && h4, h1 && h25];
            let mut values = Vec::new();
            let mut at = first_ms;
            while at <= to {
                while next < items.len() && items[next].end_ms <= at {
                    for window in &mut windows {
                        window.push(next, &items);
                    }
                    next += 1;
                }
                let [r1, r5, r15, rh1, rh4, rh25] =
                    windows.each_mut().map(|w| w.range_at(at, &items));
                values.push(CoinDeltas {
                    d1m: r1,
                    d5m: r5,
                    d15m: r15,
                    d1h: rh1,
                    d3h: rh1.max(rh4),
                    d24h: rh1.max(rh25),
                });
                at += STEP_MS;
            }
            segments.push(Segment {
                first_ms,
                values,
                live,
            });
        }
        let mut track = Self { segments };
        if let Some((at, snapshot)) = anchor {
            // An anchor asked for and not found — a hole in the tape at the stamp — is no track:
            // the evaluation alone is not the core's number (see `track_for`).
            if !track.anchor(at, &CoinDeltas::of(snapshot)) {
                return None;
            }
        }
        track
            .segments
            .iter()
            .any(|s| s.live.iter().any(|&l| l))
            .then_some(track)
    }

    /// Shift every live field by what separates the evaluation from the report's snapshot at
    /// the moment it was stamped. A field the report holds at exactly zero is not the core's
    /// range — no market holds still for an hour — but a field it never filled, and it is not
    /// made live: the model keeps the report's zero, as it did before the track.
    ///
    /// Returns whether the track reaches the stamp at all; nothing is shifted when it does not.
    /// A field is left live only where it was live AT the stamp as well: an offset read off a
    /// field the history did not reach there is no offset.
    fn anchor(&mut self, at: i64, snapshot: &CoinDeltas) -> bool {
        let Some((estimate, live_at_stamp)) = self.at(at) else {
            return false;
        };
        let snap = snapshot.fields();
        let est = estimate.fields();
        for segment in &mut self.segments {
            for (field, live) in segment.live.iter_mut().enumerate() {
                *live = *live && live_at_stamp[field] && snap[field] != 0.0;
            }
            for value in &mut segment.values {
                for (field, slot) in value.fields_mut().into_iter().enumerate() {
                    *slot = (*slot + snap[field] - est[field]).max(0.0);
                }
            }
        }
        true
    }

    /// The deltas at a moment: the snapshot with every field this track has live there
    /// replaced. Outside the covered stretches, the snapshot as it is.
    pub fn apply(&self, t_ms: i64, snapshot: &Deltas) -> Deltas {
        let Some((values, live)) = self.at(t_ms) else {
            return *snapshot;
        };
        let pick = |field: usize, own: f64, snap: f64| if live[field] { own } else { snap };
        Deltas {
            d1m: pick(0, values.d1m, snapshot.d1m),
            d5m: pick(1, values.d5m, snapshot.d5m),
            d15m: pick(2, values.d15m, snapshot.d15m),
            d1h: pick(3, values.d1h, snapshot.d1h),
            d3h: pick(4, values.d3h, snapshot.d3h),
            d24h: pick(5, values.d24h, snapshot.d24h),
            ..*snapshot
        }
    }

    /// The evaluated coin deltas at a moment and which of them are live there, or `None`
    /// outside the covered stretches.
    pub fn at(&self, t_ms: i64) -> Option<(CoinDeltas, [bool; 6])> {
        self.segments
            .iter()
            .find_map(|s| s.point(t_ms).map(|v| (*v, s.live)))
    }
}

/// The sliding extremes of one window: two monotone queues of item indices, the highs
/// decreasing and the lows increasing from the front, so the front of each is the extreme of
/// what the window holds.
struct Extremes {
    reach_ms: i64,
    highs: std::collections::VecDeque<usize>,
    lows: std::collections::VecDeque<usize>,
}

impl Extremes {
    fn new(reach_ms: i64) -> Self {
        Self {
            reach_ms,
            highs: std::collections::VecDeque::new(),
            lows: std::collections::VecDeque::new(),
        }
    }

    fn push(&mut self, index: usize, items: &[Item]) {
        let item = items[index];
        while self
            .highs
            .back()
            .is_some_and(|&i| items[i].high <= item.high)
        {
            self.highs.pop_back();
        }
        self.highs.push_back(index);
        while self.lows.back().is_some_and(|&i| items[i].low >= item.low) {
            self.lows.pop_back();
        }
        self.lows.push_back(index);
    }

    /// The range at a boundary, per cent: what ended inside `(at − reach, at]` — a bar that
    /// began before the window's start but ended inside it counts whole, as the core's candle
    /// does. Zero when the window holds nothing.
    fn range_at(&mut self, at: i64, items: &[Item]) -> f64 {
        let start = at - self.reach_ms;
        while self
            .highs
            .front()
            .is_some_and(|&i| items[i].end_ms <= start)
        {
            self.highs.pop_front();
        }
        while self.lows.front().is_some_and(|&i| items[i].end_ms <= start) {
            self.lows.pop_front();
        }
        match (self.highs.front(), self.lows.front()) {
            (Some(&h), Some(&l)) if items[l].low > 0.0 => {
                (items[h].high / items[l].low - 1.0) * 100.0
            }
            _ => 0.0,
        }
    }
}

/// The stretches of time the history covers without a break: the bars joined across holes up
/// to [`MAX_BAR_GAP_MS`], and the tape's own coverage.
fn continuous_spans(bars: &[Bar], covered: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = bars
        .iter()
        .map(|b| (b.from_ms, b.to_ms))
        .chain(covered.iter().copied())
        .collect();
    spans.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (from, to) in spans {
        match out.last_mut() {
            Some(last) if from <= last.1 + MAX_BAR_GAP_MS => last.1 = last.1.max(to),
            _ => out.push((from, to)),
        }
    }
    out
}

/// The first multiple of [`STEP_MS`] at or after a moment.
fn ceil_step(t_ms: i64) -> i64 {
    t_ms.div_euclid(STEP_MS) * STEP_MS
        + if t_ms.rem_euclid(STEP_MS) == 0 {
            0
        } else {
            STEP_MS
        }
}

/// Where the models read a deal's deltas: from the run-up before its entry order's life began —
/// the creation, or the buy where the report stamps no creation the tape reaches — through the
/// close. An entry model placing its order off an earlier print, or a variant filling after the
/// close, reads the snapshot there.
pub fn eval_span(deal: &Deal) -> (i64, i64) {
    let open = deal.order_open_ms().unwrap_or(deal.buy_ms);
    (open - super::RUN_UP_MS, deal.close_ms)
}

/// When the report stamped a trade's deltas (FAQ :423): at the buy for MoonShot, at the detect
/// and the placement of the buy order for every other kind — its creation stamp, where the
/// report has one. `None` for a creation the report does not stamp.
pub fn snapshot_ms(deal: &Deal) -> Option<i64> {
    if deal.kind == KIND_MOONSHOT {
        Some(deal.buy_ms)
    } else {
        deal.buy_set_ms
    }
}

/// The history bars a track over `covered` reads off the kline cache: the one-minute bars, and
/// the five-minute ones wherever no minute bar lies. A read the cache did not answer in time is
/// an empty history, and the windows it would have fed stay on the snapshot.
///
/// Args:
///     cache: The terminal's kline cache.
///     exchange_key: The cache's exchange key of the deal's core (`"{code}:{dex:08x}"`).
///     market: The market as the core spells it.
///     covered: The tape's coverage the track is evaluated over.
pub fn read_bars(
    cache: &KlineCache,
    exchange_key: &str,
    market: &str,
    covered: &Coverage,
) -> Vec<Bar> {
    let Some((from, to)) = covered.hull() else {
        return Vec::new();
    };
    let from = from - LOOKBACK_MS - CANDLE_MS;
    let read = |kind_min: u32| {
        let span_ms = i64::from(kind_min) * MINUTE_MS;
        cache
            .read_range(exchange_key, market, kind_min, from, to)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.t_open_ms.is_finite())
            .map(|c| {
                let from_ms = c.t_open_ms as i64;
                Bar {
                    from_ms,
                    to_ms: from_ms + span_ms,
                    high: f64::from(c.high),
                    low: f64::from(c.low),
                }
            })
            .collect::<Vec<Bar>>()
    };
    let minutes = read(1);
    let mut bars = minutes.clone();
    bars.extend(read(5).into_iter().filter(|five| {
        !minutes
            .iter()
            .any(|m| m.from_ms >= five.from_ms && m.from_ms < five.to_ms)
    }));
    bars.sort_by_key(|b| b.from_ms);
    bars
}

/// The track of one deal, from its tape and the kline cache — the one call the tuner's table and
/// the `real_data` bench both make, so what the bench measures is what the table replays.
///
/// Args:
///     cache: The terminal's kline cache.
///     exchange_key: The cache's exchange key of the deal's core.
///     market: The market as the core spells it.
///     deal: The trade, for its snapshot and the moment it was stamped.
///     ticks: The tape, ascending.
///     covered: The tape's coverage.
pub fn track_for(
    cache: &KlineCache,
    exchange_key: &str,
    market: &str,
    deal: &Deal,
    ticks: &[Tick],
    covered: &Coverage,
) -> Option<Arc<DeltaTrack>> {
    // Only a track anchored on the report is the core's number: evaluated alone, it placed 89
    // MoonHook takes further from the core's than the snapshot did (median 0.19 pp against
    // 0.09, 2026-09-23), while anchored at the order's creation it placed the 21 stamped ones
    // closer (11 against 7). A trade without a stamp the tape reaches keeps the snapshot.
    let at = snapshot_ms(deal)?;
    let bars = read_bars(cache, exchange_key, market, covered);
    DeltaTrack::build(
        &bars,
        ticks,
        covered.spans(),
        eval_span(deal),
        Some((at, &deal.deltas)),
    )
    .map(Arc::new)
}

#[cfg(test)]
mod tests;
