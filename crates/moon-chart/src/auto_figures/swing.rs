//! `Swing`: horizontal levels where pivots cluster.
//!
//! Pivot highs and lows of the searched window are sorted by price and grouped greedily: a group
//! starts at its lowest price and takes every next pivot up to the tolerance above that start. The
//! tolerance is the row's `tolerance_pct`, or under `ToleranceUnit::Atr` its `tolerance_atr` times
//! the ATR over `ATR_PERIOD` bars divided by the last close (`super::tolerance_frac`) — computed
//! over ALL the bars handed in, not the lookback window: the caller fetches `bars_needed`, which is
//! at least `ATR_PERIOD + 1` under ATR, while the lookback itself may be shorter. No ATR (too few
//! bars) finds nothing.
//!
//! The greedy cut depends on where a group happens to start, so it can split one zone in two: two
//! neighbouring groups whose means lie within the tolerance of each other are then merged, as long
//! as the merged zone stays within [`MAX_ZONE_TOLERANCES`] tolerances of its lowest price (the
//! 21.11.2026 audit found 425 such split pairs among the default rows' levels).
//!
//! The level is the group's mean, and its own tolerance (`Found::tolerance_frac`) is the row's
//! tolerance or, for a merged zone whose touches reach farther from the mean, that reach — so
//! every touch lies within the level's own tolerance, and its break buffer and its band grow with
//! the zone. A bar counts once even when both its high and its low fall into one group.
//!
//! A level is a price the market did NOT pass through: when, between two of its touches, closes stayed
//! beyond it for longer than the row's `BrokenPolicy` keeps a broken level (any close beyond under
//! `Remove`), the touches before that pass are history of a level that broke, and are dropped —
//! the level is what the touches after it make, if they are still enough. The pass is read by the
//! same rule as a break after the last touch (`scan_break` with the break buffer, `survives`), from
//! each touch's own side.
//!
//! A level's ORIGINAL side is read from its latest touch: a pivot high there makes it a resistance
//! (price came from below), a pivot low a support. A close beyond the level — past its price by
//! more than the break buffer (`super::BREAK_BUFFER_SHARE` of the tolerance), on the other side —
//! after that latest touch breaks it; the LATEST such break episode is the one reported, together
//! with how long the longest one lasted (see `scan_break`), and the row's `BrokenPolicy` decides
//! whether the level is dropped or kept flagged: a stay beyond longer than `Keep { bars }` drops it
//! for good, even once price is back.
//!
//! Cost on `n` bars with `P` pivots and `L` levels: O(n·(left+right) + P log P + L·n).

use moon_core::config::auto_figures::{
    ATR_PERIOD, BrokenPolicy, DetectorKind, DetectorParams, SwingParams, ToleranceUnit,
};
use moon_core::market::candles::ChartCandle;

use std::ops::Range;

use super::pivots::{Pivot, PivotKind, find_pivots};
use super::{
    Broken, DetectCtx, Detector, Found, FoundShape, Side, Touch, atr, break_buffer, scan_break,
    survives, tolerance_frac, touch_strength, touch_volume, window,
};

/// The `Swing` registry row.
pub struct Swing;

impl Detector for Swing {
    fn kind(&self) -> DetectorKind {
        DetectorKind::Swing
    }

    fn detect(&self, bars: &[ChartCandle], params: &DetectorParams, ctx: &DetectCtx) -> Vec<Found> {
        let DetectorParams::Swing(p) = params else {
            return Vec::new();
        };
        let Some(tolerance_frac) = swing_tolerance(bars, p, ctx.price_tick) else {
            return Vec::new();
        };
        let win = window(bars, p.lookback_bars);
        let mut pivots = Vec::new();
        find_pivots(win, p.pivot_left, p.pivot_right, &mut pivots);
        levels_from(win, &pivots, p, tolerance_frac)
            .into_iter()
            .map(|l| Found {
                shape: FoundShape::Horizontal { price: l.price },
                first_ms: l.touches.first().map_or(0.0, |t| t.t_ms),
                last_ms: l.touches.last().map_or(0.0, |t| t.t_ms),
                strength: l.strength,
                touch_volume: touch_volume(win, &l.touches),
                touches: l.touches,
                broken: l.broken,
                // The level's own band: a merged zone's is wider than the row's tolerance.
                tolerance_frac: l.tolerance_frac,
                tag: None,
                origin: Some(l.origin),
            })
            .collect()
    }
}

