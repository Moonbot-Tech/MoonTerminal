//! `Trend`: sloped lines through pivots, and the line search `Squeeze` reuses.
//!
//! Every pair of same-kind pivots (highs for a resistance, lows for a support) defines a candidate
//! line, built in TIME so a gap in the series does not bend it. A pivot of that kind touches the
//! line when it lies within the tolerance of the line's price at the pivot's time. Candidates with
//! enough touches and a slope inside the bounds are ranked by touch count, then by span, and taken
//! greedily: a candidate sharing two or more touches with an already accepted line is the same
//! line seen from another pair and is skipped. Only then are bodies checked — a bar between the
//! first and the last touch whose body crosses the line by more than the tolerance is a body break
//! (wicks never count), and a line with more than the allowed breaks is rejected.
//!
//! Slope is measured in % of price per bar, the base price being the line's price at its FIRST
//! touch: `(p1 − p0) / (t1 − t0) · tf_ms / p0 · 100` on the reported endpoints, so the bound can be
//! re-checked from the output alone. A pair is screened by the same formula on its own two pivots
//! before its touches are counted (that keeps flat pairs out of the candidate list); the check on
//! the final endpoints is the one that decides.
//!
//! Cost on `P` pivots of a kind and `n` bars: O(P³) to count touches over every pair, plus
//! O(C·(P + A)) to accept `C` candidates against `A` accepted lines, plus O(n) body checks per
//! line that survives the overlap test. The default 150-bar window holds ~20 pivots of a kind;
//! 2 000 bars with 3/3 pivots hold ~285, which is tens of millions of cheap float operations —
//! acceptable on a bar close, never per frame.

use moon_core::config::auto_figures::{DetectorKind, DetectorParams, TrendSide};
use moon_core::market::candles::ChartCandle;

use super::pivots::{Pivot, PivotKind, find_pivots};
use super::{
    DetectCtx, Detector, Found, FoundShape, Side, Touch, bar_ok, line_price_at, pct_frac,
    scan_break, touch_strength, window,
};

/// The `Trend` registry row.
pub struct Trend;

impl Detector for Trend {
    fn kind(&self) -> DetectorKind {
        DetectorKind::Trend
    }

    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found> {
        let DetectorParams::Trend(p) = params else {
            return Vec::new();
        };
        let win = window(bars, p.lookback_bars);
        let mut pivots = Vec::new();
        find_pivots(win, p.pivot_left, p.pivot_right, &mut pivots);
        let kinds: &[PivotKind] = match p.side {
            TrendSide::Resistance => &[PivotKind::High],
            TrendSide::Support => &[PivotKind::Low],
            TrendSide::Both => &[PivotKind::High, PivotKind::Low],
        };
        let mut out = Vec::new();
        for &kind in kinds {
            let spec = LineSpec {
                kind,
                tol_frac: pct_frac(p.tolerance_pct),
                min_touches: usize::from(p.min_touches),
                max_body_breaks: usize::from(p.max_body_breaks),
                min_slope_frac: slope_bound(p.min_slope_pct_per_bar),
                max_slope_frac: slope_bound(p.max_slope_pct_per_bar),
            };
            for line in find_lines(win, &pivots, &spec, ctx.tf_ms) {
                let origin = match kind {
                    PivotKind::High => Side::Below,
                    PivotKind::Low => Side::Above,
                };
                let (t0, p0, t1, p1) = (line.t0_ms, line.p0, line.t1_ms, line.p1);
                out.push(Found {
                    shape: FoundShape::Sloped {
                        t0_ms: t0,
                        p0,
                        t1_ms: t1,
                        p1,
                    },
                    first_ms: t0,
                    last_ms: t1,
                    strength: touch_strength(
                        line.touches.len(),
                        line.first_idx,
                        line.last_idx,
                        win.len(),
                    ),
                    broken: scan_break(win, line.last_idx, origin, |t| {
                        line_price_at(t0, p0, t1, p1, t)
                    }),
                    touches: line.touches,
                });
            }
        }
        out
    }
}

/// A slope bound in % per bar as a fraction per bar. NaN stays NaN on purpose: every comparison
/// against it is false, so a NaN bound admits no line rather than every line.
fn slope_bound(pct: f32) -> f64 {
    f64::from(pct) / 100.0
}

/// What a line search looks for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LineSpec {
    pub kind: PivotKind,
    /// Touch tolerance, fraction of the line's price.
    pub tol_frac: f64,
    /// Fewest touches; values below two read as two, the pair that defines the line.
    pub min_touches: usize,
    pub max_body_breaks: usize,
    /// Bounds on `|slope|`, fraction of price per bar (see the module doc for the base price).
    pub min_slope_frac: f64,
    pub max_slope_frac: f64,
}

/// A sloped line through its first and last touch.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub t0_ms: f64,
    pub p0: f64,
    pub t1_ms: f64,
    pub p1: f64,
    /// Touches in time order.
    pub touches: Vec<Touch>,
    /// Window indices of the first and the last touch.
    pub first_idx: usize,
    pub last_idx: usize,
}

impl Line {
    /// The line's price at `t_ms`.
    pub fn price_at(&self, t_ms: f64) -> Option<f64> {
        line_price_at(self.t0_ms, self.p0, self.t1_ms, self.p1, t_ms)
    }
}

