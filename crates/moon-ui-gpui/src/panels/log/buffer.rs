//! The Log panel's row buffer: parsed rows, the filtered view over them, and how new lines enter.
//!
//! Split out of [`super::LogPanel`] because this is the part that runs on every backend revision
//! and the part whose cost the panel is built around — and, inside a GPUI view, the part nothing
//! could test. It owns no window state, so its behaviour is checked directly in `buffer/tests.rs`.
//!
//! The rule the whole module exists to keep: **nothing here may scale with the buffer on the
//! arrival path.** A line is parsed once, when it arrives ([`super::render::LineView::parse`]);
//! filtering only reads what parsing left behind; eviction rebases indices by arithmetic. The one
//! deliberate whole-buffer pass is [`RowBuffer::refilter`], and it runs on a user action — a
//! keystroke in the search box, a toggled checkbox — never on a revision.

use super::render::{self, LineView};
use super::row;
use crate::panels::line_list;
use moon_core::applog::LogLine;
use std::collections::HashSet;

/// The filter terms one pass matches rows against, lowercased once by the caller.
#[derive(Clone, Copy)]
pub(super) struct Filters<'a> {
    pub(super) errors_only: bool,
    pub(super) query: &'a str,
    pub(super) coin: Option<&'a str>,
}

/// What an ingest did to the positions the selection addresses rows by.
///
/// The POSITION matters, not just the fact: on a multi-core source rows arrive out of order on most
/// batches, so a plain "something moved" would clear the user's selection several times a second
/// and make the rows impossible to select at all.
#[derive(PartialEq, Debug)]
pub(super) enum Disturbance {
    /// Rows were only added after everything already on screen; existing positions still hold.
    AppendedOnly,
    /// Visible positions from `from` onwards no longer address the rows they did. Anything strictly
    /// before it is untouched.
    Moved { from: usize },
}

/// Parsed rows for one source and the filtered view over them.
#[derive(Default)]
pub(super) struct RowBuffer {
    /// Parsed rows, oldest first, always sorted by timestamp.
    rows: Vec<LineView>,
    /// Indices into `rows` that pass the current filters, ascending. The virtual list addresses
    /// rows by position in THIS list.
    view: Vec<usize>,
    /// Coin bases learned from the rows of this source, so a BARE ticker is highlighted once some
    /// line has stated it outright.
    ///
    /// Accumulated, not recollected: it grows with each batch and is dropped only when the source
    /// changes. Bounded in practice by how many coins one core trades, and two consequences are
    /// deliberate — a base learned now does not light up rows parsed before it (a whole-buffer
    /// rebuild used to do that by accident), and one learned from an evicted row keeps working.
    known: HashSet<String>,
    /// Character budget of the widest row that passed the filters, sizing the horizontal scroll
    /// area. Raised as rows arrive and recomputed only by [`RowBuffer::refilter`], so eviction can
    /// leave it generous — a scroll area slightly wider than its content, never narrower.
    widest_chars: usize,
}

