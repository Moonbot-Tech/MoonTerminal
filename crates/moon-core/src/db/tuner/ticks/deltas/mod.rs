//! Live deltas — the report's deltas re-evaluated along a trade's window the way the core
//! evaluates them, where the report keeps ONE snapshot per trade. One place computes them for
//! every consumer: MoonShot's `MShotAdd*` corridor and every kind's `Add*` sell and stop read the
//! same values through [`Deal::deltas_at`].
//!
//! What the core computes (`docs-internal/STRATEGY_FORMULAS/deltas.md`: the FAQ, `data/faqru.tsv`
//! :1052, :1068, :1073, and moonproto's parity port of the core, `state/history_store/derived.rs`
//! and `state/markets/prices.rs`):
//!
//! - every coin delta is a RANGE, `(max / min − 1) · 100` over a window — never negative;
//! - the long ones run over the core's five-minute candles, stamped at their END and kept while
//!   younger than the window, so a window reaches up to one candle past its name: d15m and d1h
//!   over the closed candles and the open one; "d3h" and "d24h" over CLOSED candles younger than
//!   four hours and twenty-five (the FAQ counts the first "3ч55м"; the core developer,
//!   2026-09-23), both never under d1h — the open candle reaches them only through it (see
//!   `coin::REACH` for the phase and what it measured);
//! - the short ones, d1m and d5m, run over five-second buckets of trades;
//! - the core refreshes them on those five-second buckets. A value holds from one bucket boundary
//!   to the next and is computed over what printed BEFORE the boundary: the MoonShot report's
//!   d1m is the range of the prints up to the last completed bucket before the fill (12 exact
//!   matches of 12 found, 2026-09-23), never the range at the fill itself;
//! - d5s, Pump1h and Dump1h ([`coin`]) and the BTC deltas ([`btc`]) have their own rules.
//!
//! Inputs: the tape where it is held, and whatever bars the kline cache holds — the minute bars
//! the tuner's candle stage keeps (six hours before every window it fetched), the recorder's
//! five-minute ones wherever no minute bar lies, for the deal's market and for BTC's on the same
//! exchange. A window is read over what history it has, however much that is — twenty-three
//! hours of a twenty-five-hour window, or its first and last bars across a hole — and the share
//! it covered is kept for the summary ([`StampCheck`], [`quality`]).
//!
//! Anchored to the report: at the moment the report stamped its deltas ([`snapshot_ms`]) the track
//! equals the report exactly, and elsewhere it moves by what the tape and the bars moved. The
//! offset absorbs what those inputs cannot see — the phase of the core's candles, the holes in the
//! history, a core that follows the averaged price instead of raw trades (its `DeltasByTrades`
//! switch off). Without the anchor there is no track ([`track_for`]): evaluated alone over the
//! whole replica (2026-09-23, 1 140 trades) the bars and the tape met the report's snapshot to a
//! median of 0.03 pp on d1h, d3h and d24h but 0.15–0.16 pp on d1m, d5m and d15m, and placed 89
//! MoonHook takes further from the core's than the snapshot did (median 0.19 pp against 0.09).
//!
//! What is not re-evaluated, and stays the snapshot: the mark-price, price-bug and market-wide
//! deltas ([`NotComputed`]).

use std::fmt;
use std::sync::Arc;

use super::entry::KIND_MOONSHOT;
use super::{Deal, Deltas};
use crate::feed::types::Tick;
use crate::market::kline_cache::KlineCache;
use crate::market::trade_replay::Coverage;

mod btc;
mod coin;
pub mod field;
pub mod quality;
mod series;

pub use field::{DeltaField, NotComputed};
pub use quality::{DeltaQuality, FieldQuality, summarize};

/// The core's refresh step of the deltas, milliseconds — see the module doc. The track's points
/// sit on multiples of it, so a caller can tell two moments of one value apart by
/// `t_ms.div_euclid(STEP_MS)` alone.
pub const STEP_MS: i64 = 5_000;

const MINUTE_MS: i64 = 60_000;

