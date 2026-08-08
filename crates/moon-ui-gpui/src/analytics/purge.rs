//! "Delete the strategy and its trades from the report": the confirmation dialog, its ordered
//! core-confirmed strategy sequence, and the optional empty-folder request that follows it.
//!
//! # Why this is a sequence and not a command batch
//!
//! The destructive commands do NOT keep their order on the wire. `TRepSetRowsDeleted` and
//! `TStratCheckedSync` are `Sliced` priority while `TStratDelete` is `High`; the protocol builds
//! sliced datagrams first but flushes the High bytes to the socket BEFORE transmitting the first
//! sliced block, so the delete overtakes the other two deterministically. On top of that, every
//! session command returns only "queued on the local channel" — a downstream protocol failure is
//! logged in the feed thread and never reaches the caller.
//!
//! So each step is SENT and then WAITED FOR against evidence the core committed it: the report
//! rows leave the replica (the core echoes `RowsDeleted`), and the core acknowledges the checkbox
//! delta or drops the strategy from its list. Each wait is bounded; a step that never confirms
//! stops the sequence rather than letting the next command race ahead of it.
//! Empty-folder cleanup has no separate acknowledgement. It is queued only after the strategy is
//! confirmed absent and a fresh snapshot shows no remaining strategy in that folder or its
//! descendants. The feed thread rechecks both MoonProto's live placements and any full-list sync
//! already accepted by its runtime queue; MoonProto itself has no atomic delete-if-empty
//! precondition.
//!
//! The ordering rule and its evidence policy are really a session-layer concern — the Strategies
//! window would want the same operation, and today it can only refuse to delete an enabled
//! strategy and leave the disable-and-wait to the user. Moving the driver into `moon-core` (a
//! plain `advance(&store) -> Progress` state machine, with the UI keeping only the timer and the
//! rendering) is the deeper shape; it is not done here.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AsyncApp, BackgroundExecutor, Context, Entity, FontWeight, IntoElement,
    ParentElement, Styled, WeakEntity, Window, div, px,
};
use moon_core::db;
use moon_core::feed::{ConnStatus, StrategyRow};
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonWindowExt as _,
    h_flex, v_flex,
};
use rusqlite::Connection;
use rust_i18n::t;

use super::AnalyticsView;
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::strategies::tree::ops::{has_row_under, join_path, split_path};

/// How often a wait re-checks its evidence.
const POLL: Duration = Duration::from_millis(300);
/// How long one step may go unconfirmed before the sequence gives up on it.
///
/// Generous on purpose: a sliced payload of thousands of rec ids needs several round trips, and
/// giving up early would report a failure for an operation that then completes anyway.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// How many row reads one purge step may make before it gives up.
///
/// The first two reads may produce send-and-confirm batches; the last is a verification read only.
/// More than one batch is allowed because trades keep closing into a strategy that is still live,
/// while the final bound prevents an endlessly busy strategy from looping forever.
const PURGE_PASSES: usize = 3;

/// One step of the purge, in the order the user is promised.
///
/// Declaration order IS the run order, and `Ord` derives from it — the progress list compares
/// steps directly instead of looking their positions up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PurgeStep {
    /// Hide the closed trades currently attributed to the strategy.
    Rows,
    /// Disable the strategy without changing the core-wide trading state.
    Disable,
    /// Trades that closed while the strategy was still enabled, caught after it went off.
    Sweep,
    /// Delete the strategy only after its closed trades have been swept.
    Delete,
}

impl PurgeStep {
    /// Locale key naming this step in progress lines and failure messages.
    fn label_key(self) -> &'static str {
        match self {
            PurgeStep::Rows => "analytics.purge.step_rows",
            PurgeStep::Disable => "analytics.purge.step_disable",
            PurgeStep::Sweep => "analytics.purge.step_sweep",
            PurgeStep::Delete => "analytics.purge.step_delete",
        }
    }

    /// Every step, for rendering the progress list.
    const ORDER: [PurgeStep; 4] = [
        PurgeStep::Rows,
        PurgeStep::Disable,
        PurgeStep::Sweep,
        PurgeStep::Delete,
    ];
}

/// How a step ended badly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PurgeFail {
    /// The command could not be queued on the local session channel.
    Send,
    /// The command was queued but the core never confirmed it.
    Confirm,
    /// The core dropped off before the step could be sent.
    CoreLost,
}

