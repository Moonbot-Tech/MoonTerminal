//! `VolumeProfile`: the point of control and the value-area edges of a UTC day's volume profile,
//! and the "naked" POCs of past days price has not traded at since.
//!
//! # The profile
//!
//! The day's range `[lowest low, highest high]` is cut into `bins` equal bins, and each valid bar's
//! `quote_volume` is spread uniformly over its own `[low, high]`: every bin gets the share of the
//! volume its overlap with the bar's range is of that range (a bar with `high == low` puts all of
//! it into one bin). A bar with no turnover adds nothing; a day whose total is zero has no profile
//! and draws nothing.
//!
//! - **POC** — the centre of the bin with the most volume. A tie goes to the bin whose centre is
//!   nearest the day's last close (where the day settled is the price the tie is about), and an
//!   equal distance to the LOWER bin — a fixed rule, so the same bars always give the same POC.
//! - **Value area** — grown from the POC bin one bin at a time toward the neighbour with more
//!   volume (a tie goes to the lower side; a side already at the range's edge is not chosen),
//!   until it holds
//!   `value_area_pct` of the day's volume. **VAH** is the upper edge of its top bin, **VAL** the
//!   lower edge of its bottom bin.
//! - **Naked POC** — for each of the `naked_poc_days` complete UTC days before the reference day,
//!   that day's POC when no valid bar AFTER the day has traded at it (`low <= poc <= high`).
//!
//! # Holes
//!
//! The local 5-minute cache is written only while the terminal runs, so a day with a hole is
//! normal, and a profile of part of a day is a different profile — its POC can sit at a price the
//! missing hours never cared about. A day is profiled only when it is COVERED — the check every
//! calendar detector shares ([`super::coverage`]): no empty stretch longer than
//! [`MAX_GAP_MS`](super::coverage::MAX_GAP_MS) from its start to its first bar, between
//! neighbouring bars, or from its last bar's close to its end (the current day's: to the reference
//! time) — a quiet market's short stretches without a trade are not holes, hours in a row are. A naked POC additionally needs the span AFTER its day, up to
//! the reference time, free of such a stretch ([`span_covered`]): "nothing traded at it since" is
//! not a claim a hole of a day can support.
//!
//! Each line reports the volume its bin holds as [`Found::touch_volume`] (what
//! `StrengthBasis::Volume` ranks by), that volume's share of its day as `strength`, and the bin's
//! width as `tolerance_frac`, so a zone band shows the bin.
//!
//! # Why not `moon_core`'s profile
//!
//! `moon_core::market::synthetic_profile` spreads candles the same way, but it is the bench's
//! stand-in for the trade-built `HVol` profile: its grid is anchored at price zero with a width the
//! caller picks, and its rows are `f32`. A day's profile needs `bins` bins over the day's own range
//! and `f64` edges (a sub-cent coin's VAH must not move in the last digit), so the spread is done
//! here; `moon_chart::hvol` only selects and scales `HVol` rows and builds none.
//!
//! Cost: O(bars of the day · bins they cover) per profiled day, plus O(n) per naked day for the
//! "traded since" scan — at most 11 days of at most 500 bins.

use moon_core::config::auto_figures::{
    DetectorKind, DetectorParams, ProfilePeriod, VolumeProfileParams,
};
use moon_core::market::candles::ChartCandle;

use super::coverage::{covered, span_covered};
use super::extremes::day_of;
use super::period_levels::day_start_ms;
use super::{DetectCtx, Detector, Found, FoundShape, LineTag, bar_ok, positive};

/// The `VolumeProfile` registry row.
pub struct VolumeProfile;

impl Detector for VolumeProfile {
    fn kind(&self) -> DetectorKind {
        DetectorKind::VolumeProfile
    }

    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found> {
        let DetectorParams::VolumeProfile(p) = params else {
            return Vec::new();
        };
        if ctx.tf_ms <= 0 {
            return Vec::new();
        }
        let Some(reference_ms) = ctx.reference_ms(bars) else {
            return Vec::new();
        };
        profile_lines(bars, p, reference_ms, ctx.tf_ms as f64)
    }
}

/// One day's profile.
struct Profile {
    /// Lower edge of bin 0.
    lo: f64,
    /// Bin width; `0` when the day traded at one price only (then there is a single bin).
    width: f64,
    vols: Vec<f64>,
    total: f64,
    /// Open time of the day's first and last bar.
    first_ms: f64,
    last_ms: f64,
}

