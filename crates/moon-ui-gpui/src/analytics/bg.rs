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

use std::collections::HashMap;

use gpui::{AppContext as _, Context};
use moon_core::db::{ReadCancellation, with_read_cancellation};

use super::AnalyticsView;

/// Independently replaceable Analytics destinations.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) enum ReadLane {
    Summary,
    StrategyBase,
    Calendar,
    FilterKpi,
    FilterHistogram,
    Coins,
    CoinKpi,
    CoinPicked,
    Time,
}

/// Active cancellation tokens keyed by the UI state each request may publish.
#[derive(Default)]
pub(super) struct LatestReads {
    active: HashMap<ReadLane, ReadCancellation>,
}

impl LatestReads {
    /// Replace every listed destination with one new shared request token.
    ///
    /// A compound request may own several lanes. Replacing any one cancels its connection and
    /// retires every lane owned by that same token, allowing the caller to reschedule components
    /// that remain required.
    ///
    /// Args:
    ///     lanes: Destinations produced by the new request.
    ///
    /// Returns:
    ///     New token shared by every listed destination.
    pub(super) fn replace(&mut self, lanes: &[ReadLane]) -> ReadCancellation {
        self.retire(lanes);
        let token = ReadCancellation::new();
        for lane in lanes {
            self.active.insert(*lane, token.clone());
        }
        token
    }

    /// Cancel and remove every request that owns one of the supplied destinations.
    ///
    /// Args:
    ///     lanes: Destinations whose owning requests must be retired.
    ///
    /// Returns:
    ///     Every lane removed through direct or compound ownership.
    fn retire(&mut self, lanes: &[ReadLane]) -> Vec<ReadLane> {
        let replaced = lanes
            .iter()
            .filter_map(|lane| self.active.get(lane).cloned())
            .fold(Vec::<ReadCancellation>::new(), |mut tokens, token| {
                if !tokens.iter().any(|seen| seen.same_request(&token)) {
                    tokens.push(token);
                }
                tokens
            });
        for token in &replaced {
            token.cancel();
        }
        let mut retired = Vec::new();
        self.active.retain(|lane, token| {
            let remove = replaced.iter().any(|replaced| replaced.same_request(token));
            if remove {
                retired.push(*lane);
            }
            !remove
        });
        retired
    }

    /// Cancel requests owning any listed destination.
    ///
    /// Args:
    ///     lanes: Invalidated destinations.
    ///
    /// Returns:
    ///     Every lane retired through direct or compound ownership.
    pub(super) fn cancel(&mut self, lanes: &[ReadLane]) -> Vec<ReadLane> {
        self.retire(lanes)
    }

    /// Forget a completed request without disturbing a newer replacement.
    ///
    /// Args:
    ///     completed: Token carried by the completion.
    pub(super) fn finish(&mut self, completed: &ReadCancellation) {
        self.active
            .retain(|_, current| !current.same_request(completed));
    }
}

impl Drop for LatestReads {
    /// Cancel every detached worker whose Analytics destination is being destroyed.
    fn drop(&mut self) {
        for cancellation in self.active.values() {
            cancellation.cancel();
        }
    }
}

impl AnalyticsView {
    /// Cancel every replaceable read affected by a shared Analytics scope change.
    pub(super) fn cancel_latest_reads(&mut self) {
        self.latest_reads.cancel(&[
            ReadLane::Summary,
            ReadLane::StrategyBase,
            ReadLane::Calendar,
            ReadLane::FilterKpi,
            ReadLane::FilterHistogram,
            ReadLane::Coins,
            ReadLane::CoinKpi,
            ReadLane::CoinPicked,
            ReadLane::Time,
        ]);
    }

    /// Cancel replaceable Strategy-axis reads when its selection scope changes.
    pub(super) fn cancel_axis_reads(&mut self) {
        self.latest_reads.cancel(&[
            ReadLane::FilterKpi,
            ReadLane::FilterHistogram,
            ReadLane::Coins,
            ReadLane::CoinKpi,
            ReadLane::CoinPicked,
            ReadLane::Time,
        ]);
    }

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
            self.op_started(cx);
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

    /// Run a replaceable database read with SQLite cancellation tied to its destination lanes.
    ///
    /// Args:
    ///     lanes: UI destinations whose previous requests must be cancelled.
    ///     overlay: Whether the task participates in the delayed blocking overlay.
    ///     cx: GPUI context used to run the task and publish its result.
    ///     db: Synchronous database work executed on the background executor.
    ///     store: UI-thread completion that validates and stores the result.
    pub(super) fn spawn_latest_db<R: Send + 'static>(
        &mut self,
        lanes: &[ReadLane],
        overlay: bool,
        cx: &mut Context<Self>,
        db: impl FnOnce() -> R + Send + 'static,
        store: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        let cancellation = self.latest_reads.replace(lanes);
        let worker_cancellation = cancellation.clone();
        self.db_ops += 1;
        if overlay {
            self.op_started(cx);
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { with_read_cancellation(worker_cancellation, db) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.latest_reads.finish(&cancellation);
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

#[cfg(test)]
mod tests;
