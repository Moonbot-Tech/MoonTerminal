//! `PeriodExtreme`: the high and/or low of a UTC calendar day or ISO week.
//!
//! "Current" is the period that holds the caller's reference time ([`DetectCtx::reference_ms`]:
//! `now_ms`, never earlier than the last valid bar, or that bar when `now_ms` is unusable), and
//! "previous" the one right before it. So just after UTC midnight, before any bar of the new day
//! has closed, `Day` reports nothing — not yesterday — and `PrevDay` reports yesterday. Periods
//! are cut from `t_open_ms` in UTC: a day is `[d·86 400 000, (d+1)·86 400 000)`, an ISO week starts
//! on Monday 00:00 UTC.
//!
//! Only CLOSED bars are read, so the bar still forming is not in the current period's range: a new
//! high it prints shows up when it closes. The lag is at most one bar of the row's timeframe, which
//! is why the shipped extreme rows run on 5-minute bars.
//!
//! A period the input does not cover from its start is reported as nothing rather than as a
//! partial extreme: when the first valid bar opens after the period began, the bars before it — and
//! possibly the real high or low — are simply missing, and a wrong level drawn with confidence is
//! worse than an honest gap. A previous period's extreme also reports, informationally, the latest
//! later break of it in `broken`; it is never dropped (`DetectorParams::broken_policy` gives this
//! kind the default policy only for the event layer's false-break window). Cost O(n).

use moon_core::config::auto_figures::{
    DetectorKind, DetectorParams, ExtremePeriod, ExtremeWhich, PeriodExtremeParams,
};
use moon_core::market::candles::ChartCandle;

use super::{
    DetectCtx, Detector, Found, FoundShape, Side, Touch, bar_ok, scan_break, touch_volume,
};

/// One UTC day, ms.
pub const DAY_MS: i64 = 86_400_000;

/// The `PeriodExtreme` registry row.
pub struct PeriodExtreme;

impl Detector for PeriodExtreme {
    fn kind(&self) -> DetectorKind {
        DetectorKind::PeriodExtreme
    }

    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found> {
        let DetectorParams::PeriodExtreme(p) = params else {
            return Vec::new();
        };
        let Some(reference_ms) = ctx.reference_ms(bars) else {
            return Vec::new();
        };
        period_extremes(bars, p, reference_ms)
    }
}

/// UTC day number of a moment (days since 1970-01-01), flooring toward the past.
pub fn day_of(t_ms: f64) -> i64 {
    (t_ms / DAY_MS as f64).floor() as i64
}

/// Day number of the Monday that opens the ISO week holding `day`. 1970-01-01 was a Thursday, so
/// a day's weekday counted from Monday is `(day + 3) mod 7`.
pub fn week_start_day(day: i64) -> i64 {
    day - (day + 3).rem_euclid(7)
}

/// The `[start, end)` bounds, ms, of the period `p` names, relative to the reference day.
pub fn period_bounds(period: ExtremePeriod, reference_day: i64) -> (f64, f64) {
    let (start_day, len_days) = match period {
        ExtremePeriod::Day => (reference_day, 1),
        ExtremePeriod::PrevDay => (reference_day - 1, 1),
        ExtremePeriod::Week => (week_start_day(reference_day), 7),
        ExtremePeriod::PrevWeek => (week_start_day(reference_day) - 7, 7),
    };
    let start = (start_day * DAY_MS) as f64;
    (start, start + (len_days * DAY_MS) as f64)
}

/// The extremes `p` asks for, high first.
fn period_extremes(bars: &[ChartCandle], p: &PeriodExtremeParams, reference_ms: f64) -> Vec<Found> {
    let Some(first) = bars.iter().find(|b| bar_ok(b)) else {
        return Vec::new();
    };
    let (start, end) = period_bounds(p.period, day_of(reference_ms));
    if first.t_open_ms > start {
        return Vec::new();
    }
    // (index, bar) of the period's bars, and the extremes among them — the first bar to reach the
    // extreme wins a tie, so the touch marks where the level was set.
    let mut first_in: Option<f64> = None;
    let mut last_in: Option<f64> = None;
    let mut high: Option<(usize, f32)> = None;
    let mut low: Option<(usize, f32)> = None;
    let mut last_idx_in = 0;
    for (i, b) in bars.iter().enumerate() {
        if !bar_ok(b) || b.t_open_ms < start || b.t_open_ms >= end {
            continue;
        }
        first_in.get_or_insert(b.t_open_ms);
        last_in = Some(b.t_open_ms);
        last_idx_in = i;
        if high.is_none_or(|(_, h)| b.high > h) {
            high = Some((i, b.high));
        }
        if low.is_none_or(|(_, l)| b.low < l) {
            low = Some((i, b.low));
        }
    }
    let (Some(first_ms), Some(last_ms)) = (first_in, last_in) else {
        return Vec::new();
    };
    let previous = matches!(p.period, ExtremePeriod::PrevDay | ExtremePeriod::PrevWeek);
    let make = |(idx, price): (usize, f32), origin: Side| -> Found {
        let price = f64::from(price);
        let touches = vec![Touch {
            t_ms: bars[idx].t_open_ms,
            price,
        }];
        Found {
            shape: FoundShape::Horizontal { price },
            // The one bar that set the extreme is its one touch.
            touch_volume: touch_volume(bars, &touches),
            touches,
            // A period's extreme is a price, not a cluster: there is no band.
            tolerance_frac: 0.0,
            strength: 1.0,
            first_ms,
            last_ms,
            broken: if previous {
                scan_break(bars, last_idx_in, origin, |_| Some(price))
            } else {
                None
            },
        }
    };
    let mut out = Vec::with_capacity(2);
    if matches!(p.which, ExtremeWhich::High | ExtremeWhich::Both) {
        if let Some(h) = high {
            out.push(make(h, Side::Below));
        }
    }
    if matches!(p.which, ExtremeWhich::Low | ExtremeWhich::Both) {
        if let Some(l) = low {
            out.push(make(l, Side::Above));
        }
    }
    out
}

#[cfg(test)]
mod tests;
