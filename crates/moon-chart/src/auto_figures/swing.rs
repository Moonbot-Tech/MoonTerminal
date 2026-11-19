//! `Swing`: horizontal levels where pivots cluster.
//!
//! Pivot highs and lows of the searched window are sorted by price and grouped greedily: a group
//! starts at its lowest price and takes every next pivot up to `tolerance_pct` above that start.
//! The level is the group's mean, so every touch lies within the tolerance of the level itself
//! (the spread is at most `tolerance` of the lowest price, which is at most `tolerance` of the
//! mean). A bar counts once even when both its high and its low fall into one group.
//!
//! A level's ORIGINAL side is read from its latest touch: a pivot high there makes it a resistance
//! (price came from below), a pivot low a support. A close beyond the level — strictly past its
//! price, on the other side — after that latest touch breaks it; the LATEST such break episode is
//! the one reported (see `scan_break`), and the row's `BrokenPolicy` decides whether the level is
//! dropped or kept flagged.
//!
//! Cost on `n` bars with `P` pivots and `L` levels: O(n·(left+right) + P log P + L·n).

use moon_core::config::auto_figures::{BrokenPolicy, DetectorKind, DetectorParams, SwingParams};
use moon_core::market::candles::ChartCandle;

use super::pivots::{Pivot, PivotKind, find_pivots};
use super::{
    Broken, DetectCtx, Detector, Found, FoundShape, Side, Touch, pct_frac, scan_break, survives,
    touch_strength, touch_volume, window,
};

/// The `Swing` registry row.
pub struct Swing;

impl Detector for Swing {
    fn kind(&self) -> DetectorKind {
        DetectorKind::Swing
    }

    fn detect(
        &self,
        bars: &[ChartCandle],
        params: &DetectorParams,
        _ctx: &DetectCtx,
    ) -> Vec<Found> {
        let DetectorParams::Swing(p) = params else {
            return Vec::new();
        };
        let win = window(bars, p.lookback_bars);
        let mut pivots = Vec::new();
        find_pivots(win, p.pivot_left, p.pivot_right, &mut pivots);
        let tolerance_frac = pct_frac(p.tolerance_pct);
        levels_from(win, &pivots, p)
            .into_iter()
            .map(|l| Found {
                shape: FoundShape::Horizontal { price: l.price },
                first_ms: l.touches.first().map_or(0.0, |t| t.t_ms),
                last_ms: l.touches.last().map_or(0.0, |t| t.t_ms),
                strength: l.strength,
                touch_volume: touch_volume(win, &l.touches),
                touches: l.touches,
                broken: l.broken,
                tolerance_frac,
            })
            .collect()
    }
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
}

/// Every level of `win` that survives `p.broken`, from pivots already found on `win` with
/// `p.pivot_left` / `p.pivot_right`.
///
/// `p.min_touches = 0` reads as one: a group always holds at least one pivot.
pub(crate) fn levels_from(win: &[ChartCandle], pivots: &[Pivot], p: &SwingParams) -> Vec<Level> {
    let frac = pct_frac(p.tolerance_pct);
    let min_touches = usize::from(p.min_touches.max(1));
    let mut sorted: Vec<&Pivot> = pivots.iter().filter(|q| q.price > 0.0).collect();
    sorted.sort_by(|a, b| a.price.total_cmp(&b.price).then(a.idx.cmp(&b.idx)));

    let mut out = Vec::new();
    let mut start = 0;
    while start < sorted.len() {
        let anchor = sorted[start].price;
        let top = anchor * (1.0 + frac);
        let mut end = start + 1;
        while end < sorted.len() && sorted[end].price <= top {
            end += 1;
        }
        if let Some(level) = build_level(win, &sorted[start..end], min_touches, p.broken) {
            out.push(level);
        }
        start = end;
    }
    out
}

/// One price group turned into a level, or `None` when it has too few distinct bars or the policy
/// drops it as broken.
fn build_level(
    win: &[ChartCandle],
    group: &[&Pivot],
    min_touches: usize,
    policy: BrokenPolicy,
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
    if members.len() < min_touches {
        return None;
    }
    let price = members.iter().map(|q| q.price).sum::<f64>() / members.len() as f64;
    let first = members.first()?;
    let last = members.last()?;
    let origin = match last.kind {
        PivotKind::High => Side::Below,
        PivotKind::Low => Side::Above,
    };
    let broken = scan_break(win, last.idx, origin, |_| Some(price));
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
    })
}

#[cfg(test)]
mod tests;