/// How far a window over the core's candles reaches past its name: one candle.
const CANDLE_MS: i64 = 5 * MINUTE_MS;

/// How far before a window the widest coin delta reaches: "d24h", twenty-five hours of closed
/// candles counted back from the last close, which lies up to one candle before the moment.
pub const LOOKBACK_MS: i64 = 25 * 60 * MINUTE_MS + CANDLE_MS;

/// How far before a window BTC's history is read: the hour average is re-seeded off the last
/// hour's candles at every close (see `btc`), and the five-minute range looks back five minutes,
/// so an hour and a candle is what any moment needs; four hours leave room for a window that
/// opens on a hole in the history.
const BTC_LOOKBACK_MS: i64 = 4 * 60 * MINUTE_MS;

/// One bar of history: what printed over `[from_ms, to_ms)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub from_ms: i64,
    pub to_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// Every field at one point, in [`DeltaField::ALL`]'s order; `NaN` where the history had nothing.
type Values = [f64; DeltaField::COUNT];

/// The track over one covered stretch of the tape.
#[derive(Clone, Debug, PartialEq)]
struct Segment {
    /// The first evaluation moment, a multiple of [`STEP_MS`]; point `i` holds from
    /// `first_ms + i · STEP_MS` for one step.
    first_ms: i64,
    values: Vec<Values>,
}

impl Segment {
    fn point(&self, t_ms: i64) -> Option<&Values> {
        if t_ms < self.first_ms {
            return None;
        }
        self.values
            .get(usize::try_from((t_ms - self.first_ms).div_euclid(STEP_MS)).ok()?)
    }
}

/// What the evaluation found at the report's stamp, per field of [`DeltaField::ALL`], BEFORE the
/// anchor moved it onto the report — the measure of how well the history reproduces the core.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StampCheck {
    /// The share of each field's window the history covered at the stamp, 0 … 1.
    pub coverage: [f64; DeltaField::COUNT],
    /// `evaluation − report` at the stamp, per cent points — positive where the history saw a
    /// wider move than the core; `None` where the history had nothing, or the report never
    /// filled the field.
    pub error: [Option<f64>; DeltaField::COUNT],
}

/// The deltas of one trade along its window — see the module doc. Built once per trade by the
/// caller that holds its tape ([`track_for`]) and read by the models at every moment they place
/// something ([`Deal::deltas_at`]).
#[derive(Clone, PartialEq)]
pub struct DeltaTrack {
    segments: Vec<Segment>,
    /// Per field: whether the track answers for it. One that does not keeps the snapshot.
    live: [bool; DeltaField::COUNT],
    stamp: StampCheck,
}

impl fmt::Debug for DeltaTrack {
    /// A summary: the points themselves are thousands of numbers nobody reads in a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let points: usize = self.segments.iter().map(|s| s.values.len()).sum();
        f.debug_struct("DeltaTrack")
            .field("segments", &self.segments.len())
            .field("points", &points)
            .field("live", &self.live)
            .finish()
    }
}

/// What a track is built from.
#[derive(Clone, Copy, Debug)]
pub struct TrackInputs<'a> {
    /// The deal's market's bars, ascending. One overlapping the tape's coverage is left out — the
    /// prints there are the truth, and a bar would carry prints from after the moment it is read
    /// at.
    pub coin_bars: &'a [Bar],
    /// The tape, ascending.
    pub ticks: &'a [Tick],
    /// The spans the tape covers, ascending; the track is evaluated inside them.
    pub covered: &'a [(i64, i64)],
    /// The bars of BTC's market on the same exchange, ascending; empty leaves the BTC fields on
    /// the snapshot.
    pub btc_bars: &'a [Bar],
    /// The stretch the models read deltas over ([`eval_span`]); the covered spans are clipped to
    /// it, which is what bounds the track's size on a wide margin.
    pub eval: (i64, i64),
    /// The report's snapshot and the moment it was stamped ([`snapshot_ms`]); the track is
    /// shifted to equal it there. `None` leaves the evaluation as it is — for the tests of the
    /// windows themselves.
    pub anchor: Option<(i64, &'a Deltas)>,
}