/// The level tolerance of `p` as a fraction of price, in the row's unit, over all of `bars`, at
/// least two of the market's `price_tick` when it is known (`super::tolerance_frac`); `None` under
/// ATR without an ATR (see the module doc).
pub(crate) fn swing_tolerance(
    bars: &[ChartCandle],
    p: &SwingParams,
    price_tick: Option<f64>,
) -> Option<f64> {
    let atr = match p.tolerance_unit {
        ToleranceUnit::Pct => None,
        ToleranceUnit::Atr => atr(bars, ATR_PERIOD),
    };
    tolerance_frac(
        p.tolerance_unit,
        p.tolerance_pct,
        p.tolerance_atr,
        atr,
        bars,
        price_tick,
    )
}

/// A level with the bookkeeping `Squeeze` needs on top of what `Found` carries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Level {
    pub price: f64,
    /// Touches in time order, one per bar.
    pub touches: Vec<Touch>,
    /// Indices into the searched window of the first and the latest touch.
    pub first_idx: usize,
    pub last_idx: usize,
    /// The side price was on when it last touched the level.
    pub origin: Side,
    pub strength: f32,
    pub broken: Option<Broken>,
    /// The level's own tolerance, fraction of its price: the row's tolerance, or the half width
    /// of a merged zone when its touches lie farther from the level than that ([`zone_of`]). What
    /// its breaks are buffered by and its band is drawn with.
    pub tolerance_frac: f64,
}

/// The price and the own tolerance of a level made of `members`: their mean, and the larger of
/// `frac` and the farthest member's distance from that mean as a fraction of it — so every touch
/// lies within the tolerance of the level, a merged zone included (module doc). A single greedy
/// cut never reaches past `frac` of its mean, so it keeps `frac` exactly. `None` without members.
fn zone_of(members: &[&Pivot], frac: f64) -> Option<(f64, f64)> {
    if members.is_empty() {
        return None;
    }
    let price = members.iter().map(|q| q.price).sum::<f64>() / members.len() as f64;
    let reach = members
        .iter()
        .map(|q| (q.price - price).abs() / price)
        .fold(0.0, f64::max);
    Some((price, frac.max(reach)))
}

/// Widest a merged zone may be, in tolerances of its lowest price (module doc): two, so that a
/// merge joins the two halves of one zone a greedy cut split, never a chain of them.
pub const MAX_ZONE_TOLERANCES: f64 = 2.0;

/// Every level of `win` that survives `p.broken`, from pivots already found on `win` with
/// `p.pivot_left` / `p.pivot_right`, grouped by `frac` — the tolerance as a fraction of price,
/// already resolved in the row's unit ([`swing_tolerance`]), so `p`'s own tolerance fields are not
/// read here.
///
/// `p.min_touches = 0` reads as one: a group always holds at least one pivot.
pub(crate) fn levels_from(
    win: &[ChartCandle],
    pivots: &[Pivot],
    p: &SwingParams,
    frac: f64,
) -> Vec<Level> {
    let min_touches = usize::from(p.min_touches.max(1));
    let mut sorted: Vec<&Pivot> = pivots.iter().filter(|q| q.price > 0.0).collect();
    sorted.sort_by(|a, b| a.price.total_cmp(&b.price).then(a.idx.cmp(&b.idx)));
    price_groups(&sorted, frac)
        .into_iter()
        .filter_map(|g| build_level(win, &sorted[g], min_touches, p.broken, frac))
        .collect()
}

