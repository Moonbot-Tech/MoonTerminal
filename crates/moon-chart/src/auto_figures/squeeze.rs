//! `Squeeze`: a horizontal level plus a sloped line from its OTHER side converging into it —
//! rising lows pressing a resistance, falling highs pressing a support. One formation, one figure.
//!
//! The level half is exactly a `Swing` level of `params.level`, under the level's own
//! `BrokenPolicy`: `Remove` drops a squeeze whose level broke, `Keep { bars }` keeps it with the
//! level's break in `Found::broken`, so the breakout of a squeeze is visible for those bars and the
//! event layer can mark it as a breakout or a false break. The sloped half is a line through pivot LOWS for a resistance (pivot HIGHS for a support) found by the
//! `Trend` line search with `trend_tolerance_pct` and `min_trend_touches`, which must:
//! - rise toward a resistance / fall toward a support (converge, not diverge);
//! - have every touch strictly on the level's original side;
//! - overlap the level in time (its last touch after the level's first touch).
//!
//! `pressed` is set when the sloped line, projected to the last valid bar, is within `pressed_pct`
//! of the level — measured as a fraction of the LEVEL price — or already past it.
//!
//! Each level and each sloped line takes part in at most one formation: valid pairs are taken
//! greedily, the one whose projected gap to the level is smallest first.
//!
//! The contract gives the sloped half no body-break or slope bounds of its own; it is searched with
//! [`MAX_BODY_BREAKS`] and with no slope bounds beyond the direction above.
//!
//! Cost: one pivot pass, the `Swing` levels and two `Trend` line searches (each capped at
//! `trend::MAX_LINE_PIVOTS` pivots, see that module), plus O(L·S) to pair `L` levels with `S`
//! lines.

use moon_core::config::auto_figures::{DetectorKind, DetectorParams, SqueezeParams};
use moon_core::market::candles::ChartCandle;

use super::pivots::{PivotKind, find_pivots};
use super::swing::{Level, levels_from};
use super::trend::{Line, LineSpec, find_lines};
use super::{
    DetectCtx, Detector, Found, FoundShape, Side, Touch, bar_ok, pct_frac, touch_strength, window,
};

/// Bodies allowed to cross the sloped half between its first and last touch. One, not zero: a
/// single wide bar closing through a rising-lows line is common in a live squeeze and does not
/// undo the formation, while two already make the line a guess.
pub const MAX_BODY_BREAKS: usize = 1;

/// The `Squeeze` registry row.
pub struct Squeeze;

impl Detector for Squeeze {
    fn kind(&self) -> DetectorKind {
        DetectorKind::Squeeze
    }

    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found> {
        let DetectorParams::Squeeze(p) = params else {
            return Vec::new();
        };
        squeezes(bars, p, ctx.tf_ms)
    }
}

/// A valid (level, line) pairing and its gap, fraction of the level price (negative = crossed).
struct Pair {
    level: usize,
    line: usize,
    gap: f64,
}