impl DeltaTrack {
    /// Evaluate every field over every covered stretch of the tape, and anchor it.
    ///
    /// Returns:
    ///     The track, or `None` when an anchor was asked for and the track does not reach its
    ///     moment — the evaluation alone is not the core's number — or when no field answers.
    pub fn build(inputs: TrackInputs<'_>) -> Option<Self> {
        let coin_series = series::Series::new(inputs.coin_bars, inputs.ticks, inputs.covered);
        let btc_series = series::Series::new(inputs.btc_bars, &[], &[]);
        let mut coin_eval = coin::CoinEval::new(&coin_series);
        let mut btc_eval = btc::BtcEval::new(&btc_series);
        let stamp_at = inputs
            .anchor
            .map(|(at, _)| at.div_euclid(STEP_MS) * STEP_MS);
        let mut at_stamp: Option<(Values, [f64; DeltaField::COUNT])> = None;
        let mut segments = Vec::new();
        for &(span_from, span_to) in inputs.covered {
            let (from, to) = (span_from.max(inputs.eval.0), span_to.min(inputs.eval.1));
            let first_ms = ceil_step(from);
            if first_ms > to {
                continue;
            }
            let mut values = Vec::new();
            let mut at = first_ms;
            while at <= to {
                let mut point = [f64::NAN; DeltaField::COUNT];
                let mut coverage = [0.0; DeltaField::COUNT];
                let coin = coin_eval.at(at);
                let btc = btc_eval.at(at);
                let fields = coin::FIELDS
                    .iter()
                    .zip(coin)
                    .chain(btc::FIELDS.iter().zip(btc));
                for (field, (value, covered)) in fields {
                    point[field.index()] = value.unwrap_or(f64::NAN);
                    coverage[field.index()] = covered;
                }
                if stamp_at == Some(at) {
                    at_stamp = Some((point, coverage));
                }
                values.push(point);
                at += STEP_MS;
            }
            segments.push(Segment { first_ms, values });
        }
        let mut track = Self {
            segments,
            live: [false; DeltaField::COUNT],
            stamp: StampCheck::default(),
        };
        match inputs.anchor {
            None => {
                for field in DeltaField::ALL {
                    track.live[field.index()] = track
                        .segments
                        .iter()
                        .any(|s| s.values.iter().any(|v| v[field.index()].is_finite()));
                }
            }
            Some((_, snapshot)) => {
                let (estimate, coverage) = at_stamp?;
                track.anchor(snapshot, &estimate, &coverage);
            }
        }
        track.live.iter().any(|&l| l).then_some(track)
    }

    /// Put every field the evaluation has at the stamp onto the report: shift it by what separates
    /// the two there. A field the report holds at exactly zero where zero means "never filled"
    /// ([`DeltaField::zero_is_unfilled`]) is not made live — the model keeps the report's zero, as
    /// it did before the track.
    fn anchor(
        &mut self,
        snapshot: &Deltas,
        estimate: &Values,
        coverage: &[f64; DeltaField::COUNT],
    ) {
        let mut offset = [0.0; DeltaField::COUNT];
        for field in DeltaField::ALL {
            let i = field.index();
            let report = field.of(snapshot);
            let unfilled = report == 0.0 && field.zero_is_unfilled();
            self.stamp.coverage[i] = coverage[i];
            if !estimate[i].is_finite() || unfilled {
                continue;
            }
            self.stamp.error[i] = Some(estimate[i] - report);
            self.live[i] = true;
            offset[i] = report - estimate[i];
        }
        for segment in &mut self.segments {
            for point in &mut segment.values {
                for field in DeltaField::ALL {
                    let i = field.index();
                    let shifted = point[i] + offset[i];
                    // BTC's hour deviation carries a sign; every other field is a size.
                    point[i] = if field == DeltaField::Btc1h {
                        shifted
                    } else {
                        shifted.max(0.0)
                    };
                }
            }
        }
    }

