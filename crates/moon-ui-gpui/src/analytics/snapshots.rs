//! Restore immutable aggregate results without retaining hidden GPUI views or read transactions.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui::Context;
use moon_core::db::analytics::{Query, StrategyBase, Summary, UndatedCloses};

use super::cache::{
    CacheHit, CacheKey, CacheRevisions, CachedCalendar, CachedRead, CachedResult, CachedScope,
    with_cache,
};
use super::{AnalyticsView, ProfitLoadState, Tab, tuner};

/// Exact page operands captured before a worker can outlive the selected page or its filters.
#[derive(Clone)]
pub(super) struct SnapshotRequest {
    pub(super) tab: Tab,
    pub(super) key: CacheKey,
    pub(super) report_generation: u64,
    pub(super) q: Query,
    pub(super) previous: Option<Query>,
    pub(super) hourly: bool,
    pub(super) replica_hidden: Option<Vec<u64>>,
}

/// Retained allocations still owe changed publications when their LRU entry was evicted.
fn retained_snapshot_needs_refresh(
    retained: Option<&CacheKey>,
    requested: &CacheKey,
    as_of: i64,
    now: i64,
) -> bool {
    retained.is_some_and(|key| key != requested) || now.saturating_sub(as_of) >= 60
}

impl<T> ProfitLoadState<T> {
    /// Keep derived render caches only when both the immutable allocation and its unit agree.
    ///
    /// Split totals are held by value in the view, so their restore conservatively rebuilds.
    pub(super) fn shares_snapshot(&self, cached: &CachedScope<T>) -> bool {
        match (self, cached) {
            (
                Self::Ready {
                    unit: Some(current),
                    data,
                },
                CachedScope::Comparable { unit, data: next },
            ) => current == unit && Arc::ptr_eq(data, next),
            (Self::Ready { unit: None, data }, CachedScope::Empty(next)) => Arc::ptr_eq(data, next),
            _ => false,
        }
    }

    /// Restore a shared, unit-safe result without copying a page's arrays on the UI thread.
    pub(super) fn restore(&mut self, cached: &CachedScope<T>) {
        *self = match cached {
            CachedScope::Comparable { unit, data } => Self::Ready {
                unit: Some(*unit),
                data: Arc::clone(data),
            },
            CachedScope::Empty(data) => Self::Ready {
                unit: None,
                data: Arc::clone(data),
            },
            CachedScope::Split(totals) => Self::Split((**totals).clone()),
        };
    }
}

impl Tab {
    /// Keep snapshot freshness attached to its page while users move between tabs.
    pub(super) fn snapshot_index(self) -> usize {
        match self {
            Self::Summary => 0,
            Self::Strategies => 1,
            Self::Calendar => 2,
        }
    }
}

impl AnalyticsView {
    /// Keep independent publications separate so simultaneous data changes cannot alias a hit.
    pub(super) fn cache_revisions(&self) -> CacheRevisions {
        CacheRevisions {
            reports: self
                .report_generation
                .as_ref()
                .map_or(0, |g| g.load(Ordering::Acquire)),
            valuation: self
                .valuation_generation
                .as_ref()
                .map_or(0, |g| g.load(Ordering::Acquire)),
            strategies: moon_core::strat_db::generation(),
            read_state: moon_core::db::read_state_revision(),
            valuation_attachment: moon_core::db::valuation::attach_epoch(),
        }
    }