/// Why a sequence stopped early.
enum PurgeStop {
    /// The dialog was closed or replaced; there is nobody left to report to.
    Abandoned,
    /// A specific step could not be sent or confirmed.
    Failed(PurgeStep, PurgeFail),
    /// The strategy is already confirmed deleted, but its folder request was not locally queued.
    FolderSend,
}

/// What the dialog is showing right now.
#[derive(Debug, PartialEq)]
pub(super) enum PurgeState {
    /// Counting the strategy's trades for the confirmation text.
    Counting,
    /// The whole-history count is known and the operation can be confirmed.
    Ready { total: usize },
    /// The count could not be read, so the size of the deletion is unknown.
    CountFailed(String),
    /// The named step has started but has not yet been confirmed.
    Running(PurgeStep),
    /// Every step completed and was confirmed where confirmation was required.
    Done,
    /// The confirmed sequence completed, but the optional folder request did not enter the queue.
    FolderSendFailed,
    /// The named step stopped for the stated reason.
    Failed { step: PurgeStep, fail: PurgeFail },
}

/// One open "delete the strategy and its trades" operation.
pub(super) struct PurgeOp {
    /// Identity of THIS operation. A completion belonging to a dialog the user already closed and
    /// reopened must not publish into the new one, and a bare `is_some()` check cannot tell them
    /// apart.
    seq: u64,
    core_uid: u64,
    sid: u64,
    name: String,
    /// Core label exactly as the clicked row rendered it, so the confirmation names the same core
    /// the user was looking at rather than re-resolving it from a store that may have moved.
    core_name: String,
    /// Trades the clicked row counts for the selected Analytics period.
    period_trades: i64,
    /// Matching rows the protocol cannot address, so they survive the purge.
    ///
    /// Kept on the operation rather than inside `PurgeState::Ready`: the caveat must still be on
    /// screen when the sequence reports success, or "done" would read as "every trade is gone"
    /// while these are still in the report.
    legacy_rows: i64,
    state: PurgeState,
}

/// Assemble the confirmation's summary sentences, in fixed order.
///
/// Pure and separate from the layout so the legacy caveat cannot be dropped by an edit to the
/// rendering: the sentence exists or it does not, and that is testable without a window.
///
/// Args:
///     total: Soft-deletable trades found across the strategy's whole history.
///     period: Trades the clicked row counts for the selected period.
///     legacy: Matching rows in a legacy source, which cannot be addressed by the protocol.
///
/// Returns:
///     The count (or the "nothing to delete" line), then the legacy caveat when there is one.
pub(super) fn purge_summary_lines(total: usize, period: i64, legacy: i64) -> Vec<String> {
    let mut lines = Vec::new();
    if total == 0 {
        lines.push(t!("analytics.purge.no_rows").to_string());
    } else {
        lines.push(t!("analytics.purge.counts", total = total, period = period).to_string());
    }
    if legacy > 0 {
        lines.push(t!("analytics.purge.legacy_note", n = legacy).to_string());
    }
    lines
}

/// Choose the containing folder that became empty after one live strategy disappeared.
///
/// The candidate must come from the live row captured immediately before deletion. Both the
/// occupancy check and the command target use the shared path parser, so non-canonical separators
/// cannot make the UI inspect one folder and ask the core to delete another.
///
/// Args:
///     deleted_folder: Raw live folder path captured immediately before the delete command.
///     remaining: Fresh live strategy snapshot after the strategy disappeared.
///
/// Returns:
///     The canonical non-root folder path only when it has no direct or descendant strategies.
fn deletable_folder_after(
    deleted_folder: Option<&str>,
    remaining: &[StrategyRow],
) -> Option<String> {
    let path = split_path(deleted_folder?);
    if path.is_empty() || has_row_under(remaining, &path) {
        return None;
    }
    Some(join_path(&path))
}

/// Read the strategy's addressable rows, reusing `reader` when it is still usable.
///
/// The connection is handed in and back rather than reopened per call: a wait polls this every few
/// hundred milliseconds, and `open_reader` is a file probe plus a fresh connection plus two
/// ATTACHes. A failed read returns no connection, so the next attempt reconnects rather than
/// reusing one that may be broken.
///
/// Both statements run inside one pinned snapshot, so the rec ids and the legacy count cannot
/// describe two different committed states — the dialog would otherwise promise a number that was
/// never true at any instant.
fn read_purge_rows(
    reader: Option<Connection>,
    key: db::ReportStrategyKey,
) -> (Option<Connection>, db::ReadResult<db::StrategyPurgeRows>) {
    let conn = match reader {
        Some(conn) => conn,
        None => match db::open_reader() {
            Ok(conn) => conn,
            Err(fail) => return (None, Err(fail)),
        },
    };
    let result =
        db::read_snapshot(&conn).and_then(|snapshot| db::strategy_purge_rows(&snapshot, key));
    match result {
        Ok(rows) => (Some(conn), Ok(rows)),
        Err(fail) => (None, Err(fail)),
    }
}