    /// The deltas at a moment: the snapshot with every field this track answers for there
    /// replaced. Outside the covered stretches, the snapshot as it is.
    pub fn apply(&self, t_ms: i64, snapshot: &Deltas) -> Deltas {
        let mut out = *snapshot;
        if let Some(point) = self.segments.iter().find_map(|s| s.point(t_ms)) {
            for field in DeltaField::ALL {
                let value = point[field.index()];
                if self.live[field.index()] && value.is_finite() {
                    field.set(&mut out, value);
                }
            }
        }
        out
    }

    /// One field's value at a moment, or `None` where the track does not answer for it.
    pub fn value(&self, t_ms: i64, field: DeltaField) -> Option<f64> {
        let point = self.segments.iter().find_map(|s| s.point(t_ms))?;
        let value = point[field.index()];
        (self.live[field.index()] && value.is_finite()).then_some(value)
    }

    /// Whether the track answers for a field at all.
    pub fn is_live(&self, field: DeltaField) -> bool {
        self.live[field.index()]
    }

    /// What the evaluation found at the report's stamp, before the anchor.
    pub fn stamp(&self) -> &StampCheck {
        &self.stamp
    }
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

/// A market's history bars over `[from_ms, to_ms]` off the kline cache: the one-minute bars, and
/// the five-minute ones wherever no minute bar lies. A read the cache did not answer in time is
/// an empty history, and the fields it would have fed stay on the snapshot.
///
/// Args:
///     cache: The terminal's kline cache.
///     exchange_key: The cache's exchange key of the deal's core (`"{code}:{dex:08x}"`).
///     market: The market as the core spells it.
///     from_ms: The earliest moment wanted.
///     to_ms: The latest.
pub fn read_bars(
    cache: &KlineCache,
    exchange_key: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> Vec<Bar> {
    let read = |kind_min: u32| {
        let span_ms = i64::from(kind_min) * MINUTE_MS;
        cache
            .read_range(exchange_key, market, kind_min, from_ms, to_ms)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.t_open_ms.is_finite())
            .map(|c| {
                let from_ms = c.t_open_ms as i64;
                Bar {
                    from_ms,
                    to_ms: from_ms + span_ms,
                    open: f64::from(c.open),
                    high: f64::from(c.high),
                    low: f64::from(c.low),
                    close: f64::from(c.close),
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
///     market: The deal's market as the core spells it.
///     btc_market: BTC's market on the same exchange, when the catalog names one; `None` leaves
///         the BTC fields on the snapshot.
///     deal: The trade, for its snapshot and the moment it was stamped.
///     ticks: The tape, ascending.
///     covered: The tape's coverage.
pub fn track_for(
    cache: &KlineCache,
    exchange_key: &str,
    market: &str,
    btc_market: Option<&str>,
    deal: &Deal,
    ticks: &[Tick],
    covered: &Coverage,
) -> Option<Arc<DeltaTrack>> {
    // Only a track anchored on the report is the core's number (see the module doc): a trade
    // without a stamp the tape reaches keeps the snapshot.
    let at = snapshot_ms(deal)?;
    let (from, to) = covered.hull()?;
    let eval = eval_span(deal);
    let coin_bars = read_bars(
        cache,
        exchange_key,
        market,
        from - LOOKBACK_MS - CANDLE_MS,
        to,
    );
    // A deal on BTC's own market reads BTC off its own bars.
    let btc_bars = match btc_market {
        Some(btc) if btc == market => coin_bars
            .iter()
            .filter(|b| b.to_ms > eval.0 - BTC_LOOKBACK_MS)
            .copied()
            .collect(),
        Some(btc) => read_bars(cache, exchange_key, btc, eval.0 - BTC_LOOKBACK_MS, eval.1),
        None => Vec::new(),
    };
    DeltaTrack::build(TrackInputs {
        coin_bars: &coin_bars,
        ticks,
        covered: covered.spans(),
        btc_bars: &btc_bars,
        eval,
        anchor: Some((at, &deal.deltas)),
    })
    .map(Arc::new)
}

#[cfg(test)]
mod tests;
