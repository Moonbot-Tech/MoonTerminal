//! Read cursors that turn the Log panel's live sources into an append-only stream.
//!
//! The panel used to answer every backend revision by re-reading its whole source — up to
//! `VIEW_LIMIT` rows merged across every core — and re-parsing all of it, so a single new line on
//! one core cost a full rebuild. Measured on this machine at ten cores that was ~25 ms per rebuild,
//! four times a second, whether or not a single row survived the errors-only filter.
//!
//! What makes the difference is a per-source cursor: `CoreData::log_seq` and `applog::revision`
//! count lines ever pushed, so "what have I not seen" is a subtraction. [`LiveCursors`] keeps one
//! cursor per core plus one for the local ring, and [`LiveCursors::pull`] returns exactly the
//! unseen lines. [`super::LogPanel`] parses those and appends them.
//!
//! The first load still goes through [`super::render::aggregate`] — it applies the same per-core and
//! total caps the view is defined by — after which [`LiveCursors::seek_to_end`] marks everything as
//! seen.

use super::{LogSource, LogSourceItem};
use crate::Backend;
use moon_core::applog::{self, LogLine};
use moon_core::session::{CoreId, CoreStore};
use std::collections::{HashMap, HashSet};

/// Read position in each live source feeding the panel's current selection.
///
/// A cursor is meaningless across sources, so switching source clears them ([`LiveCursors::clear`])
/// and the panel takes a fresh snapshot.
#[derive(Default)]
pub(super) struct LiveCursors {
    /// Last seen `CoreData::log_seq` per core.
    cores: HashMap<CoreId, u64>,
    /// Last seen `applog::revision`.
    local: u64,
}

impl LiveCursors {
    /// Forget every read position, so the next pull would return each source's whole ring.
    pub(super) fn clear(&mut self) {
        self.cores.clear();
        self.local = 0;
    }

    /// Record the local ring's cursor as taken together with a snapshot of it.
    ///
    /// The pair has to come from one lock acquisition ([`applog::snapshot_with_cursor`]) because
    /// feed threads push into that ring; deriving the cursor here instead would drop any line that
    /// landed between the two reads.
    pub(super) fn set_local(&mut self, cursor: u64) {
        self.local = cursor;
    }

    /// Mark everything currently in the selected source as already seen.
    ///
    /// Called right after a full snapshot so the next [`LiveCursors::pull`] returns only what
    /// arrived afterwards. The core store is only ever mutated on the UI thread, so reading its
    /// counters in the same synchronous pass as the snapshot is exact — nothing can land between
    /// the two. The local ring has no such guarantee and is handled by [`LiveCursors::set_local`],
    /// which is why this leaves it alone.
    pub(super) fn seek_to_end(
        &mut self,
        b: &Backend,
        source: &LogSource,
        sources: &[LogSourceItem],
        allowed: Option<&HashSet<CoreId>>,
    ) {
        match source {
            LogSource::Local => {}
            LogSource::Core(id) => {
                let seq = b.session.store().core(*id).map_or(0, |core| core.log_seq);
                self.cores.insert(*id, seq);
            }
            LogSource::Aggregate | LogSource::Exchange(_) => {
                self.seek_cores(b.session.store(), sources, allowed)
            }
        }
    }

    /// [`LiveCursors::seek_to_end`] for the multi-core sources, without the backend around it.
    fn seek_cores(
        &mut self,
        store: &CoreStore,
        sources: &[LogSourceItem],
        allowed: Option<&HashSet<CoreId>>,
    ) {
        self.cores.clear();
        for id in member_ids(sources, allowed) {
            let seq = store.core(id).map_or(0, |core| core.log_seq);
            self.cores.insert(id, seq);
        }
    }

    /// Return every line the selected source produced since the last pull, oldest first.
    ///
    /// Rows come back in canonical source order; putting them in timestamp order is
    /// [`super::buffer::RowBuffer::ingest`]'s job, and it sorts stably, so rows sharing a timestamp
    /// keep the same order [`super::render::aggregate`] gives them.
    ///
    /// Args:
    ///     b: Backend holding the live rings and revision counters.
    ///     source: Currently selected live source.
    ///     sources: Canonically ordered source entries in the panel scope.
    ///     allowed: Selected exchange membership, or `None` for every core source.
    ///
    /// Returns:
    ///     The unseen lines, each already labelled with its source name.
    pub(super) fn pull(
        &mut self,
        b: &Backend,
        source: &LogSource,
        sources: &[LogSourceItem],
        allowed: Option<&HashSet<CoreId>>,
    ) -> Vec<LogLine> {
        match source {
            LogSource::Local => {
                let (lines, next) = applog::since(self.local);
                self.local = next;
                lines
            }
            LogSource::Core(id) => {
                let store = b.session.store();
                let Some(core) = store.core(*id) else {
                    return Vec::new();
                };
                let cursor = self.cores.get(id).copied().unwrap_or(0);
                let (lines, next) = core.log_since(cursor);
                let lines: Vec<LogLine> = lines.cloned().collect();
                self.cores.insert(*id, next);
                lines
            }
            LogSource::Aggregate | LogSource::Exchange(_) => {
                self.pull_cores(b.session.store(), sources, allowed)
            }
        }
    }

    /// [`LiveCursors::pull`] for the multi-core sources, without the backend around it.
    fn pull_cores(
        &mut self,
        store: &CoreStore,
        sources: &[LogSourceItem],
        allowed: Option<&HashSet<CoreId>>,
    ) -> Vec<LogLine> {
        let members = member_ids(sources, allowed);
        let mut fresh: Vec<LogLine> = Vec::new();
        for item in sources {
            let LogSource::Core(id) = item.source else {
                continue;
            };
            if !members.contains(&id) {
                continue;
            }
            let Some(core) = store.core(id) else {
                continue;
            };
            let cursor = self.cores.get(&id).copied().unwrap_or(0);
            let (lines, next) = core.log_since(cursor);
            // A core read for the first time hands over its whole ring. Cap that at the same
            // per-core budget the snapshot path uses, or one newly configured core would contribute
            // more history in one batch than the aggregate ever shows from it.
            let lines: Vec<&LogLine> = lines.collect();
            let from = lines.len().saturating_sub(super::AGG_PER_CORE);
            fresh.extend(lines[from..].iter().map(|line| {
                let mut line = (*line).clone();
                // The aggregate labels each row with the core that wrote it; a core's own ring
                // leaves `target` empty because the selector already names it.
                line.target = item.display.clone();
                line
            }));
            self.cores.insert(id, next);
        }
        // Cursors of cores outside the current membership are KEPT: the map is bounded by the
        // configured core count either way, and a core that leaves and returns would otherwise come
        // back at cursor zero and replay its whole ring.
        //
        // Rows come out in canonical source order, NOT timestamp order. Ordering the buffer is
        // `RowBuffer::ingest`'s invariant, and it sorts stably, so rows sharing a timestamp keep the
        // source order `render::aggregate` gives them.
        fresh
    }
}

/// Core ids this source draws from, honouring an exchange membership when one is selected.
fn member_ids(sources: &[LogSourceItem], allowed: Option<&HashSet<CoreId>>) -> HashSet<CoreId> {
    sources
        .iter()
        .filter_map(|item| match item.source {
            LogSource::Core(id) if allowed.is_none_or(|allowed| allowed.contains(&id)) => Some(id),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
/// Tests for incremental live-source reading.
mod tests;