    /// Build the same semantic identity for a foreground read and a speculative read.
    pub(super) fn snapshot_request(&self, tab: Tab) -> SnapshotRequest {
        let q = match tab {
            Tab::Summary => self.query_for(self.period),
            Tab::Strategies => self.query_for(self.strat_period),
            Tab::Calendar => self.cal_query(),
        };
        let previous = (tab == Tab::Calendar)
            .then(|| self.cal_query_prev())
            .flatten();
        let hourly = tab == Tab::Calendar && self.cal_mode == super::calendar::CalMode::Day;
        // An implicit All under a hiding preset means the replica universe minus those ids.
        // Carry that meaning even when reopening starts without a replica core list.
        let replica_hidden = self.replica_hidden_core_ids(true);
        let revisions = self.cache_revisions();
        let key = match tab {
            Tab::Summary => CacheKey::summary(&q, replica_hidden.as_deref(), revisions, true),
            Tab::Strategies => CacheKey::strategies(&q, replica_hidden.as_deref(), revisions),
            Tab::Calendar => CacheKey::calendar(
                &q,
                previous.as_ref(),
                hourly,
                replica_hidden.as_deref(),
                revisions,
            ),
        };
        SnapshotRequest {
            tab,
            key,
            report_generation: revisions.reports.wrapping_add(revisions.valuation),
            q,
            previous,
            hourly,
            replica_hidden,
        }
    }

    /// An explicit refresh bypasses a saved result while unrelated scopes retain their cache.
    pub(super) fn forget_visible_snapshot(&mut self) {
        let key = self.snapshot_request(self.tab).key;
        with_cache(|cache| cache.remove_scope(&key));
        self.cancel_prefetch();
    }

    /// Reuse an exact hit, or restore an age-labelled snapshot while its replacement is read.
    ///
    /// The return value means no foreground query is needed: a fresh hit or a matching prefetch
    /// already owns the request. Partial read errors are never represented by a cache entry.
    pub(super) fn restore_or_join_snapshot(
        &mut self,
        request: &SnapshotRequest,
        cx: &mut Context<Self>,
    ) -> bool {
        let index = request.tab.snapshot_index();
        if !request.key.is_reusable()
            || self.snapshot_keys[index].as_ref().is_some_and(|key| {
                !key.same_scope(&request.key) || !key.same_read_state(&request.key)
            })
        {
            // A preset enum survives civil rollover; its previous resolved bounds do not.
            self.retire_snapshot(request.tab);
        }
        let hit = with_cache(|cache| cache.lookup(&request.key, true));
        let initial = match request.tab {
            Tab::Summary => matches!(self.data, ProfitLoadState::Loading),
            Tab::Strategies => matches!(self.strategy_data, ProfitLoadState::Loading),
            Tab::Calendar => matches!(self.cal_days, ProfitLoadState::Loading),
        };
        if let Some(mut hit) = hit {
            // Page calculation time does not certify a throttled core-selector copy as new.
            let cores_at = match &hit.result {
                CachedResult::Summary(read) => read.cores_at,
                CachedResult::Strategies(read) => read.cores_at,
                CachedResult::Calendar(read) => read.cores_at,
            };
            hit.fresh &=
                !super::refresh::core_metadata_wait(cores_at, std::time::Instant::now()).is_zero();
            let fresh = hit.fresh;
            log::debug!("analytics: cache {} hit, fresh={fresh}", request.tab.id());
            if fresh || initial {
                self.restore_snapshot_hit(request, hit, cx);
            } else {
                // Retaining a settled allocation must not turn an expired cache lookup into
                // a clean tab entry. Keep its own publication time rather than copying older rows.
                let as_of = self.snapshot_as_of[request.tab.snapshot_index()].unwrap_or(hit.as_of);
                self.mark_snapshot_stale(request.tab, as_of);
            }
            if fresh {
                return true;
            }
        } else if !initial
            && let Some(as_of) = self.snapshot_as_of[request.tab.snapshot_index()]
            && retained_snapshot_needs_refresh(
                self.snapshot_keys[index].as_ref(),
                &request.key,
                as_of,
                moon_core::util::now_unix_ms_i64() / 1000,
            )
        {
            // Oversized or evicted pages still have a retained allocation and a bounded age.
            self.mark_snapshot_stale(request.tab, as_of);
        }
        self.join_prefetch(&request.key)
    }