/// Handles and the reusable report reader shared by one running purge.
struct PurgeRun {
    view: WeakEntity<AnalyticsView>,
    executor: BackgroundExecutor,
    seq: u64,
    core_uid: u64,
    sid: u64,
    key: db::ReportStrategyKey,
    reader: Option<Connection>,
}

impl PurgeRun {
    /// Run `edit` against the view, or `None` once the window is gone.
    fn with_view<T>(
        &self,
        cx: &mut AsyncApp,
        edit: impl FnOnce(&mut AnalyticsView, &mut Context<AnalyticsView>) -> T,
    ) -> Option<T> {
        cx.update(|cx| self.view.update(cx, edit).ok())
    }

    /// Is this operation still the one the dialog is showing?
    ///
    /// Re-checked immediately before every send, not merely when a step opens: each step awaits a
    /// background scan first, and Cancel, Escape or a replacement operation can land during that
    /// await. Without this, a cancelled purge would still fire its destructive command, with no
    /// dialog left to report it.
    fn still_mine(&self, cx: &mut AsyncApp) -> bool {
        self.with_view(cx, |view, _| {
            view.strat_purge
                .as_ref()
                .is_some_and(|op| op.seq == self.seq)
        })
        .unwrap_or(false)
    }

    /// Is the core connected right now? A retained snapshot is not evidence — `CoreStore` keeps a
    /// disconnected core's last strategy list indefinitely.
    fn core_ready(&self, cx: &mut AsyncApp) -> bool {
        self.with_view(cx, |view, cx| {
            view.backend
                .read(cx)
                .session
                .store()
                .core(self.core_uid)
                .is_some_and(|core| core.status == ConnStatus::Ready)
        })
        .unwrap_or(false)
    }

    /// The report generation, which advances on every committed replica write.
    fn report_generation(&self, cx: &mut AsyncApp) -> u64 {
        self.with_view(cx, |view, _| view.current_report_generation())
            .unwrap_or(0)
    }

    /// Publish the step the sequence is entering, after confirming the core can still receive it.
    fn open_step(&self, cx: &mut AsyncApp, step: PurgeStep) -> Result<(), PurgeStop> {
        if !self.core_ready(cx) {
            return Err(PurgeStop::Failed(step, PurgeFail::CoreLost));
        }
        let published = self
            .with_view(cx, |view, cx| {
                view.publish_purge(self.seq, cx, |op| op.state = PurgeState::Running(step));
                view.strat_purge
                    .as_ref()
                    .is_some_and(|op| op.seq == self.seq)
            })
            .unwrap_or(false);
        if published {
            Ok(())
        } else {
            Err(PurgeStop::Abandoned)
        }
    }

    /// The strategy's still-addressable rec ids.
    async fn rec_ids(&mut self, step: PurgeStep) -> Result<Vec<i64>, PurgeStop> {
        let (reader, key) = (self.reader.take(), self.key);
        let executor = self.executor.clone();
        let (reader, result) = executor
            .spawn(async move { read_purge_rows(reader, key) })
            .await;
        self.reader = reader;
        result
            .map(|rows| rows.rec_ids)
            .map_err(|_| PurgeStop::Failed(step, PurgeFail::Confirm))
    }

    /// Queue one command on the local session channel.
    ///
    /// Success proves only local queue acceptance. Any later serialization, socket, or protocol
    /// failure is logged on the feed thread and cannot reach this caller, so the caller must wait
    /// for separate core-originated evidence before advancing.
    fn send(
        &self,
        cx: &mut AsyncApp,
        step: PurgeStep,
        command: impl FnOnce(&moon_core::session::SessionManager) -> anyhow::Result<()>,
    ) -> Result<(), PurgeStop> {
        if !self.still_mine(cx) {
            return Err(PurgeStop::Abandoned);
        }
        let queued = self
            .with_view(cx, |view, cx| {
                command(&view.backend.read(cx).session).is_ok()
            })
            .unwrap_or(false);
        if queued {
            Ok(())
        } else {
            Err(PurgeStop::Failed(step, PurgeFail::Send))
        }
    }

