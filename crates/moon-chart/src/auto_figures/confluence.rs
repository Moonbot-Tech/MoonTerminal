//! Confluence: a line with a line of ANOTHER row on a DIFFERENT timeframe close by — a 1h level next
//! to a 4h one, a place both scales agree on (`AutoEvent::Confluence`).
//!
//! No detector can see this: each runs over one row's bars. So it is judged here, across rows, by
//! the caller that holds every row's kept lines. Pure and allocation-light; the chart calls it
//! whenever it recomputes events, the scanner can call it the same way.
//!
//! # Use
//!
//! ```text
//! // One entry per kept line of every visible row, in any order.
//! let placed: Vec<PlacedLine> = rows.iter().flat_map(|row| {
//!     row.kept.iter().filter_map(move |line| Some(PlacedLine {
//!         row: row.ix,
//!         tf: row.tf,
//!         price: line.found.shape.price_at(at_ms)?,
//!         pct: row.events.confluence_pct,
//!     }))
//! }).collect();
//! let mut near = Vec::new();
//! confluence(&placed, &mut near);
//! // near[i] answers for placed[i]; insert `AutoEvent::Confluence` into that line's `EventSet`
//! // when `row.events.marks(AutoEvent::Confluence)`.
//! ```
//!
//! # Rule
//!
//! Line `i` is flagged when some line `j` has `j.row != i.row`, `j.tf != i.tf` and
//! `|j.price - i.price| <= i.price · i.pct / 100` — the distance in `i`'s OWN row's percentage of
//! `i`'s price, so each row's setting decides its own mark (two rows with different settings can
//! disagree, which is what two settings mean). Both conditions are checked: a row has one
//! timeframe per run, but nothing here relies on it. A line whose price is not a positive finite
//! number is never flagged and never flags another.
//!
//! # Cost
//!
//! O(n log n): the lines are grouped by timeframe (at most seven groups) and sorted by price once;
//! each line then binary-searches its band in every OTHER timeframe's group, and decides "a line of
//! another row in that band" in O(1) from the run of equal rows starting at the band's first line.
//! A caller that runs it on every price change keeps a [`ConfluenceScratch`] and calls
//! [`confluence_in`]: the groups keep their buffers between runs, so a run allocates nothing once
//! they have grown to the largest input seen.

use moon_core::config::auto_figures::TfId;

use super::{pct_frac, positive};

/// One kept line, placed for the confluence pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedLine {
    /// Which row the line belongs to — any id equal for the lines of one row (the row's index).
    pub row: usize,
    /// The timeframe the row's detector ran on.
    pub tf: TfId,
    /// The line's price at the moment judged: `FoundShape::price_at(at_ms)`.
    pub price: f64,
    /// The row's `EventMarks::confluence_pct`, % of price.
    pub pct: f32,
}

/// One timeframe's lines, sorted by price, with what an O(1) "another row?" check needs.
#[derive(Debug, Clone, Default)]
struct Group {
    /// (price, row), ascending by price.
    lines: Vec<(f64, usize)>,
    /// For each position, the last position of the run of equal rows that starts there.
    run_end: Vec<usize>,
}

impl Group {
    /// Sort the lines pushed since the last [`Group::clear`] and index their runs of equal rows.
    fn finish(&mut self) {
        self.lines
            .sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        self.run_end.clear();
        self.run_end.resize(self.lines.len(), 0);
        for k in (0..self.lines.len()).rev() {
            self.run_end[k] = if k + 1 < self.lines.len() && self.lines[k + 1].1 == self.lines[k].1
            {
                self.run_end[k + 1]
            } else {
                k
            };
        }
    }

    /// Empty the group, keeping its buffers.
    fn clear(&mut self) {
        self.lines.clear();
        self.run_end.clear();
    }

    /// Whether a line of a row other than `row` lies in `[lo, hi]`.
    fn other_row_within(&self, lo: f64, hi: f64, row: usize) -> bool {
        let a = self.lines.partition_point(|l| l.0 < lo);
        let b = self.lines.partition_point(|l| l.0 <= hi);
        a < b && (self.lines[a].1 != row || self.run_end[a] + 1 < b)
    }
}

/// The working buffers of [`confluence_in`], kept by a caller that runs the pass often (the chart
/// runs it on every price change) so a run allocates nothing once the buffers have grown to the
/// largest input seen. Holds no state between runs: every run starts by clearing it.
#[derive(Debug, Clone, Default)]
pub struct ConfluenceScratch {
    /// One group per timeframe met so far; a timeframe absent from a run leaves its group empty.
    groups: Vec<(TfId, Group)>,
}

/// For each of `lines`, whether a line of another row on a different timeframe lies within that
/// line's row's `pct` of its price; written into `out`, one flag per line in input order (see the
/// module doc for the rule and the cost).
///
/// Allocates its working buffers on every call; a caller that runs it often keeps a
/// [`ConfluenceScratch`] and calls [`confluence_in`].
pub fn confluence(lines: &[PlacedLine], out: &mut Vec<bool>) {
    confluence_in(lines, &mut ConfluenceScratch::default(), out);
}

/// [`confluence`] with caller-owned working buffers: the same answer, without the allocations
/// once `scratch` has grown.
pub fn confluence_in(lines: &[PlacedLine], scratch: &mut ConfluenceScratch, out: &mut Vec<bool>) {
    out.clear();
    for (_, g) in &mut scratch.groups {
        g.clear();
    }
    for l in lines {
        let Some(price) = positive(l.price) else {
            continue;
        };
        match scratch.groups.iter_mut().find(|(tf, _)| *tf == l.tf) {
            Some((_, g)) => g.lines.push((price, l.row)),
            None => scratch.groups.push((
                l.tf,
                Group {
                    lines: vec![(price, l.row)],
                    run_end: Vec::new(),
                },
            )),
        }
    }
    for (_, g) in &mut scratch.groups {
        g.finish();
    }
    let groups = &scratch.groups;
    out.extend(lines.iter().map(|l| {
        let Some(price) = positive(l.price) else {
            return false;
        };
        let reach = price * pct_frac(l.pct);
        let (lo, hi) = (price - reach, price + reach);
        groups
            .iter()
            .filter(|(tf, _)| *tf != l.tf)
            .any(|(_, g)| g.other_row_within(lo, hi, l.row))
    }));
}

#[cfg(test)]
mod tests;