impl Profile {
    /// Price of bin `i`'s lower edge.
    fn edge(&self, i: usize) -> f64 {
        self.lo + i as f64 * self.width
    }

    /// Price of bin `i`'s centre.
    fn centre(&self, i: usize) -> f64 {
        self.lo + (i as f64 + 0.5) * self.width
    }

    /// The POC bin; see the module doc for the tie rule. `None` for an empty profile.
    fn poc(&self, last_close: f64) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, &v) in self.vols.iter().enumerate() {
            let better = match best {
                None => true,
                Some(b) => {
                    v > self.vols[b]
                        || (v == self.vols[b]
                            && (self.centre(i) - last_close).abs()
                                < (self.centre(b) - last_close).abs())
                }
            };
            if better {
                best = Some(i);
            }
        }
        best
    }

    /// The value area's bottom and top bin, grown from `poc` until it holds `pct` % of the volume.
    fn value_area(&self, poc: usize, pct: f32) -> (usize, usize) {
        let share = if pct.is_finite() {
            f64::from(pct.clamp(0.0, 100.0)) / 100.0
        } else {
            0.0
        };
        let target = self.total * share;
        let (mut lo, mut hi) = (poc, poc);
        let mut held = self.vols[poc];
        let last = self.vols.len() - 1;
        while held < target && (lo > 0 || hi < last) {
            // A missing side reads as -1, below any real (non-negative) bin.
            let up = if hi < last { self.vols[hi + 1] } else { -1.0 };
            let down = if lo > 0 { self.vols[lo - 1] } else { -1.0 };
            if up > down {
                hi += 1;
                held += up;
            } else {
                lo -= 1;
                held += down;
            }
        }
        (lo, hi)
    }
}

/// The valid bars opening in `[start, end)`.
fn bars_in(bars: &[ChartCandle], start: f64, end: f64) -> Vec<&ChartCandle> {
    bars.iter()
        .filter(|b| bar_ok(b) && b.t_open_ms >= start && b.t_open_ms < end)
        .collect()
}

/// Whether `day` (its valid bars, ascending) covers its day from `start` — to `end` when the day is
/// finished, to `reference_ms` when it is the current one ([`covered`]).
fn complete(
    day: &[&ChartCandle],
    start: f64,
    end: Option<f64>,
    reference_ms: f64,
    tf: f64,
) -> bool {
    covered(
        day.iter().map(|b| b.t_open_ms),
        start,
        end,
        reference_ms,
        tf,
    )
}

/// The profile of `day`'s bars over `bins` bins, or `None` when nothing traded.
fn build(day: &[&ChartCandle], bins: u16) -> Option<Profile> {
    let first = day.first()?;
    let last = day.last()?;
    let lo = day
        .iter()
        .map(|b| f64::from(b.low))
        .fold(f64::MAX, f64::min);
    let hi = day
        .iter()
        .map(|b| f64::from(b.high))
        .fold(f64::MIN, f64::max);
    let flat = !(hi > lo);
    let n = if flat { 1 } else { usize::from(bins.max(1)) };
    let width = if flat { 0.0 } else { (hi - lo) / n as f64 };
    let bin_of = |price: f64| -> usize {
        if flat {
            return 0;
        }
        // Clamped: the day's high lands exactly on the top edge.
        (((price - lo) / width).floor().max(0.0) as usize).min(n - 1)
    };
    let mut vols = vec![0.0f64; n];
    for b in day {
        let v = f64::from(b.quote_volume);
        if !(v.is_finite() && v > 0.0) {
            continue;
        }
        let (l, h) = (f64::from(b.low), f64::from(b.high));
        let (i0, i1) = (bin_of(l), bin_of(h));
        if flat || !(h > l) || i0 == i1 {
            vols[i0] += v;
            continue;
        }
        for (i, slot) in vols.iter_mut().enumerate().take(i1 + 1).skip(i0) {
            let bin_lo = lo + i as f64 * width;
            let overlap = h.min(bin_lo + width) - l.max(bin_lo);
            if overlap > 0.0 {
                *slot += v * overlap / (h - l);
            }
        }
    }
    let total: f64 = vols.iter().sum();
    (total.is_finite() && total > 0.0).then_some(Profile {
        lo,
        width,
        vols,
        total,
        first_ms: first.t_open_ms,
        last_ms: last.t_open_ms,
    })
}