    /// Queue the conditional empty-folder intent after the confirmed strategy sequence.
    ///
    /// Unlike [`Self::send`], a local failure has its own outcome because the Delete step has
    /// already succeeded and reporting that whole step as unsent would misstate the partial result.
    fn send_empty_folder(
        &self,
        cx: &mut AsyncApp,
        folder: String,
        expected_placements: Vec<(u64, String)>,
    ) -> Result<(), PurgeStop> {
        if !self.still_mine(cx) {
            return Err(PurgeStop::Abandoned);
        }
        let queued = self
            .with_view(cx, |view, cx| {
                view.backend
                    .read(cx)
                    .session
                    .delete_empty_folder(self.core_uid, folder, expected_placements)
                    .is_ok()
            })
            .unwrap_or(false);
        if queued {
            Ok(())
        } else {
            Err(PurgeStop::FolderSend)
        }
    }

    /// Inspect the current live strategy snapshot after proving the core can receive commands.
    ///
    /// Args:
    ///     cx: Async application context used to read the owning Analytics view.
    ///     step: Step that should own any connection failure.
    ///     inspect: Projection that copies only the strategy evidence needed after this UI read.
    ///
    /// Returns:
    ///     The caller's projection over every live strategy row for the ready core.
    fn inspect_strategies<T>(
        &self,
        cx: &mut AsyncApp,
        step: PurgeStep,
        inspect: impl FnOnce(&[StrategyRow]) -> T,
    ) -> Result<T, PurgeStop> {
        let result = self.with_view(cx, |view, cx| {
            let backend = view.backend.read(cx);
            let core = backend.session.store().core(self.core_uid)?;
            (core.status == ConnStatus::Ready).then(|| inspect(&core.strategies))
        });
        match result {
            None => Err(PurgeStop::Abandoned),
            Some(None) => Err(PurgeStop::Failed(step, PurgeFail::CoreLost)),
            Some(Some(result)) => Ok(result),
        }
    }

    /// Send soft-deletes until the strategy has no addressable rows left.
    ///
    /// Each pass waits for the ids IT sent to leave the replica — not for the whole re-read to come
    /// back empty. The strategy is still live during the first pass, so trades keep closing into
    /// it, and a wait for global emptiness would time out on rows it never sent. The outer loop is
    /// what picks those up, and it is bounded: if rows still remain after the last pass the step
    /// fails, rather than deleting a strategy whose trades are still in the report.
    async fn purge_rows(&mut self, cx: &mut AsyncApp, step: PurgeStep) -> Result<(), PurgeStop> {
        for pass in 0..PURGE_PASSES {
            let ids = self.rec_ids(step).await?;
            if ids.is_empty() {
                return Ok(());
            }
            if pass + 1 == PURGE_PASSES {
                // The final pass is verification-only. Rows still remain after both allowed
                // send-and-confirm batches, so stop with the strategy alive for a safe retry.
                return Err(PurgeStop::Failed(step, PurgeFail::Confirm));
            }
            let sent: std::collections::HashSet<i64> = ids.iter().copied().collect();
            // Baseline read BEFORE the send: the report writer runs on its own thread, so its
            // commit can land between the send and a baseline taken after it. The wait would then
            // see no further commit, never re-read, and report a timeout for rows the core had
            // already cleared.
            let baseline = self.report_generation(cx);
            self.send(cx, step, |session| {
                session.set_report_rows_deleted_ids(self.core_uid, true, ids)
            })?;
            self.await_rows_gone(cx, step, &sent, baseline).await?;
        }
        Ok(())
    }

    /// Wait until none of `sent` is addressable any more.
    ///
    /// The core commits the batch and echoes it back, and that echo is what clears the rows
    /// locally — so those ids leaving the replica IS the confirmation.
    async fn await_rows_gone(
        &mut self,
        cx: &mut AsyncApp,
        step: PurgeStep,
        sent: &std::collections::HashSet<i64>,
        baseline: u64,
    ) -> Result<(), PurgeStop> {
        // Wall-clock, because a slow scan advances no counter of its own.
        let deadline = Instant::now() + STEP_TIMEOUT;
        let mut seen_generation = baseline;
        loop {
            self.executor.timer(POLL).await;
            if Instant::now() >= deadline {
                return Err(PurgeStop::Failed(step, PurgeFail::Confirm));
            }
            // Re-reading costs a scan of this core's history, so only look when something was
            // committed since the last look. The counter is global to the reports writer, so this
            // thins the reads rather than making them exact.
            let generation = self.report_generation(cx);
            if generation == seen_generation {
                continue;
            }
            seen_generation = generation;
            let left = self.rec_ids(step).await?;
            if !left.iter().any(|id| sent.contains(id)) {
                return Ok(());
            }
        }
    }

