//! Report catch-up for one core: declaring it to the core once the replica writer knows where it
//! starts, and its progress for the status bar, read off the events that writer gets.

use moonproto::state::ReportEvent;
use moonproto::{MoonClient, ReportHistoryDepth, ReportSyncRequest};

use crate::config::ServerConfig;
use crate::db::{DbMsg, ReportStart, ReportTx};
use crate::feed::ReportSyncProgress;

#[cfg(test)]
mod tests;

/// Whether the replica writer has yet to open the replica — declaring this core's catch-up the
/// moment it has, when the core replicates its reports.
///
/// The feed asks on every pass while the answer is `true` and never again afterwards. A core
/// with its report feed off declares nothing but is told the same thing: the writer also stores
/// what such a core measures (its clock offset), and the feed holds that back until then.
///
/// Args:
///     client: This core's connection.
///     sink: The replica writer's channel and start states.
///     server: The core, for its feed flags and replica uid.
///
/// Returns:
///     `true` while the writer is not through its initialization.
pub(super) fn writer_pending(client: &MoonClient, sink: &ReportTx, server: &ServerConfig) -> bool {
    if server.feed.reports {
        !declare(client, sink, server)
    } else {
        !sink.is_started()
    }
}

/// Declare this core's report catch-up, if the replica writer can say where it starts.
///
/// The declaration is an intent: the library sends it once connected and repeats it by itself
/// after a hard reconnect. Where it starts comes from the writer's durable start state — a
/// checkpoint pairs the numeric cursor with the core's database epoch, so a replaced core database
/// is detected even when the new one already grew past the old ids. Catch-up is PAGED: the next
/// page is not requested until the writer commits and acknowledges the current one.
///
/// The writer has no start state until it has opened the replica, and it opens it only after the
/// startup integrity check — minutes on a file of several gigabytes. So this never waits: with no
/// answer yet it declares nothing and returns `false`, and the feed calls it again on a later
/// pass (it left a wake with [`ReportTx::wake_when_started`]). Until then the core sends no report
/// rows at all — asking for the schema is what turns live delivery on — so nothing queues up
/// behind a writer that is not running, and the catch-up that follows carries what was missed.
///
/// Args:
///     client: This core's connection.
///     sink: The replica writer's channel and start states.
///     server: The core, for its replica uid and the log line.
///
/// Returns:
///     `true` once the catch-up has been declared (or its declaration failed and was logged).
fn declare(client: &MoonClient, sink: &ReportTx, server: &ServerConfig) -> bool {
    let Some(start) = sink.try_start(server.uid) else {
        return false;
    };
    let started = match start {
        ReportStart::Fresh => client
            .reports()
            .sync(ReportSyncRequest::fresh(ReportHistoryDepth::All))
            .map(|_| "fresh(All)".to_string()),
        ReportStart::Resume(from) => client
            .reports()
            .sync(ReportSyncRequest::resume(from))
            .map(|_| format!("resume(from_rec_id={from}) — миграция без epoch")),
        ReportStart::Checkpoint(checkpoint) => client.reports().sync_from(checkpoint).map(|_| {
            format!(
                "sync_from(epoch={}, from_rec_id={})",
                checkpoint.epoch, checkpoint.next_from_rec_id
            )
        }),
    };
    match started {
        Ok(how) => log::info!(
            "отчёты: core={} «{}» sync запрошен: {how}",
            server.uid,
            server.name
        ),
        Err(e) => log::warn!(
            "отчёты: core={} «{}» sync не запустился: {e:?}",
            server.uid,
            server.name
        ),
    }
    // Open rows may have closed or been deleted offline BELOW the catch-up start. The writer
    // reads them and registers them for checking; the results arrive as ordinary
    // RowUpsert/RowDelete events. The feed repeats this on every reconnect.
    sink.send(DbMsg::RecheckOpenRows {
        core_uid: server.uid,
        after: None,
        reports: client.reports(),
    });
    true
}

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
