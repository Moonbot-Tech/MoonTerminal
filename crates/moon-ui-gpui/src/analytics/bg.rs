//! The shared scaffold of a background DB read: run a closure on the background
//! executor, then hand the result back to the view on the UI thread.
//!
//! Every Analytics recompute follows the same envelope: count the busy overlay,
//! spawn the read, let the caller validate and store the completed snapshot, then
//! balance the overlay and reconsider deferred report work. Applying the result
//! first is what prevents a newly scheduled refresh from retiring the completion
//! that just made progress. Every recompute path goes through here; what varies per
//! call, including its scope generation and destination, stays inside `store`. The
//! two filter suggestion sweeps and write paths keep their envelopes inline because
//! their completions coordinate multiple generations or send commands to cores.

use gpui::{AppContext as _, Context};

use super::AnalyticsView;

impl AnalyticsView {
    /// Run `db` on the background executor and `store` its result on the view.
    ///
    /// `overlay` — whether the read counts toward the blocking busy overlay
    /// (`op_started`/`op_finished`); a read that must not dim the window (a selection
    /// cue, a debounced rescan) passes `false`.
    ///
    /// `store` runs before the overlay is balanced and before deferred report work is scheduled,
    /// on every completion — stale ones included. It must check its own scope generation before
    /// publishing the result.
    ///
    /// Every task increments `db_ops`, including `overlay = false` selection and coin-KPI scans.
    /// The automatic report refresh gate uses that count to avoid overlapping full-period work.
    ///
    /// Args:
    ///     overlay: Whether the task participates in the delayed blocking overlay.
    ///     cx: GPUI context used to run the task and publish its result.
    ///     db: Database work executed on the background executor.
    ///     store: UI-thread completion that validates and stores the result.
    pub(super) fn spawn_db<R: Send + 'static>(
        &mut self,
        overlay: bool,
        cx: &mut Context<Self>,
        db: impl FnOnce() -> R + Send + 'static,
        store: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        self.db_ops += 1;
        if overlay {
            self.op_started();
        }
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { db() }).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.db_ops = this.db_ops.saturating_sub(1);
                    store(this, result, cx);
                    if overlay {
                        this.op_finished(cx);
                    }
                    this.schedule_report_refresh(cx);
                });
            });
        })
        .detach();
    }
}