    /// Wait until the core's snapshot of this strategy satisfies `check`.
    ///
    /// A core that leaves `Ready`, or disappears from the store entirely, fails as `CoreLost`:
    /// `store.core(..)` would otherwise yield `None`, which both predicates would read as "the
    /// strategy is gone" and report as success for a command that may never have left the machine.
    async fn await_strategy(
        &self,
        cx: &mut AsyncApp,
        step: PurgeStep,
        check: impl Fn(Option<&StrategyRow>, u64) -> bool,
    ) -> Result<(), PurgeStop> {
        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            let outcome = self
                .with_view(cx, |view, cx| {
                    let backend = view.backend.read(cx);
                    let store = backend.session.store();
                    let core = store.core(self.core_uid)?;
                    if core.status != ConnStatus::Ready {
                        return None;
                    }
                    let strategy = core.strategies.iter().find(|s| s.id == self.sid);
                    Some(check(strategy, core.strategies_ack_rev))
                })
                // The view is gone; nothing is left to publish to.
                .unwrap_or(Some(true));
            match outcome {
                None => return Err(PurgeStop::Failed(step, PurgeFail::CoreLost)),
                Some(true) => return Ok(()),
                Some(false) => {}
            }
            if Instant::now() >= deadline {
                return Err(PurgeStop::Failed(step, PurgeFail::Confirm));
            }
            self.executor.timer(POLL).await;
        }
    }

    /// The whole ordered sequence.
    async fn run(&mut self, cx: &mut AsyncApp) -> Result<(), PurgeStop> {
        self.open_step(cx, PurgeStep::Rows)?;
        self.purge_rows(cx, PurgeStep::Rows).await?;

        self.open_step(cx, PurgeStep::Disable)?;
        // Read BEFORE the send: the core's acknowledgement is the only proof the checkbox change
        // was committed, and `StrategyRow.checked` is not — the protocol library flips its own
        // snapshot the instant `set_checked` is called, before anything is transmitted.
        let before = self.with_view(cx, |view, cx| {
            let backend = view.backend.read(cx);
            let core = backend.session.store().core(self.core_uid)?;
            let checked = core
                .strategies
                .iter()
                .find(|s| s.id == self.sid)
                .is_some_and(|s| s.checked);
            Some((core.strategies_ack_rev, checked))
        });
        let (ack_before, was_checked) = match before.flatten() {
            Some(before) => before,
            None => return Err(PurgeStop::Failed(PurgeStep::Disable, PurgeFail::CoreLost)),
        };
        // An already-disabled strategy needs no delta, and the protocol sends no packet for an
        // empty one — so waiting for an acknowledgement that will never arrive would time out on
        // the most ordinary case there is: deleting a strategy the user turned off long ago.
        let (core_uid, sid) = (self.core_uid, self.sid);
        if was_checked {
            self.send(cx, PurgeStep::Disable, |session| {
                // `None`: a core-wide stop is NOT what deleting one strategy means.
                session.apply_strategies(core_uid, vec![(sid, false)], None)
            })?;
            // The acknowledgement is per CORE, not per strategy — the protocol event drops the ids
            // it arrived with — so the flag is checked too, and a strategy that vanished entirely
            // counts as no longer enabled. Residual gap: an unrelated checkbox acknowledgement
            // arriving inside this window can satisfy the wait. It cannot be closed from here;
            // it needs `StratEvent::CheckedEcho` to carry its ids upstream. The consequence is
            // bounded — the delete step then waits for the strategy to actually disappear, so a
            // core that refused the delete reports a failure rather than a false success.
            self.await_strategy(cx, PurgeStep::Disable, |strategy, ack| {
                strategy.is_none() || (ack != ack_before && strategy.is_some_and(|s| !s.checked))
            })
            .await?;
        }

        // Trades that closed between the first purge and the disable would otherwise outlive the
        // strategy that made them.
        self.open_step(cx, PurgeStep::Sweep)?;
        self.purge_rows(cx, PurgeStep::Sweep).await?;

        self.open_step(cx, PurgeStep::Delete)?;
        // Capture provenance immediately before deletion. Analytics labels and report rows do not
        // carry the core's authoritative folder spelling, and a strategy already absent here must
        // never fabricate a folder target.
        let (deleted_folder, before_delete) =
            self.inspect_strategies(cx, PurgeStep::Delete, |strategies| {
                let folder = strategies
                    .iter()
                    .find(|strategy| strategy.id == sid)
                    .map(|strategy| strategy.folder_path.clone());
                let placements = strategies
                    .iter()
                    .map(|strategy| (strategy.id, strategy.folder_path.clone()))
                    .collect();
                (folder, placements)
            })?;
        if deleted_folder.is_some() {
            self.send(cx, PurgeStep::Delete, |session| {
                session.delete_strategy_if_unchanged(core_uid, sid, before_delete)
            })?;
        }
        self.await_strategy(cx, PurgeStep::Delete, |strategy, _| strategy.is_none())
            .await?;

        // Folder deletion has no dedicated acknowledgement. Capture the complete placement state
        // with the fresh post-delete snapshot so the feed thread can fail closed if this terminal
        // queues a create, move, or delete before it serializes the folder-wide command.
        let cleanup = self.inspect_strategies(cx, PurgeStep::Delete, |remaining| {
            let folder = deletable_folder_after(deleted_folder.as_deref(), remaining)?;
            let expected_placements = remaining
                .iter()
                .map(|strategy| (strategy.id, strategy.folder_path.clone()))
                .collect();
            Some((folder, expected_placements))
        })?;
        if let Some((folder, expected_placements)) = cleanup {
            self.send_empty_folder(cx, folder, expected_placements)?;
        }
        Ok(())
    }
}

