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
//! # Who calls it, and when
//!
//! Both places that put several rows' lines side by side, BEFORE each row's own selection
//! (`take::select_skipping`) and before the cross-row confluence pass: the chart's publication of
//! a finished run (the terminal's `chartdx/auto_figures_sync/select.rs`, once per result, never on
//! a price tick) and the screener's scan ([`super::scan`]). The pass runs over every row's FOUND
//! formations, not over what each row kept: a twin dropped after the selection would have spent
//! one of its row's `count` slots and left the row's next line undrawn, and a row whose selection
//! moved on a tick would bring its twin back under its own name — the caption switching between
//! "Flag" and "Channel" as price moves. Dropped before, the losing row never sees the twin, and
//! its `take` fills the slot with its next line.
//!
//! A dropped twin is neither drawn nor counted nor a confluence partner, on the chart and in the
//! screener alike. What its row MARKS is not lost with it: the winner shows the union of the
//! events every row of its set marks ([`union_marks`]) — a "Channels" row marking breakouts and a
//! "Flags" row that does not still see the breakout of the flag both found.

use moon_core::config::auto_figures::{EventMarks, TfId};

use super::{Found, FoundShape, LineTag};

/// What a formation is, as far as being someone's twin goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// A triangle, wedge, channel or flag: two sides through their own touches.
    TwoLine,
    DoubleTop,
    DoubleBottom,
}

/// One found formation as the twin pass reads it: its identity across rows and the rank of its
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

/// For every entry of `twins` — every FOUND line of the pane's (or the market's) rows, in row order,
/// `None` for a line that is not a formation — the index of the line that draws it instead, into
/// `winner_of` (one entry per line, overwritten): `Some(j)` for a twin that loses to line `j`,
/// `None` for a line that is drawn (a survivor, or anything but a formation).
///
/// A formation loses to its twin with the higher `rank`; on equal ranks (the same kind found by
/// two rows) the EARLIER line wins, so the row higher in the panel keeps it. Exactly one line of
/// each set of twins survives, and every other member of the set names that one — never a fellow
/// loser — so a caller can hand the survivor what each loser's row contributes ([`union_marks`]).
///
/// Cost: O(P²) in the formations among `twins` (the other lines are skipped in one pass): a pane
/// and a market find formations in tens, and a level row adds only its `None`s. Nothing is
/// allocated once `winner_of` and `scratch` have grown.
pub fn twin_winners(
    twins: &[Option<Twin>],
    scratch: &mut Vec<usize>,
    winner_of: &mut Vec<Option<usize>>,
) {
    winner_of.clear();
    winner_of.resize(twins.len(), None);
    scratch.clear();
    scratch.extend(
        twins
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.is_some().then_some(i)),
    );
    for &i in scratch.iter() {
        let Some(a) = twins[i] else {
            continue;
        };
        // The set's survivor: the highest rank, the earliest line on a tie.
        let mut best = i;
        let mut best_rank = a.rank;
        for &j in scratch.iter() {
            let Some(b) = twins[j] else {
                continue;
            };
            if j != i && a.same(&b) && (b.rank > best_rank || (b.rank == best_rank && j < best)) {
                best = j;
                best_rank = b.rank;
            }
        }
        if best != i {
            winner_of[i] = Some(best);
        }
    }
}

/// The events the survivor of a set of twins shows: those its own row marks, plus every one
/// `twin`'s row marks that it does not (LinKvo, 19.11.2026: one drawing, the union of the marks).
///
/// The distances belong to the event they size: an event only `twin`'s row marks comes with that
/// row's `approach_pct`, `stop_hunt_pct` or `confluence_pct`; one both rows mark keeps the
/// winner's own. The false-break window is the winner row's `BrokenPolicy`, which is not part of
/// the marks.
pub fn union_marks(winner: &EventMarks, twin: &EventMarks) -> EventMarks {
    let mut out = *winner;
    if twin.approach && !winner.approach {
        out.approach = true;
        out.approach_pct = twin.approach_pct;
    }
    if twin.stop_hunt && !winner.stop_hunt {
        out.stop_hunt = true;
        out.stop_hunt_pct = twin.stop_hunt_pct;
    }
    if twin.confluence && !winner.confluence {
        out.confluence = true;
        out.confluence_pct = twin.confluence_pct;
    }
    out.squeeze |= twin.squeeze;
    out.breakout |= twin.breakout;
    out.false_break |= twin.false_break;
    out.bounce |= twin.bounce;
    out.role_flip |= twin.role_flip;
    out
}

#[cfg(test)]
mod tests;