    /// Mark retained figures stale independently of whether their LRU entry still exists.
    fn mark_snapshot_stale(&mut self, tab: Tab, as_of: i64) {
        self.cached_refreshing[tab.snapshot_index()] = Some(as_of);
        match tab {
            Tab::Summary => self.data_dirty = true,
            Tab::Strategies => self.strategy_dirty = true,
            Tab::Calendar => self.cal_dirty = true,
        }
    }

    /// Remove figures whose meaning or non-generation money-read certification changed.
    fn retire_snapshot(&mut self, tab: Tab) {
        let index = tab.snapshot_index();
        self.cached_refreshing[index] = None;
        self.snapshot_as_of[index] = None;
        self.snapshot_keys[index] = None;
        match tab {
            Tab::Summary => {
                self.data = ProfitLoadState::default();
                self.summary_derived = None;
                self.summary_catching_up = false;
                self.data_dirty = true;
            }
            Tab::Strategies => {
                self.strategy_data = ProfitLoadState::default();
                self.strategy_dirty = true;
            }
            Tab::Calendar => {
                self.cal_days = ProfitLoadState::default();
                self.cal_prev = crate::load_state::LoadState::default();
                self.cal_dirty = true;
            }
        }
    }

    /// Publish the page and all metadata from one successful cached snapshot.
    pub(super) fn restore_snapshot_hit(
        &mut self,
        request: &SnapshotRequest,
        hit: CacheHit,
        cx: &mut Context<Self>,
    ) {
        let tab = request.tab;
        self.cached_refreshing[tab.snapshot_index()] = (!hit.fresh).then_some(hit.as_of);
        self.snapshot_as_of[tab.snapshot_index()] = Some(hit.as_of);
        self.snapshot_keys[tab.snapshot_index()] = Some(request.key.clone());
        match hit.result {
            CachedResult::Summary(read) if tab == Tab::Summary => {
                let range = self.period.range(self.bound_zone());
                let range_moved = range != self.data_range;
                let snapshot_changed = range_moved || !self.data.shares_snapshot(&read.data);
                if range_moved {
                    self.hover_profit_bucket = None;
                    self.summary_popup_hover.reset_for_reload(true);
                }
                self.data_period = self.period;
                self.data_range = range;
                self.data.restore(&read.data);
                self.data_as_of = Some(hit.as_of);
                self.summary_catching_up = !hit.fresh;
                self.data_dirty = !hit.fresh;
                if snapshot_changed {
                    self.summary_derived = None;
                    if let Some(data) = self.data.data() {
                        let present = data.core_days.iter().map(|core| core.uid).collect();
                        self.summary_ui.retain_picks(&present);
                    }
                }
                self.restore_snapshot_metadata(&read.undated, read.cores.as_deref(), read.cores_at);
            }
            CachedResult::Strategies(read) if tab == Tab::Strategies => {
                if !self.strategy_data.shares_snapshot(&read.data) {
                    let next = match &read.data {
                        CachedScope::Comparable { data, .. } | CachedScope::Empty(data) => {
                            Some(data.strategies.as_slice())
                        }
                        CachedScope::Split(_) => None,
                    };
                    if tuner::core_names_changed(
                        self.strategy_data
                            .data()
                            .map(|data| data.strategies.as_slice()),
                        next,
                    ) {
                        self.strat_core_w = None;
                    }
                    self.strat_metric_w = None;
                    self.strat_visible = None;
                }
                self.strategy_data.restore(&read.data);
                self.strategy_data_period = self.strat_period;
                self.strategy_dirty = !hit.fresh;
                self.restore_snapshot_metadata(&read.undated, read.cores.as_deref(), read.cores_at);
            }
            CachedResult::Calendar(read) if tab == Tab::Calendar => {
                self.cal_days.restore(&read.data);
                self.cal_prev.apply(Ok(read.previous));
                self.cal_dirty = !hit.fresh;
                self.restore_snapshot_metadata(&read.undated, read.cores.as_deref(), read.cores_at);
            }
            _ => return,
        }
        self.core_refresh_needed = true;
        let wait =
            super::refresh::core_metadata_wait(self.last_cores_at, std::time::Instant::now());
        // A due core read belongs to the foreground reload already starting now. A timer(0)
        // would queue a second refresh behind that read and repeat the Strategies axis.
        if !wait.is_zero() {
            self.schedule_core_metadata_refresh(wait, cx);
        }
        cx.notify();
    }