impl AnalyticsView {
    /// Drop the open purge operation, whatever state it is in.
    pub(super) fn close_purge(&mut self, cx: &mut Context<Self>) {
        self.strat_purge = None;
        cx.notify();
    }

    /// Publish into the open operation only when it is still the one that started the work.
    fn publish_purge(&mut self, seq: u64, cx: &mut Context<Self>, edit: impl FnOnce(&mut PurgeOp)) {
        if let Some(op) = self.strat_purge.as_mut().filter(|op| op.seq == seq) {
            edit(op);
            cx.notify();
        }
    }

    /// Open the confirmation for one strategy row and start counting its trades.
    ///
    /// Args:
    ///     core_uid: Core the strategy belongs to.
    ///     sid: Live strategy id.
    ///     name: Display name already resolved by the row.
    ///     core_name: Core label already resolved by the row.
    ///     period_trades: Trades the row counts for the selected period.
    ///     window: Analytics owner window.
    ///     cx: Analytics context.
    pub(in crate::analytics) fn open_purge_dialog(
        &mut self,
        core_uid: u64,
        sid: u64,
        name: String,
        core_name: String,
        period_trades: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.purge_seq = self.purge_seq.wrapping_add(1);
        let seq = self.purge_seq;
        self.strat_purge = Some(PurgeOp {
            seq,
            core_uid,
            sid,
            name,
            core_name,
            period_trades,
            legacy_rows: 0,
            state: PurgeState::Counting,
        });
        self.purge_dialog(window, cx);

        let key = db::ReportStrategyKey {
            core_uid,
            strategy_id: sid as i64,
        };
        // `overlay = false`: counting a few thousand rec ids must not dim the whole window behind
        // a dialog the user is already looking at.
        self.spawn_db(
            false,
            cx,
            move || read_purge_rows(None, key).1,
            move |this, rows, cx| {
                this.publish_purge(seq, cx, |op| {
                    op.state = match rows {
                        Ok(rows) => {
                            op.legacy_rows = rows.legacy_rows;
                            PurgeState::Ready {
                                total: rows.rec_ids.len(),
                            }
                        }
                        // A read failure must never render as "0 trades": the user would confirm a
                        // deletion whose size nobody knows.
                        Err(fail) => PurgeState::CountFailed(fail.to_string()),
                    };
                });
            },
        );
    }

