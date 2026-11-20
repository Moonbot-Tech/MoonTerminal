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
//! # Among the lines rows KEEP, and in which order
//!
//! Twins are judged among what each row's own selection keeps, never among what it merely found
//! ([`select_rows`]): a formation is somebody's twin only where two rows would both DRAW it. Judged
//! over the found lines instead, a "Flags" row that finds the flag but does not keep it — the flag
//! is on the side the row does not take, past its `count`, or there is no reference price — would
//! still take it away from the "Channels" row that does keep it, and the formation would vanish
//! from the chart altogether. So:
//!
//! 1. every row runs its own `take` as if it were alone;
//! 2. among the formations so kept, one set of twins is drawn once, by the line with the most
//!    precise name (`Flag > Wedge = Triangle > Channel`; the upper row on a tie);
//! 3. a row whose kept twin went to another row takes its next line instead, ONCE — its `take`
//!    again with the lost line, and every formation another row now draws, out of the running
//!    (`take::select_skipping`), so the slot is not left empty and the refill cannot bring the
//!    same formation back;
//! 4. twins among the refills themselves (two rows refilling with one formation) are drawn once
//!    the same way, the loser dropped with no further refill.
//!
//! The survivor shows the union of the events every row of its set marks ([`union_marks`]) — a
//! "Channels" row marking breakouts and a "Flags" row that does not still see the breakout of the
//! flag both keep. A dropped twin is neither drawn nor counted nor a confluence partner, on the
//! chart and in the screener alike.
//!
//! Who calls it: both places that put several rows' lines side by side — the chart's selection
//! (the terminal's `chartdx/auto_figures_sync/select.rs`, on a publication and on every price
//! change, since a row's selection follows the price) and the screener's scan ([`super::scan`]).
//! On a price change it costs O(K²) in the K formations the rows keep — each row keeps a few — plus
//! one more `take` for a row that lost a twin; it never walks every row's found lines pairwise.

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

/// For every entry of `twins` — every line put side by side, in row order (the lines the rows KEEP,
/// [`select_rows`]), `None` for a line that is not a formation — the index of the line that draws
/// it instead, into
/// `winner_of` (one entry per line, overwritten): `Some(j)` for a twin that loses to line `j`,
/// `None` for a line that is drawn (a survivor, or anything but a formation).
///
/// A formation loses to its twin with the higher `rank`; on equal ranks (the same kind found by
/// two rows) the EARLIER line wins, so the row higher in the panel keeps it. Exactly one line of
/// each set of twins survives, and every other member of the set names that one — never a fellow
/// loser — so a caller can hand the survivor what each loser's row contributes ([`union_marks`]).
///
/// Cost: O(P²) in the formations among `twins` (the other lines are skipped in one pass): the rows
/// of a pane or a market keep a few each, and a level row adds only its `None`s. Nothing is
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

/// One row of a cross-row selection ([`select_rows`]): what it found, on which timeframe, and the
/// marks it shows.
#[derive(Debug, Clone, Copy)]
pub struct TwinRow<'a> {
    pub tf: TfId,
    pub found: &'a [Found],
    pub marks: &'a EventMarks,
}

/// What the cross-row selection decided for one row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowPick {
    /// Indices into the row's found lines that it draws, in the order its selection ranked them.
    pub kept: Vec<usize>,
    /// The kept lines that survived a set of twins together with the union of the marks of every
    /// row of the set ([`union_marks`]): `(index into found, marks)`. A kept line not listed shows
    /// its row's own marks.
    pub joined: Vec<(usize, EventMarks)>,
}

impl RowPick {
    /// The marks found line `ix` shows when it survived a set of twins; `None` for the row's own.
    pub fn joined_of(&self, ix: usize) -> Option<&EventMarks> {
        self.joined.iter().find(|(i, _)| *i == ix).map(|(_, m)| m)
    }
}