    /// Restore selector and omission facts alongside their page without inventing new freshness.
    fn restore_snapshot_metadata(
        &mut self,
        undated: &Arc<UndatedCloses>,
        cores: Option<&Vec<(u64, String)>>,
        cores_at: Option<std::time::Instant>,
    ) {
        self.undated = Some((**undated).clone());
        self.undated_error = None;
        if let Some(cores) = cores {
            self.cores = cores.clone();
            self.last_cores_at = cores_at;
        }
    }

    /// Complete a successful base read, retaining shared allocations even when the LRU is full.
    pub(super) fn publish_snapshot(
        &mut self,
        request: &SnapshotRequest,
        result: CachedResult,
        chain_axis: bool,
        cx: &mut Context<Self>,
    ) {
        let current = self.snapshot_request(request.tab);
        if !request.key.same_scope(&current.key) || !request.key.same_read_state(&current.key) {
            self.retire_snapshot(request.tab);
            self.request_report_refresh(super::refresh::RefreshUrgency::User, false, cx);
            return;
        }
        let fresh = request.key == current.key;
        let as_of = moon_core::util::now_unix_ms_i64() / 1000;
        let result = self.with_snapshot_cores(result);
        // Generation checks precede insertion: a successful old read cannot replace a newer
        // cache entry or certify old metadata as belonging to a later publication.
        if fresh {
            with_cache(|cache| cache.insert(request.key.clone(), result.clone()));
        }
        self.restore_snapshot_hit(
            request,
            CacheHit {
                result,
                fresh,
                as_of,
            },
            cx,
        );
        self.settle_report_refresh_retry(false, cx);
        if !fresh {
            self.request_report_refresh(super::refresh::RefreshUrgency::Writer, false, cx);
        }
        if chain_axis
            && request.tab == Tab::Strategies
            && self.tab == Tab::Strategies
            && self.strategy_data.split().is_none()
            && !(super::probe_selects_strategy() && self.probe_select_first(cx))
        {
            self.reload_axis_after_report(self.strat_mode, false, cx);
        }
    }

    /// Attach already-discovered selector metadata to throttled successful snapshots.
    fn with_snapshot_cores(&self, result: CachedResult) -> CachedResult {
        let fallback = || Some(Arc::new(self.cores.clone()));
        let cores_at = self.last_cores_at;
        match result {
            CachedResult::Summary(read) if read.cores.is_none() => {
                CachedResult::Summary(Arc::new(CachedRead::<Summary> {
                    cores: fallback(),
                    cores_at,
                    ..(*read).clone()
                }))
            }
            CachedResult::Strategies(read) if read.cores.is_none() => {
                CachedResult::Strategies(Arc::new(CachedRead::<StrategyBase> {
                    cores: fallback(),
                    cores_at,
                    ..(*read).clone()
                }))
            }
            CachedResult::Calendar(read) if read.cores.is_none() => {
                CachedResult::Calendar(Arc::new(CachedCalendar {
                    cores: fallback(),
                    cores_at,
                    ..(*read).clone()
                }))
            }
            result => result,
        }
    }

    /// A promoted speculative read can publish only under the scope that requested it.
    pub(super) fn publish_prefetched_snapshot(
        &mut self,
        request: &SnapshotRequest,
        result: CachedResult,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tab != request.tab || self.snapshot_request(self.tab).key != request.key {
            return false;
        }
        self.publish_snapshot(request, result, true, cx);
        true
    }
}

#[cfg(test)]
mod tests;