    /// Open the MoonUI Root-owned confirmation dialog.
    ///
    /// Everything that changes while the operation runs lives inside the content closure: the
    /// dialog's own `footer` takes a value built once, which would freeze the buttons on the state
    /// they had when the dialog opened.
    fn purge_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        window.open_unique_moon_dialog(
            "an-strategy-purge-dialog",
            cx,
            move |dialog, _window, cx| {
                let p = MoonPalette::active(cx);
                let (content_view, cancel_view, close_view) =
                    (view.clone(), view.clone(), view.clone());
                dialog
                    .w(px(460.0))
                    // A destructive confirmation offers Cancel, not a bare dismiss.
                    .close_button(false)
                    .overlay(true)
                    .overlay_closable(true)
                    .bg(moon(p.shell_high))
                    .border_color(moon(p.border))
                    .rounded(design::r_container(cx))
                    .text_color(moon(p.text))
                    .header(
                        div()
                            .w_full()
                            .py_2()
                            .border_b_1()
                            .border_color(moon(p.border))
                            .child(t!("analytics.purge.title").to_string()),
                    )
                    .on_cancel(move |_, _, cx| {
                        cancel_view.update(cx, |this, cx| this.close_purge(cx));
                        true
                    })
                    .on_close(move |_, _, cx| {
                        close_view.update(cx, |this, cx| this.close_purge(cx));
                    })
                    .content(move |content, _window, cx| {
                        content.child(purge_body(content_view.clone(), cx))
                    })
            },
        );
    }

    /// Start the ordered sequence for the open operation.
    fn confirm_purge(&mut self, cx: &mut Context<Self>) {
        let Some(op) = self.strat_purge.as_mut() else {
            return;
        };
        let (seq, core_uid, sid) = (op.seq, op.core_uid, op.sid);
        op.state = PurgeState::Running(PurgeStep::Rows);
        cx.notify();

        // The sequence's own reads bypass `spawn_db`, so count them here: the refresh gate uses
        // `db_ops` to keep a full-period Analytics reload from overlapping other database work.
        self.db_ops += 1;
        cx.spawn(async move |view, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut run = PurgeRun {
                view: view.clone(),
                executor,
                seq,
                core_uid,
                sid,
                key: db::ReportStrategyKey {
                    core_uid,
                    strategy_id: sid as i64,
                },
                reader: None,
            };
            let outcome = run.run(cx).await;
            cx.update(|cx| {
                let _ = view.update(cx, |this, cx| {
                    this.db_ops = this.db_ops.saturating_sub(1);
                    match outcome {
                        Ok(()) => {
                            this.publish_purge(seq, cx, |op| op.state = PurgeState::Done);
                            // The row is built from trades that are now hidden and a strategy that
                            // is now gone; ask for the refresh that drops it rather than waiting
                            // for the next periodic one.
                            this.mark_report_data_stale();
                            this.request_report_refresh(false, cx);
                        }
                        Err(PurgeStop::Abandoned) => {}
                        Err(PurgeStop::FolderSend) => {
                            this.publish_purge(seq, cx, |op| {
                                op.state = PurgeState::FolderSendFailed;
                            });
                            this.mark_report_data_stale();
                            this.request_report_refresh(false, cx);
                        }
                        Err(PurgeStop::Failed(step, fail)) => {
                            this.publish_purge(seq, cx, |op| {
                                op.state = PurgeState::Failed { step, fail };
                            });
                        }
                    }
                });
            });
        })
        .detach();
    }
}

/// Render the dialog: the subject, then either the confirmation or the running progress.
fn purge_body(view: Entity<AnalyticsView>, cx: &mut gpui::App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let Some(op) = view.read(cx).strat_purge.as_ref() else {
        return div().into_any_element();
    };

    let mut body = v_flex().w_full().gap_2().text_color(moon(p.text)).child(
        div()
            .w_full()
            .child(format!("{} — {}", op.name, op.core_name)),
    );

    match &op.state {
        PurgeState::Counting => {
            body = body.child(
                div()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.purge.counting").to_string()),
            );
        }
        PurgeState::CountFailed(msg) => {
            body = body.child(MoonAlert::error("an-purge-count-error", msg.clone()));
        }
        PurgeState::Ready { total } => {
            for (index, line) in purge_summary_lines(*total, op.period_trades, op.legacy_rows)
                .into_iter()
                .enumerate()
            {
                // The legacy caveat is the only line after the count, and it is a warning.
                let color = if index == 0 { p.text } else { p.orange };
                body = body.child(div().text_color(moon(color)).child(line));
            }
            // Only what the operation will DO. An "it can be restored" note used to sit here and
            // was removed deliberately: the terminal offers no route back for these rows, so the
            // sentence promised the user something that does not exist.
            for note in [t!("analytics.purge.steps").to_string()] {
                body = body.child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(note),
                );
            }
        }
        state @ (PurgeState::Running(_)
        | PurgeState::Done
        | PurgeState::FolderSendFailed
        | PurgeState::Failed { .. }) => {
            body = body.child(purge_progress(state, p));
            if matches!(state, PurgeState::Done) {
                body = body.child(
                    div()
                        .text_color(moon(p.green))
                        .child(t!("analytics.purge.done").to_string()),
                );
                // The rows the protocol could not address are still there; saying so beside the
                // success line is what keeps "done" from reading as "every trade is gone".
                if op.legacy_rows > 0 {
                    body =
                        body.child(div().text_color(moon(p.orange)).child(
                            t!("analytics.purge.legacy_note", n = op.legacy_rows).to_string(),
                        ));
                }
            }
            if let PurgeState::Failed { step, fail } = state {
                let key = match fail {
                    PurgeFail::Send => "analytics.purge.send_failed",
                    PurgeFail::Confirm => "analytics.purge.timeout",
                    PurgeFail::CoreLost => "analytics.purge.core_lost",
                };
                let message = match fail {
                    PurgeFail::CoreLost => t!(key).to_string(),
                    _ => t!(key, step = t!(step.label_key())).to_string(),
                };
                body = body.child(MoonAlert::error("an-purge-error", message));
            }
            if matches!(state, PurgeState::FolderSendFailed) {
                body = body.child(MoonAlert::error(
                    "an-purge-folder-error",
                    t!("analytics.purge.folder_send_failed").to_string(),
                ));
            }
        }
    }

    body.child(purge_actions(view, &op.state, p))
        .into_any_element()
}