impl RowBuffer {
    /// Drops everything, for a source the buffer cannot be carried across.
    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.view.clear();
        self.known.clear();
        self.widest_chars = 0;
    }

    /// Number of rows held, filtered or not — the "total" the panel reports.
    pub(super) fn total(&self) -> usize {
        self.rows.len()
    }

    /// Number of rows currently visible.
    pub(super) fn visible(&self) -> usize {
        self.view.len()
    }

    /// The row at visible position `ix`.
    pub(super) fn at(&self, ix: usize) -> Option<&LineView> {
        self.view.get(ix).and_then(|&row| self.rows.get(row))
    }

    /// Visible positions, for resolving a selected range into rows.
    pub(super) fn visible_rows(&self) -> &[usize] {
        &self.view
    }

    /// Every held row, for resolving a visible position.
    pub(super) fn rows(&self) -> &[LineView] {
        &self.rows
    }

    pub(super) fn widest_chars(&self) -> usize {
        self.widest_chars
    }

    /// Whether one parsed row passes `f`.
    fn passes(row: &LineView, f: Filters) -> bool {
        if f.errors_only && !line_list::is_error(row.sev) {
            return false;
        }
        if !f.query.is_empty()
            && !row.lower.contains(f.query)
            // The source column is short, and this only runs for rows the message did not already
            // match, so it stays off the common path.
            && !row.target.to_lowercase().contains(f.query)
        {
            return false;
        }
        if f.coin.is_some_and(|coin| !row.lower.contains(coin)) {
            return false;
        }
        true
    }

    /// Rebuilds the visible list over the whole buffer.
    ///
    /// The path for anything that changes which EXISTING rows qualify: a query edit, the errors-only
    /// toggle, a coin chip. It re-scans, but parses nothing — severity, the lowercase text and the
    /// coin range were all computed when each row arrived.
    pub(super) fn refilter(&mut self, f: Filters) {
        self.view.clear();
        self.widest_chars = 0;
        self.extend_view_from(0, f);
    }

    /// Parses `fresh`, merges it in, evicts past `cap`, and extends the visible list.
    ///
    /// Args:
    ///     fresh: Unseen lines, ordered oldest first.
    ///     cap: Rows to keep; the oldest above it are dropped.
    ///     f: Current filter terms.
    ///
    /// Returns:
    ///     Whether visible positions moved, so the caller can decide the selection's fate.
    pub(super) fn ingest(
        &mut self,
        mut fresh: Vec<LogLine>,
        cap: usize,
        f: Filters,
    ) -> Disturbance {
        if fresh.is_empty() {
            return self.evict(cap);
        }
        if fresh.len() > cap {
            // Parsing rows the cap would immediately evict is pure waste. Reachable after a hidden
            // tab is reopened onto a busy aggregate.
            fresh.drain(0..fresh.len() - cap);
        }
        // Learn the batch's coin names BEFORE parsing it, so a base one line states outright
        // highlights its bare mentions in the same batch.
        self.known.extend(render::collect_coin_bases(&fresh));
        let mut parsed: Vec<LineView> = fresh
            .iter()
            .map(|line| LineView::parse(line, &self.known))
            .collect();
        // The buffer's sortedness is this type's invariant, so it is established here rather than
        // trusted from the caller: a source whose lines are not perfectly ordered — two feed threads
        // racing into one ring, a core's clock stepping back mid-batch — would otherwise leave the
        // buffer permanently unsorted, and every later splice would compound it.
        parsed.sort_by(|a, b| a.ts.cmp(&b.ts));
        let at = self.splice(parsed);
        // Everything before the insertion point kept its index and its verdict; from there on the
        // visible list is rebuilt, so `keep` is the first position that can have changed.
        let keep = self.view.partition_point(|&ix| ix < at);
        let spliced = (keep < self.view.len()).then_some(keep);
        self.view.truncate(keep);
        self.extend_view_from(at, f);
        let evicted = match self.evict(cap) {
            Disturbance::Moved { from } => Some(from),
            Disturbance::AppendedOnly => None,
        };
        match spliced.into_iter().chain(evicted).min() {
            Some(from) => Disturbance::Moved { from },
            None => Disturbance::AppendedOnly,
        }
    }

    /// Inserts already-sorted `parsed` and returns the first buffer index it disturbed.
    ///
    /// Existing rows win ties, so equal timestamps keep the order they were displayed in. Only the
    /// tail from the insertion point is rebuilt — the prefix above it cannot be affected by rows
    /// that all sort at or after it. That matters because inserting earlier rows is NOT rare: cores
    /// stamp their own lines and their clocks disagree, so on a multi-core source most batches
    /// arrive partly "late".
    fn splice(&mut self, parsed: Vec<LineView>) -> usize {
        let Some(first) = parsed.first() else {
            return self.rows.len();
        };
        let at = self.rows.partition_point(|row| row.ts <= first.ts);
        if at == self.rows.len() {
            self.rows.extend(parsed);
            return at;
        }
        let tail = self.rows.split_off(at);
        let mut old = tail.into_iter().peekable();
        let mut new = parsed.into_iter().peekable();
        loop {
            let take_old = match (old.peek(), new.peek()) {
                (Some(o), Some(n)) => o.ts <= n.ts,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            let next = if take_old { old.next() } else { new.next() };
            self.rows.extend(next);
        }
        at
    }

    /// Appends the rows from `from` onwards that pass `f` to the visible list.
    fn extend_view_from(&mut self, from: usize, f: Filters) {
        let mut widest = self.widest_chars;
        let added: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .skip(from)
            .filter(|(_, row)| Self::passes(row, f))
            .map(|(ix, row)| {
                widest = widest.max(row::row_width_chars(row));
                ix
            })
            .collect();
        self.widest_chars = widest.min(line_list::WIDEST_CHARS_CAP);
        self.view.extend(added);
    }

    /// Drops the oldest rows above `cap` and rebases the visible list onto what is left.
    ///
    /// A busy source sits AT its cap, so this runs on every revision — hence arithmetic on the
    /// visible indices rather than re-running the filters over the whole buffer.
    fn evict(&mut self, cap: usize) -> Disturbance {
        if self.rows.len() <= cap {
            return Disturbance::AppendedOnly;
        }
        let dropped = self.rows.len() - cap;
        self.rows.drain(0..dropped);
        let before = self.view.len();
        self.view.retain(|&ix| ix >= dropped);
        for ix in &mut self.view {
            *ix -= dropped;
        }
        // Rebasing changes which ROW each entry names, but a visible POSITION only moves when an
        // entry ahead of it disappeared. Evicting rows that were all filtered out leaves the list on
        // screen exactly as it was, and must not cost the user their selection.
        if self.view.len() == before {
            Disturbance::AppendedOnly
        } else {
            Disturbance::Moved { from: 0 }
        }
    }
}

#[cfg(test)]
/// Tests for row buffering, splicing and eviction.
mod tests;
