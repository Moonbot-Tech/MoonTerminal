//! One low-priority, cancellable base snapshot prepares the next Analytics page during idle time.

use std::sync::Arc;

use gpui::{AppContext as _, Context};
use moon_core::db::analytics::{self, AnalyticsRead, CalendarRead};
use moon_core::db::{ReadCancellation, ReadFail, with_read_cancellation};

use super::cache::{CacheKey, CachedCalendar, CachedRead, CachedResult, with_cache};
use super::refresh::RefreshUrgency;
use super::snapshots::SnapshotRequest;
use super::{AnalyticsView, Tab};

/// A speculative read keeps its slot until cancellation has actually completed.
struct Prefetch {
    key: CacheKey,
    target: Tab,
    cancellation: ReadCancellation,
    promoted: bool,
}

impl Prefetch {
    /// A failed promoted owner still owes its visible base across data publications, not scopes.
    fn needs_visible_retry(&self, visible: Tab, key: &CacheKey) -> bool {
        self.promoted && self.target == visible && self.key.same_scope(key)
    }
}

/// Per-window speculative work and the scopes already attempted for its current visible request.
#[derive(Default)]
pub(super) struct PrefetchState {
    active: Option<Prefetch>,
    visible_scope: Option<CacheKey>,
    attempted: Vec<CacheKey>,
}

impl PrefetchState {
    /// Include cancelled-but-draining workers in the shared refresh interlock.
    pub(super) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Start a new bounded attempt set only when the user's scope or data publication changed.
    fn set_visible_scope(&mut self, key: CacheKey) {
        if self.visible_scope.as_ref() != Some(&key) {
            self.visible_scope = Some(key);
            self.attempted.clear();
        }
    }

    /// Claim the sole worker slot, remembering failures and oversized successes alike.
    fn start(&mut self, key: CacheKey, target: Tab) -> Option<ReadCancellation> {
        if self.active.is_some() || self.attempted.contains(&key) {
            return None;
        }
        let cancellation = ReadCancellation::new();
        self.attempted.push(key.clone());
        self.active = Some(Prefetch {
            key,
            target,
            cancellation: cancellation.clone(),
            promoted: false,
        });
        Some(cancellation)
    }

    /// Stop speculation without releasing its slot before SQLite acknowledges the cancellation.
    fn cancel(&mut self) {
        if let Some(active) = &self.active {
            active.cancellation.cancel();
        }
    }

    /// Yield background preparation without retiring a base already owned by visible navigation.
    fn cancel_unpromoted(&mut self) {
        if self.active.as_ref().is_some_and(|active| !active.promoted) {
            self.cancel();
        }
    }

    /// Retain a worker that can become the selected page instead of replacing identical work.
    fn cancel_unrelated(&mut self, target: Tab) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.target != target)
        {
            self.cancel();
        }
    }

    /// A retained destination is reusable only while its complete query identity still matches.
    fn cancel_unless_matches(&mut self, key: &CacheKey) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.key != *key)
        {
            self.cancel();
        }
    }

    /// Promote only an exact, still-live request to foreground ownership.
    fn join(&mut self, key: &CacheKey) -> bool {
        if let Some(active) = &mut self.active
            && active.key == *key
            && !active.cancellation.is_cancelled()
        {
            active.promoted = true;
            return true;
        }
        false
    }

    /// A late completion cannot release another worker's slot.
    fn finish(&mut self, cancellation: &ReadCancellation) -> Option<Prefetch> {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.cancellation.same_request(cancellation))
        {
            self.active.take()
        } else {
            None
        }
    }
}

impl Drop for PrefetchState {
    /// Closing the Analytics window must stop its otherwise-detached speculative database read.
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Preserve every classified compound failure instead of turning prefetch failure into empty data.
fn analytics_failures<T>(read: AnalyticsRead<T>) -> Vec<ReadFail> {
    read.data
        .err()
        .into_iter()
        .chain(read.undated.err())
        .chain(read.cores.and_then(Result::err))
        .collect()
}

/// Calendar has a different primary field but the same independently classified metadata errors.
fn calendar_failures(read: CalendarRead) -> Vec<ReadFail> {
    read.period
        .err()
        .into_iter()
        .chain(read.undated.err())
        .chain(read.cores.and_then(Result::err))
        .collect()
}

/// Compute compact page bases only; speculative work never starts tuner searches or tape replay.
fn read_snapshot(request: &SnapshotRequest) -> Result<CachedResult, Vec<ReadFail>> {
    match request.tab {
        Tab::Summary => CachedRead::from_analytics(analytics::summary_data_with_replica_scope(
            &request.q,
            true,
            request.replica_hidden.as_deref(),
        ))
        .map(|read| CachedResult::Summary(Arc::new(read)))
        .map_err(|read| analytics_failures(*read)),
        Tab::Strategies => {
            CachedRead::from_analytics(analytics::strategy_base_data_with_replica_scope(
                &request.q,
                true,
                request.replica_hidden.as_deref(),
            ))
            .map(|read| CachedResult::Strategies(Arc::new(read)))
            .map_err(|read| analytics_failures(*read))
        }
        Tab::Calendar => {
            CachedCalendar::from_calendar(analytics::calendar_data_with_replica_scope(
                &request.q,
                request.previous.as_ref(),
                request.hourly,
                true,
                request.replica_hidden.as_deref(),
            ))
            .map(|read| CachedResult::Calendar(Arc::new(read)))
            .map_err(|read| calendar_failures(*read))
        }
    }
}

/// Prepare the nearest likely page first without loading every possible Analytics period.
fn next_pages(visible: Tab) -> [Tab; 2] {
    match visible {
        Tab::Summary => [Tab::Strategies, Tab::Calendar],
        Tab::Strategies => [Tab::Summary, Tab::Calendar],
        Tab::Calendar => [Tab::Strategies, Tab::Summary],
    }
}

impl AnalyticsView {
    /// Cancel speculative work whose input changed, keeping its interlock until acknowledgment.
    pub(super) fn cancel_prefetch(&mut self) {
        self.prefetch.cancel();
    }

