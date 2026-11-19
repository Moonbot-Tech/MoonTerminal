//! One formation found by two rows is drawn once (LinKvo, 19.11.2026).
//!
//! The panel's formation rows run independently: a "Flags" row and a "Channels" row are two runs
//! of the same detector with different switches, and the same pair of sides can pass both — once
//! as the flag it is, once as the channel its geometry is. Drawn twice, one stretch of the chart
//! carries two captions ("Flags · Flag" and "Channels · Channel") on one pair of lines. The
//! answer is one drawing, under the more precise name: a flag or a pennant says more than the
//! wedge or triangle it is shaped as, and either says more than "channel" —
//! `Flag > Wedge = Triangle > Channel`. A double top or bottom is a geometry of its own (two
//! extremes and a neckline), so it is never the twin of a two-line kind, only of the same double
//! found by another row.
//!
//! # What makes two formations the same one
//!
//! The identity the chart's "Hide this line" already follows a formation by across runs
//! (`chartdx/auto_figures_sync/select.rs::LineKey`): the first touch of EACH side, plus the
//! family — any two-line kind, a double top, a double bottom. A formation keeps those while it
//! grows (one more swing moves only its last touch), and two rows that read the same pair of
//! sides start them at the same pivots. No tolerance: rows on one timeframe read the same bars, so
//! the same pivot has the same open time to the bit — and rows on DIFFERENT timeframes are never
//! twins, because a 1h and a 4h reading of a pattern are two readings, which is exactly what a
//! confluence between them is about.
//!
//! # Who calls it
//!
//! Both places that put several rows' lines side by side, after each row's own selection
//! (`take::select`) and before the cross-row confluence pass: the chart's pane (the terminal's
//! `chartdx/auto_figures_sync/select.rs`) and the screener's scan ([`super::scan`]). A dropped
//! twin is then neither drawn nor counted nor a confluence partner, on the chart and in the
//! screener alike.

use moon_core::config::auto_figures::TfId;

use super::{Found, FoundShape, LineTag};

/// What a formation is, as far as being someone's twin goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// A triangle, wedge, channel or flag: two sides through their own touches.
    TwoLine,
    DoubleTop,
    DoubleBottom,
}

/// One kept formation as the twin pass reads it: its identity across rows and the rank of its
/// name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Twin {
    /// The timeframe of the row that found it; twins share it.
    tf: TfId,
    family: Family,
    /// Open time of the upper side's first touch, as bits (compared for identity only).
    upper_t0: u64,
    /// Open time of the lower side's first touch, as bits.
    lower_t0: u64,
    /// How precise its name is ([`rank`]); the higher one is drawn.
    rank: u8,
}

impl Twin {
    /// The twin identity of `found`, found by a row on `tf`; `None` for anything but a formation
    /// (`FoundShape::Pair` with a formation tag), which nothing duplicates across rows.
    pub fn of(found: &Found, tf: TfId) -> Option<Self> {
        let FoundShape::Pair { upper, lower } = found.shape else {
            return None;
        };
        let tag = found.tag?;
        let family = match tag {
            LineTag::Channel | LineTag::Wedge | LineTag::Triangle | LineTag::Flag => {
                Family::TwoLine
            }
            LineTag::DoubleTop => Family::DoubleTop,
            LineTag::DoubleBottom => Family::DoubleBottom,
            _ => return None,
        };
        Some(Self {
            tf,
            family,
            upper_t0: upper.t0_ms.to_bits(),
            lower_t0: lower.t0_ms.to_bits(),
            rank: rank(tag),
        })
    }

    /// Whether `self` and `other` are the same formation, whatever each is called.
    pub fn same(&self, other: &Self) -> bool {
        self.tf == other.tf
            && self.family == other.family
            && self.upper_t0 == other.upper_t0
            && self.lower_t0 == other.lower_t0
    }
}

/// How precise a formation's name is: a flag (or pennant) names the move it pauses, a wedge or a
/// triangle the way its sides converge, a channel only that they run parallel. A double ranks
/// alone — it is only ever compared with the same double.
fn rank(tag: LineTag) -> u8 {
    match tag {
        LineTag::Flag => 3,
        LineTag::Wedge | LineTag::Triangle => 2,
        LineTag::Channel => 1,
        _ => 0,
    }
}

/// Which of `twins` — every kept line of the pane (or the market), in row order, `None` for a
/// line that is not a formation — are drawn by another line instead, into `losers` (one flag per
/// line, overwritten).
///
/// A formation loses to its twin with the higher `rank`; on equal ranks (the same kind found by
/// two rows) the EARLIER line wins, so the row higher in the panel keeps it. Exactly one line of
/// each set of twins survives.
///
/// Cost: O(n²) in the kept lines, which a pane and a market count in tens; nothing is allocated
/// once `losers` has grown.
pub fn twin_losers(twins: &[Option<Twin>], losers: &mut Vec<bool>) {
    losers.clear();
    losers.resize(twins.len(), false);
    for (i, a) in twins.iter().enumerate() {
        let Some(a) = a else {
            continue;
        };
        losers[i] = twins.iter().enumerate().any(|(j, b)| {
            b.is_some_and(|b| {
                j != i && a.same(&b) && (b.rank > a.rank || (b.rank == a.rank && j < i))
            })
        });
    }
}

#[cfg(test)]
mod tests;