fn squeezes(bars: &[ChartCandle], p: &SqueezeParams, tf_ms: i64) -> Vec<Found> {
    let win = window(bars, p.level.lookback_bars);
    let Some(last_t) = win.iter().rev().find(|b| bar_ok(b)).map(|b| b.t_open_ms) else {
        return Vec::new();
    };
    let mut pivots = Vec::new();
    find_pivots(win, p.level.pivot_left, p.level.pivot_right, &mut pivots);
    // `levels_from` has already applied the level's policy: what is left is either unbroken or
    // kept flagged.
    let levels: Vec<Level> = levels_from(win, &pivots, &p.level);
    if levels.is_empty() {
        return Vec::new();
    }
    let spec = |kind| LineSpec {
        kind,
        tol_frac: pct_frac(p.trend_tolerance_pct),
        min_touches: usize::from(p.min_trend_touches),
        max_body_breaks: MAX_BODY_BREAKS,
        min_slope_frac: 0.0,
        max_slope_frac: f64::INFINITY,
    };
    // Lows rise under a resistance (origin below), highs fall over a support (origin above).
    let lows = find_lines(win, &pivots, &spec(PivotKind::Low), tf_ms);
    let highs = find_lines(win, &pivots, &spec(PivotKind::High), tf_ms);
    let pressed_frac = pct_frac(p.pressed_pct);

    let lines_for = |origin: Side| match origin {
        Side::Below => &lows,
        Side::Above => &highs,
    };
    let mut pairs: Vec<(Side, Pair)> = Vec::new();
    for (li, level) in levels.iter().enumerate() {
        let lines = lines_for(level.origin);
        for (si, line) in lines.iter().enumerate() {
            if let Some(gap) = pair_gap(level, line, last_t) {
                pairs.push((
                    level.origin,
                    Pair {
                        level: li,
                        line: si,
                        gap,
                    },
                ));
            }
        }
    }
    pairs.sort_by(|a, b| {
        a.1.gap
            .total_cmp(&b.1.gap)
            .then(a.1.level.cmp(&b.1.level))
            .then(a.1.line.cmp(&b.1.line))
    });

    let mut level_used = vec![false; levels.len()];
    let mut low_used = vec![false; lows.len()];
    let mut high_used = vec![false; highs.len()];
    let mut out = Vec::new();
    for (origin, pair) in pairs {
        let line_used = match origin {
            Side::Below => &mut low_used,
            Side::Above => &mut high_used,
        };
        if level_used[pair.level] || line_used[pair.line] {
            continue;
        }
        level_used[pair.level] = true;
        line_used[pair.line] = true;
        let level = &levels[pair.level];
        let line = &lines_for(origin)[pair.line];
        let touches = merge_touches(&level.touches, &line.touches);
        let first_idx = level.first_idx.min(line.first_idx);
        let last_idx = level.last_idx.max(line.last_idx);
        out.push(Found {
            shape: FoundShape::Squeeze {
                level: level.price,
                t0_ms: line.t0_ms,
                p0: line.p0,
                t1_ms: line.t1_ms,
                p1: line.p1,
                pressed: pair.gap <= pressed_frac,
            },
            first_ms: touches.first().map_or(0.0, |t| t.t_ms),
            last_ms: touches.last().map_or(0.0, |t| t.t_ms),
            strength: touch_strength(touches.len(), first_idx, last_idx, win.len()),
            touches,
            broken: level.broken,
        });
    }
    out
}

/// The touches of both halves in time order, each pivot once.
///
/// A pivot can belong to both: a level groups pivot highs AND lows within its band, so a low just
/// under a resistance can be both a level touch and a touch of the rising lows line. Counted twice,
/// it would inflate the formation's touch count and its strength.
fn merge_touches(level: &[Touch], line: &[Touch]) -> Vec<Touch> {
    let mut touches = Vec::with_capacity(level.len() + line.len());
    touches.extend_from_slice(level);
    touches.extend_from_slice(line);
    touches.sort_by(|a, b| a.t_ms.total_cmp(&b.t_ms).then(a.price.total_cmp(&b.price)));
    touches.dedup_by(|a, b| a.t_ms == b.t_ms && a.price == b.price);
    touches
}

/// The gap between `line` projected to `last_t` and `level`, as a fraction of the level price —
/// positive while the line is still on the level's original side — or `None` when the two do not
/// form a squeeze (see the module doc).
fn pair_gap(level: &Level, line: &Line, last_t: f64) -> Option<f64> {
    if level.price <= 0.0 || line.t1_ms <= level.touches.first()?.t_ms {
        return None;
    }
    let converging = match level.origin {
        Side::Below => line.p1 > line.p0,
        Side::Above => line.p1 < line.p0,
    };
    let on_side = line.touches.iter().all(|t| match level.origin {
        Side::Below => t.price < level.price,
        Side::Above => t.price > level.price,
    });
    if !converging || !on_side {
        return None;
    }
    let projected = line.price_at(last_t)?;
    let gap = match level.origin {
        Side::Below => (level.price - projected) / level.price,
        Side::Above => (projected - level.price) / level.price,
    };
    gap.is_finite().then_some(gap)
}

#[cfg(test)]
mod tests;