/// Every row's drawn lines, with one formation two rows KEEP drawn once (the module doc's four
/// steps).
///
/// `select(r, skip)` is row `r`'s own selection: its `take` (and whatever else the caller drops,
/// such as the lines the user hid) with the found lines where `skip[i]` is `true` out of the
/// running — `skip` is empty on the first call, and shorter than `found` means the rest are not
/// skipped (`take::select_skipping`'s contract). It is called once per row, and once more for a row
/// that lost a kept twin.
///
/// Returns one [`RowPick`] per row of `rows`, in order. Cost O(K²) in the formations kept, see the
/// module doc.
pub fn select_rows<F>(rows: &[TwinRow<'_>], mut select: F) -> Vec<RowPick>
where
    F: FnMut(usize, &[bool]) -> Vec<usize>,
{
    let mut picks: Vec<RowPick> = (0..rows.len())
        .map(|r| RowPick {
            kept: select(r, &[]),
            joined: Vec::new(),
        })
        .collect();
    let mut scratch = Vec::new();
    let first = kept_losers(rows, &picks, &mut scratch);
    if first.is_empty() {
        return picks;
    }
    // The formations drawn after the first round, by row: what a refill must not bring back.
    let lost_at = |r: usize, ix: usize| first.iter().any(|l| l.row == r && l.ix == ix);
    let drawn: Vec<(usize, Twin)> = picks
        .iter()
        .enumerate()
        .flat_map(|(r, p)| p.kept.iter().map(move |&ix| (r, ix)))
        .filter(|&(r, ix)| !lost_at(r, ix))
        .filter_map(|(r, ix)| {
            let row = &rows[r];
            row.found
                .get(ix)
                .and_then(|f| Twin::of(f, row.tf))
                .map(|t| (r, t))
        })
        .collect();
    for (r, row) in rows.iter().enumerate() {
        if !first.iter().any(|l| l.row == r) {
            continue;
        }
        let skip: Vec<bool> = row
            .found
            .iter()
            .enumerate()
            .map(|(ix, f)| {
                lost_at(r, ix)
                    || Twin::of(f, row.tf)
                        .is_some_and(|t| drawn.iter().any(|(dr, d)| *dr != r && d.same(&t)))
            })
            .collect();
        picks[r].kept = select(r, &skip);
    }
    let second = kept_losers(rows, &picks, &mut scratch);
    for l in &second {
        picks[l.row].kept.retain(|&ix| ix != l.ix);
    }
    for l in first.iter().chain(&second) {
        if !picks[l.winner_row].kept.contains(&l.winner_ix) {
            continue;
        }
        let winner = &mut picks[l.winner_row];
        let joined = union_marks(
            winner
                .joined_of(l.winner_ix)
                .unwrap_or(rows[l.winner_row].marks),
            rows[l.row].marks,
        );
        match winner.joined.iter_mut().find(|(i, _)| *i == l.winner_ix) {
            Some(entry) => entry.1 = joined,
            None => winner.joined.push((l.winner_ix, joined)),
        }
    }
    picks
}

/// A kept formation that loses to a twin another kept line draws, and that line.
#[derive(Debug, Clone, Copy)]
struct Loser {
    row: usize,
    ix: usize,
    winner_row: usize,
    winner_ix: usize,
}

/// The losers among the lines `picks` keep: [`twin_winners`] over the kept lines alone.
fn kept_losers(rows: &[TwinRow<'_>], picks: &[RowPick], scratch: &mut Vec<usize>) -> Vec<Loser> {
    let owners: Vec<(usize, usize)> = picks
        .iter()
        .enumerate()
        .flat_map(|(r, p)| p.kept.iter().map(move |&ix| (r, ix)))
        .collect();
    let twins: Vec<Option<Twin>> = owners
        .iter()
        .map(|&(r, ix)| rows[r].found.get(ix).and_then(|f| Twin::of(f, rows[r].tf)))
        .collect();
    let mut winner_of = Vec::new();
    twin_winners(&twins, scratch, &mut winner_of);
    winner_of
        .iter()
        .enumerate()
        .filter_map(|(flat, w)| {
            let (row, ix) = owners[flat];
            let (winner_row, winner_ix) = owners[(*w)?];
            Some(Loser {
                row,
                ix,
                winner_row,
                winner_ix,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
