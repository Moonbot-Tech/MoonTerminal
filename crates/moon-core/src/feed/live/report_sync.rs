//! Report catch-up progress for the status bar, read off the events the replica writer gets.

use moonproto::state::ReportEvent;

use crate::feed::ReportSyncProgress;

#[cfg(test)]
mod tests;

/// Follows one core's report catch-up from its start to its completion.
#[derive(Default)]
pub(super) struct ReportSyncTracker {
    progress: Option<ReportSyncProgress>,
}

impl ReportSyncTracker {
    /// Fold one report event in.
    ///
    /// Returns the value to publish when the event moved the progress: `Some(None)` once a
    /// catch-up completes, `None` for an event that says nothing about catch-up.
    pub(super) fn observe(
        &mut self,
        event: &ReportEvent,
        now_ms: i64,
    ) -> Option<Option<ReportSyncProgress>> {
        match event {
            // Announced only for a sync this terminal requested — a resume after a hard reconnect
            // is silent — so every announcement is a new catch-up with a span of its own.
            ReportEvent::SyncStarted { .. } => {
                self.progress = Some(ReportSyncProgress::started(now_ms));
                Some(self.progress)
            }
            ReportEvent::SyncPage(page) => {
                let first_row = page.rows.iter().map(|row| row.rec_id).min();
                self.on_page(
                    PageFacts {
                        first_row,
                        last_rec_id: page.last_rec_id,
                        max_rec_id: page.max_rec_id,
                        rows: page.rows.len(),
                        recreated: page.database_recreated,
                    },
                    now_ms,
                );
                Some(self.progress)
            }
            ReportEvent::SyncComplete(_) => {
                self.progress = None;
                Some(None)
            }
            _ => None,
        }
    }

    /// Fold one page in. Separate from [`Self::observe`] because a `ReportSyncPage` cannot be
    /// built outside moonproto, and this is the part worth testing.
    fn on_page(&mut self, page: PageFacts, now_ms: i64) {
        let started = self.progress.map_or(now_ms, |p| p.started_ms);
        // A recreated database restarts the download from zero.
        if page.recreated {
            self.progress = Some(ReportSyncProgress::started(started));
            return;
        }
        let progress = self
            .progress
            .get_or_insert_with(|| ReportSyncProgress::started(started));
        // An empty page reports `last_rec_id = 0` and delivered nothing.
        if page.last_rec_id > 0 {
            let first = page.first_row.unwrap_or(page.last_rec_id);
            progress.first_rec_id = Some(progress.first_rec_id.map_or(first, |f| f.min(first)));
            progress.last_rec_id = progress.last_rec_id.max(page.last_rec_id);
        }
        progress.max_rec_id = page.max_rec_id;
        progress.rows = progress.rows.saturating_add(page.rows as u64);
    }
}

/// What one catch-up page says about progress.
struct PageFacts {
    /// Lowest `newRecID` among the page's rows.
    first_row: Option<i64>,
    last_rec_id: i64,
    max_rec_id: i64,
    rows: usize,
    recreated: bool,
}
