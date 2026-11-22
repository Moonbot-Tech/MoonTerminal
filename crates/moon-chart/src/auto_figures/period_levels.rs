//! `PeriodLevel`: the opening and closing prices of UTC calendar periods — the previous day's
//! close, the day's open, the week's open, the previous week's close.
//!
//! Days and ISO weeks are cut exactly as `extremes` cuts them ([`day_of`], [`week_start_day`]),
//! relative to the caller's reference time ([`DetectCtx::reference_ms`]). Trading sessions and an
//! opening range are deliberately not here (LinKvo, 19.11.2026: crypto trades around the clock).
//!
//! The 5-minute cache these bars come from is written only while the terminal runs, so a period's
//! first or last bar is often simply missing. A price taken from the bar NEXT to a hole is not the
//! period's open or close, and a wrong level drawn with confidence is worse than none. A price
//! needs only its own edge bar, not the rest of the period — the edge rules are the ones every
//! calendar detector shares ([`super::coverage`]):
//! - an OPEN needs the period's first bar — a valid bar opening within one bar length
//!   (`DetectCtx::tf_ms`) of the period's start;
//! - a CLOSE needs the period's last bar — a valid bar of the period whose own close, one bar
//!   length after its open, reaches the period's end.
//!
//! Without a usable bar length (`tf_ms <= 0`) neither can be checked and nothing is found.
//!
//! The lines are facts of the calendar: never dropped, and never reported as broken (an open or a
//! close has no side it "holds" the way a high or a pivot level does; the event layer still reads
//! breaks from the bars). Strength ranks a week's price over a day's — the longer period's price
//! is the one more orders remember. Cost O(n) per enabled flag.

use moon_core::config::auto_figures::{DetectorKind, DetectorParams, PeriodLevelParams};
use moon_core::market::candles::ChartCandle;

use super::coverage::{closes_at, opens_at};
use super::extremes::{DAY_MS, day_of, week_start_day};
use super::{
    DetectCtx, Detector, Found, FoundShape, LineTag, Touch, bar_ok, positive, touch_volume,
};

/// The `PeriodLevel` registry row.
pub struct PeriodLevel;

impl Detector for PeriodLevel {
    fn kind(&self) -> DetectorKind {
        DetectorKind::PeriodLevel
    }

    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found> {
        let DetectorParams::PeriodLevel(p) = params else {
            return Vec::new();
        };
        if ctx.tf_ms <= 0 {
            return Vec::new();
        }
        let Some(reference_ms) = ctx.reference_ms(bars) else {
            return Vec::new();
        };
        period_levels(bars, p, reference_ms, ctx.tf_ms as f64)
    }
}

/// Start of UTC day `day`, ms.
pub(crate) fn day_start_ms(day: i64) -> f64 {
    (day * DAY_MS) as f64
}

/// Index of the period's FIRST bar: the first valid bar opening at or after `start`, when it opens
/// within `tf` of it. `None` when the period's start is missing from the input.
fn first_bar_at(bars: &[ChartCandle], start: f64, tf: f64) -> Option<usize> {
    let i = bars
        .iter()
        .position(|b| bar_ok(b) && b.t_open_ms >= start)?;
    opens_at(bars[i].t_open_ms, start, tf).then_some(i)
}

/// Index of the period's LAST bar: the last valid bar of `[start, end)`, when its close (`tf` after
/// its open) reaches `end`. `None` when the period's end is missing from the input.
fn last_bar_at(bars: &[ChartCandle], start: f64, end: f64, tf: f64) -> Option<usize> {
    let i = bars
        .iter()
        .rposition(|b| bar_ok(b) && b.t_open_ms >= start && b.t_open_ms < end)?;
    closes_at(bars[i].t_open_ms, end, tf).then_some(i)
}

/// The levels `p` asks for, in flag order: previous day's close, day's open, week's open, previous
/// week's close.
fn period_levels(
    bars: &[ChartCandle],
    p: &PeriodLevelParams,
    reference_ms: f64,
    tf: f64,
) -> Vec<Found> {
    let day = day_of(reference_ms);
    let week = week_start_day(day);
    // (index of the bar the price comes from, read its close?, tag, strength)
    let mut picks: Vec<(usize, bool, LineTag, f32)> = Vec::with_capacity(4);
    if p.prev_day_close {
        if let Some(i) = last_bar_at(bars, day_start_ms(day - 1), day_start_ms(day), tf) {
            picks.push((i, true, LineTag::PrevDayClose, 1.0));
        }
    }
    if p.day_open {
        if let Some(i) = first_bar_at(bars, day_start_ms(day), tf) {
            picks.push((i, false, LineTag::DayOpen, 1.0));
        }
    }
    if p.week_open {
        if let Some(i) = first_bar_at(bars, day_start_ms(week), tf) {
            picks.push((i, false, LineTag::WeekOpen, 2.0));
        }
    }
    if p.prev_week_close {
        if let Some(i) = last_bar_at(bars, day_start_ms(week - 7), day_start_ms(week), tf) {
            picks.push((i, true, LineTag::PrevWeekClose, 2.0));
        }
    }
    picks
        .into_iter()
        .filter_map(|(i, close, tag, strength)| {
            let b = &bars[i];
            // A ring bar's open and close are unknown (`NaN`, see `super`): the level is that
            // bar's own price, which no other bar's stands in for, so it is not found.
            let price = positive(f64::from(if close { b.close } else { b.open }))?;
            let touches = vec![Touch {
                t_ms: b.t_open_ms,
                price,
            }];
            Some(Found {
                shape: FoundShape::Horizontal { price },
                // The bar the price was read from is the line's one touch and its anchor.
                touch_volume: touch_volume(bars, &touches),
                touches,
                strength,
                first_ms: b.t_open_ms,
                last_ms: b.t_open_ms,
                broken: None,
                // A calendar price is a point, not a cluster: no band.
                tolerance_frac: 0.0,
                tag: Some(tag),
                origin: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