/// `sorted` (pivots ascending by price) cut into price groups, as index ranges in ascending order:
/// greedy cuts of `frac` above each group's lowest price, then neighbouring cuts merged whose means
/// lie within `frac` of each other while the merged zone spans at most [`MAX_ZONE_TOLERANCES`]
/// tolerances of its lowest price (module doc). Cost O(P).
fn price_groups(sorted: &[&Pivot], frac: f64) -> Vec<Range<usize>> {
    let mut cuts: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    while start < sorted.len() {
        let top = sorted[start].price * (1.0 + frac);
        let mut end = start + 1;
        while end < sorted.len() && sorted[end].price <= top {
            end += 1;
        }
        cuts.push(start..end);
        start = end;
    }
    let mean =
        |r: &Range<usize>| sorted[r.clone()].iter().map(|q| q.price).sum::<f64>() / r.len() as f64;
    let mut out: Vec<Range<usize>> = Vec::with_capacity(cuts.len());
    for cut in cuts {
        if let Some(prev) = out.last_mut() {
            let (near, lowest) = (mean(prev), sorted[prev.start].price);
            let highest = sorted[cut.end - 1].price;
            if mean(&cut) - near <= near * frac
                && highest <= lowest * (1.0 + MAX_ZONE_TOLERANCES * frac)
            {
                prev.end = cut.end;
                continue;
            }
        }
        out.push(cut);
    }
    out
}

/// The side a touch shows the level held price on: below a pivot high, above a pivot low.
fn held_side(kind: PivotKind) -> Side {
    match kind {
        PivotKind::High => Side::Below,
        PivotKind::Low => Side::Above,
    }
}

/// One price group turned into a level, or `None` when it has too few distinct bars — counted
/// after the touches before a pass through the level are dropped (module doc) — or the policy
/// drops it as broken. `frac` is the touch tolerance the group was built with; the break buffer
/// is its [`break_buffer`].
fn build_level(
    win: &[ChartCandle],
    group: &[&Pivot],
    min_touches: usize,
    policy: BrokenPolicy,
    frac: f64,
) -> Option<Level> {
    // One touch per bar: an outside bar whose high and low both fall in a narrow group is one
    // touch, not two. Time order, highs before lows on the same bar (the pivot order).
    let mut members: Vec<&Pivot> = group.to_vec();
    members.sort_by(|a, b| {
        a.idx
            .cmp(&b.idx)
            .then((a.kind == PivotKind::Low).cmp(&(b.kind == PivotKind::Low)))
    });
    members.dedup_by_key(|q| q.idx);
    // Each stretch between two touches — from the earlier touch's bar up to, not including, the
    // later touch's — read from the earlier touch's side against the level's price; the last
    // stretch the level did not survive cuts its history there. Cutting moves the price, so the
    // stretches left are read again against the new one, until nothing is cut: the history
    // that stays is checked against the price the level is drawn at. Each round drops at least
    // one touch, so it ends.
    while let Some((price, tol)) = zone_of(&members, frac) {
        let buffer = break_buffer(tol);
        // Searched from the newest stretch back: the first one found is the last pass.
        let restart = members
            .windows(2)
            .enumerate()
            .rev()
            .find(|(_, pair)| {
                let Some(stretch) = win.get(..pair[1].idx) else {
                    return false;
                };
                let origin = held_side(pair[0].kind);
                let broken = scan_break(stretch, pair[0].idx, origin, buffer, |_| Some(price));
                !survives(broken, policy)
            })
            .map_or(0, |(i, _)| i + 1);
        if restart == 0 {
            break;
        }
        members.drain(..restart);
    }
    if members.len() < min_touches {
        return None;
    }
    let (price, tolerance_frac) = zone_of(&members, frac)?;
    let first = members.first()?;
    let last = members.last()?;
    let origin = held_side(last.kind);
    let buffer = break_buffer(tolerance_frac);
    let broken = scan_break(win, last.idx, origin, buffer, |_| Some(price));
    if !survives(broken, policy) {
        return None;
    }
    Some(Level {
        price,
        touches: members
            .iter()
            .map(|q| Touch {
                t_ms: q.t_ms,
                price: q.price,
            })
            .collect(),
        first_idx: first.idx,
        last_idx: last.idx,
        origin,
        strength: touch_strength(members.len(), first.idx, last.idx, win.len()),
        broken,
        tolerance_frac,
    })
}

#[cfg(test)]
mod tests;