    /// A queued foreground axis outranks speculation, while a joined visible base keeps ownership.
    pub(super) fn yield_speculative_prefetch(&mut self) {
        self.prefetch.cancel_unpromoted();
    }

    /// A tab click preserves a matching speculative destination so its foreground read can join it.
    pub(super) fn cancel_unrelated_prefetch(&mut self, tab: Tab) {
        self.prefetch.cancel_unrelated(tab);
    }

    /// Stop a same-page speculation when a period, filter or publication changed its query.
    pub(super) fn cancel_prefetch_unless_matches(&mut self, key: &CacheKey) {
        self.prefetch.cancel_unless_matches(key);
    }

    /// Avoid a duplicate database scan when the selected page is already being prepared.
    pub(super) fn join_prefetch(&mut self, key: &CacheKey) -> bool {
        self.cancel_prefetch_unless_matches(key);
        self.prefetch.join(key)
    }

    /// Start one next-page base only after visible data, user work and queued refreshes have settled.
    ///
    /// Args:
    ///     cx: Analytics context used to launch and publish the detached background read.
    pub(super) fn schedule_prefetch(&mut self, cx: &mut Context<Self>) {
        if self.prefetch.is_active()
            || self.db_ops > 0
            || self.busy_ops > 0
            || self.report_refresh.has_pending()
        {
            return;
        }
        let visible_ready = match self.tab {
            Tab::Summary => self.data.data().is_some() || self.data.split().is_some(),
            Tab::Strategies => {
                self.strategy_data.data().is_some() || self.strategy_data.split().is_some()
            }
            Tab::Calendar => self.cal_days.data().is_some() || self.cal_days.split().is_some(),
        };
        if !visible_ready {
            return;
        }
        let visible_scope = self.snapshot_request(self.tab).key;
        self.prefetch.set_visible_scope(visible_scope);
        for target in next_pages(self.tab) {
            let revisions = self.cache_revisions();
            let request = self.snapshot_request(target);
            if !request.key.is_reusable() {
                return;
            }
            if self.cache_revisions() != revisions {
                return;
            }
            if self.prefetch.attempted.contains(&request.key)
                || with_cache(|cache| cache.has_fresh(&request.key))
            {
                continue;
            }
            let Some(cancellation) = self.prefetch.start(request.key.clone(), target) else {
                return;
            };
            let worker_cancellation = cancellation.clone();
            cx.spawn(async move |this, cx| {
                let (request, result) = cx
                    .background_spawn(async move {
                        let result =
                            with_read_cancellation(worker_cancellation, || read_snapshot(&request));
                        (request, result)
                    })
                    .await;
                cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| {
                        let Some(completed) = this.prefetch.finish(&cancellation) else {
                            return;
                        };
                        let mut published = false;
                        if !completed.cancellation.is_cancelled() {
                            match result {
                                Ok(result) if this.cache_revisions() == revisions => {
                                    with_cache(|cache| {
                                        cache.insert(request.key.clone(), result.clone())
                                    });
                                    if completed.promoted {
                                        // Publish even an oversized snapshot rejected by the cache.
                                        published =
                                            this.publish_prefetched_snapshot(&request, result, cx);
                                    }
                                }
                                Ok(_) => {}
                                Err(failures) => {
                                    log::debug!(
                                        "analytics: prefetch {} not settled: {:?}",
                                        completed.target.id(),
                                        failures
                                    );
                                }
                            }
                        }
                        if !published
                            && completed
                                .needs_visible_retry(this.tab, &this.snapshot_request(this.tab).key)
                        {
                            // Selection work can cancel a promoted base after navigation already
                            // acknowledged its request. Keep that base owed, but let the shared
                            // gate wait for the active axis, search or write before retrying it.
                            this.request_report_refresh(RefreshUrgency::User, false, cx);
                        }
                        this.schedule_report_refresh(cx);
                        this.schedule_prefetch(cx);
                    });
                });
            })
            .detach();
            break;
        }
    }
}

#[cfg(test)]
mod tests;