/// The four confirmable steps with their state: confirmed, in flight, failed, or still ahead.
///
/// Everything before the step in flight has been confirmed by the core, which is what makes it
/// safe to tell the user those steps stand.
fn purge_progress(state: &PurgeState, p: MoonPalette) -> AnyElement {
    let stopped_at = match state {
        PurgeState::Running(step) => Some((*step, false)),
        PurgeState::Failed { step, .. } => Some((*step, true)),
        _ => None,
    };
    let mut list = v_flex().w_full().gap_1();
    for step in PurgeStep::ORDER {
        // Colour alone does not say WHICH step is running — the finished ones are coloured too,
        // and on a fast core the whole list can look uniformly "coloured in". The marker is the
        // part that reads at a glance; it is composed here rather than stored in the dictionary,
        // which never holds glyphs.
        let (marker, color, weight) = match stopped_at {
            Some((at, failed)) if step == at => {
                if failed {
                    ("x", p.red, FontWeight::SEMIBOLD)
                } else {
                    (">", p.text, FontWeight::SEMIBOLD)
                }
            }
            Some((at, _)) if step < at => ("+", p.green, FontWeight::NORMAL),
            Some(_) => ("-", p.text_muted, FontWeight::NORMAL),
            // Nothing in flight and nothing failed: the whole sequence is behind us.
            None => ("+", p.green, FontWeight::NORMAL),
        };
        list = list.child(
            h_flex()
                .w_full()
                .gap_2()
                .items_start()
                .text_color(moon(color))
                .font_weight(weight)
                // Fixed width so the labels line up whatever marker each row carries.
                .child(div().flex_none().w(px(12.0)).child(marker))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(t!(step.label_key()).to_string()),
                ),
        );
    }
    list.into_any_element()
}

/// Cancel / confirm, or a single Close once the sequence has ended.
fn purge_actions(view: Entity<AnalyticsView>, state: &PurgeState, p: MoonPalette) -> AnyElement {
    let confirmable = matches!(state, PurgeState::Ready { .. });
    let ended = matches!(
        state,
        PurgeState::Done
            | PurgeState::FolderSendFailed
            | PurgeState::Failed { .. }
            | PurgeState::CountFailed(_)
    );

    let close_view = view.clone();
    let confirm_view = view;
    let mut row = h_flex()
        .w_full()
        .pt_2()
        .justify_end()
        .gap_2()
        .border_t_1()
        .border_color(moon_alpha(p.border, 0.6))
        .child(
            MoonButton::new("an-purge-cancel")
                .ghost()
                .size(MoonButtonSize::Micro)
                .label(if ended {
                    t!("dialogs.close").to_string()
                } else {
                    t!("dialogs.cancel").to_string()
                })
                .on_click(move |_, window, cx| {
                    close_view.update(cx, |this, cx| this.close_purge(cx));
                    window.close_dialog(cx);
                })
                .render(),
        );
    if !ended {
        row = row.child(
            MoonButton::new("an-purge-ok")
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Danger)
                // Never confirm a deletion whose size is still unknown or already under way.
                .disabled(!confirmable)
                .label(t!("analytics.purge.ok").to_string())
                .on_click(move |_, _window, cx| {
                    confirm_view.update(cx, |this, cx| this.confirm_purge(cx));
                })
                .render(),
        );
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests;