/// Every line `spec` describes among `pivots` of `win`; see the module doc for the procedure.
///
/// Empty when `tf_ms <= 0`, when there are fewer pivots of the kind than touches required, or when
/// a bound is NaN.
pub(crate) fn find_lines(
    win: &[ChartCandle],
    pivots: &[Pivot],
    spec: &LineSpec,
    tf_ms: i64,
) -> Vec<Line> {
    let piv: Vec<&Pivot> = pivots
        .iter()
        .filter(|q| q.kind == spec.kind && q.price > 0.0)
        .collect();
    let min_touches = spec.min_touches.max(2);
    if tf_ms <= 0 || piv.len() < min_touches {
        return Vec::new();
    }
    let tf = tf_ms as f64;

    // Candidate pairs (a, b, touches). Touches are recounted on acceptance instead of stored, so
    // the candidate list stays three words per pair.
    let mut candidates: Vec<(usize, usize, usize)> = Vec::new();
    for a in 0..piv.len() {
        for b in a + 1..piv.len() {
            let (pa, pb) = (piv[a], piv[b]);
            if !slope_ok(pa.t_ms, pa.price, pb.t_ms, pb.price, tf, spec) {
                continue;
            }
            let count = piv
                .iter()
                .filter(|q| touches_line(q, pa, pb, spec.tol_frac))
                .count();
            if count >= min_touches {
                candidates.push((a, b, count));
            }
        }
    }
    candidates.sort_by(|x, y| {
        y.2.cmp(&x.2)
            .then((piv[y.1].idx - piv[y.0].idx).cmp(&(piv[x.1].idx - piv[x.0].idx)))
            .then(x.0.cmp(&y.0))
            .then(x.1.cmp(&y.1))
    });

    let mut accepted: Vec<Line> = Vec::new();
    let mut accepted_sets: Vec<Vec<usize>> = Vec::new();
    for (a, b, _) in candidates {
        let (pa, pb) = (piv[a], piv[b]);
        let set: Vec<usize> = (0..piv.len())
            .filter(|&k| touches_line(piv[k], pa, pb, spec.tol_frac))
            .collect();
        let overlaps = accepted_sets
            .iter()
            .any(|s| s.iter().filter(|k| set.contains(k)).count() >= 2);
        if overlaps {
            continue;
        }
        let (Some(&first), Some(&last)) = (set.first(), set.last()) else {
            continue;
        };
        let (first, last) = (piv[first], piv[last]);
        let (Some(p0), Some(p1)) = (
            line_price_at(pa.t_ms, pa.price, pb.t_ms, pb.price, first.t_ms),
            line_price_at(pa.t_ms, pa.price, pb.t_ms, pb.price, last.t_ms),
        ) else {
            continue;
        };
        if !slope_ok(first.t_ms, p0, last.t_ms, p1, tf, spec) {
            continue;
        }
        let line = Line {
            t0_ms: first.t_ms,
            p0,
            t1_ms: last.t_ms,
            p1,
            touches: set
                .iter()
                .map(|&k| Touch {
                    t_ms: piv[k].t_ms,
                    price: piv[k].price,
                })
                .collect(),
            first_idx: first.idx,
            last_idx: last.idx,
        };
        if body_breaks(win, &line, spec) > spec.max_body_breaks {
            continue;
        }
        accepted.push(line);
        accepted_sets.push(set);
    }
    accepted
}

/// `|slope|` of the line through `(t0, p0)` and `(t1, p1)`, in fraction of `p0` per bar of `tf`
/// ms, lies inside the spec's bounds. A NaN anywhere fails the check.
fn slope_ok(t0: f64, p0: f64, t1: f64, p1: f64, tf: f64, spec: &LineSpec) -> bool {
    let dt = t1 - t0;
    if !(dt > 0.0 && p0 > 0.0) {
        return false;
    }
    let abs = ((p1 - p0) / dt * tf / p0).abs();
    abs >= spec.min_slope_frac && abs <= spec.max_slope_frac
}

/// Pivot `q` lies within `tol_frac` of the line through `a` and `b`, at `q`'s time.
fn touches_line(q: &Pivot, a: &Pivot, b: &Pivot, tol_frac: f64) -> bool {
    line_price_at(a.t_ms, a.price, b.t_ms, b.price, q.t_ms)
        .is_some_and(|line| super::within(q.price, line, tol_frac))
}

/// Bars between the line's first and last touch whose BODY crosses it by more than the tolerance:
/// above a resistance through highs, below a support through lows.
fn body_breaks(win: &[ChartCandle], line: &Line, spec: &LineSpec) -> usize {
    let Some(span) = win.get(line.first_idx..=line.last_idx) else {
        return usize::MAX;
    };
    span.iter()
        .filter(|b| bar_ok(b))
        .filter(|b| {
            let Some(at) = line.price_at(b.t_open_ms) else {
                return false;
            };
            let top = f64::from(b.open.max(b.close));
            let bottom = f64::from(b.open.min(b.close));
            match spec.kind {
                PivotKind::High => top > at * (1.0 + spec.tol_frac),
                PivotKind::Low => bottom < at * (1.0 - spec.tol_frac),
            }
        })
        .count()
}

#[cfg(test)]
mod tests;