/// The profile of UTC day `day` when it is covered (`finished` says whether the day is over, so its
/// end must be covered too; otherwise it is covered up to `reference_ms`), with its last close.
fn day_profile(
    bars: &[ChartCandle],
    day: i64,
    finished: bool,
    reference_ms: f64,
    bins: u16,
    tf: f64,
) -> Option<(Profile, f64)> {
    let (start, end) = (day_start_ms(day), day_start_ms(day + 1));
    let in_day = bars_in(bars, start, end);
    if !complete(&in_day, start, finished.then_some(end), reference_ms, tf) {
        return None;
    }
    // Only the POC's tie-break reads it, which needs where the day ended, not its exact close.
    let last_close = super::price_of(in_day.last()?)?;
    Some((build(&in_day, bins)?, last_close))
}

/// A horizontal line of `profile` at `price`, standing for bin `bin`.
fn line(profile: &Profile, price: f64, bin: usize, tag: LineTag) -> Option<Found> {
    let price = positive(price)?;
    let vol = profile.vols[bin];
    Some(Found {
        shape: FoundShape::Horizontal { price },
        touches: Vec::new(),
        strength: (vol / profile.total) as f32,
        first_ms: profile.first_ms,
        last_ms: profile.last_ms,
        broken: None,
        touch_volume: vol,
        tolerance_frac: profile.width / price,
        tag: Some(tag),
    })
}

/// Whether the span from `from` up to `reference_ms` is covered well enough to say what traded in
/// it ([`span_covered`]), and if so whether any of its bars traded at `price`. `None` when the span
/// has too long a hole.
fn traded_since(
    bars: &[ChartCandle],
    from: f64,
    reference_ms: f64,
    tf: f64,
    price: f64,
) -> Option<bool> {
    let after = bars_in(bars, from, f64::INFINITY);
    if !span_covered(after.iter().map(|b| b.t_open_ms), from, reference_ms, tf) {
        return None;
    }
    Some(
        after
            .iter()
            .any(|b| f64::from(b.low) <= price && price <= f64::from(b.high)),
    )
}

/// Every line `p` asks for: the profiled day's POC, VAH and VAL, then the naked POCs newest day
/// first.
fn profile_lines(
    bars: &[ChartCandle],
    p: &VolumeProfileParams,
    reference_ms: f64,
    tf: f64,
) -> Vec<Found> {
    let reference_day = day_of(reference_ms);
    let mut out = Vec::new();
    // The previous day's POC drawn as the period's POC: a naked-POC pass over the same day would
    // draw it a second time, so it only re-tags this one.
    let mut prev_day_poc: Option<usize> = None;
    let (day, finished) = match p.period {
        ProfilePeriod::Day => (reference_day, false),
        ProfilePeriod::PrevDay => (reference_day - 1, true),
    };
    if p.poc || p.vah || p.val {
        if let Some((profile, last_close)) =
            day_profile(bars, day, finished, reference_ms, p.bins, tf)
        {
            if let Some(poc) = profile.poc(last_close) {
                let (va_lo, va_hi) = profile.value_area(poc, p.value_area_pct);
                if p.poc {
                    if let Some(f) = line(&profile, profile.centre(poc), poc, LineTag::Poc) {
                        if finished {
                            prev_day_poc = Some(out.len());
                        }
                        out.push(f);
                    }
                }
                if p.vah {
                    out.extend(line(&profile, profile.edge(va_hi + 1), va_hi, LineTag::Vah));
                }
                if p.val {
                    out.extend(line(&profile, profile.edge(va_lo), va_lo, LineTag::Val));
                }
            }
        }
    }
    for back in 1..=i64::from(p.naked_poc_days) {
        let day = reference_day - back;
        let Some((profile, last_close)) = day_profile(bars, day, true, reference_ms, p.bins, tf)
        else {
            continue;
        };
        let Some(poc) = profile.poc(last_close) else {
            continue;
        };
        let price = profile.centre(poc);
        if traded_since(bars, day_start_ms(day + 1), reference_ms, tf, price) != Some(false) {
            continue;
        }
        if back == 1 {
            if let Some(ix) = prev_day_poc {
                out[ix].tag = Some(LineTag::NakedPoc);
                continue;
            }
        }
        out.extend(line(&profile, price, poc, LineTag::NakedPoc));
    }
    out
}

#[cfg(test)]
mod tests;
