//! Load-shedding policy for report-driven Analytics refreshes.
//!
//! The report generation is the source of truth. This gate turns its potentially bursty commit
//! stream into one trailing refresh after a quiet period, while a maximum interval prevents a
//! continuous stream from starving the visible window forever.

use std::time::{Duration, Instant};

use super::Tab;

/// Quiet time that coalesces adjacent report commits into one analytical scan.
const QUIET_PERIOD: Duration = Duration::from_secs(1);

/// Maximum age of visible Analytics data while report commits continue without a quiet period.
const MAX_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Minimum interval between full core-list scans while Calendar remains visible.
const CORE_METADATA_INTERVAL: Duration = Duration::from_secs(60);

/// Maximum automatic retries for one report generation under persistent SQLite contention.
const MAX_BUSY_RETRIES: u8 = 3;

/// Bounded retry allowance for transient database contention.
#[derive(Default)]
pub(super) struct BusyRetryBudget {
    attempts: u8,
    active: bool,
}

impl BusyRetryBudget {
    /// Observe another committed generation without replenishing an active retry episode.
    ///
    /// A continuous writer stream may advance the generation while a blocked read is still
    /// running. Only an idle budget may start fresh here; otherwise every commit could turn the
    /// bounded Busy retry policy into an endless loop.
    pub(super) fn observe_generation(&mut self) {
        if !self.active {
            self.attempts = 0;
        }
    }

    /// Claim one automatic retry.
    ///
    /// Returns:
    ///     `true` while the active contention episode still has retry allowance.
    pub(super) fn claim(&mut self) -> bool {
        self.active = true;
        if self.attempts >= MAX_BUSY_RETRIES {
            return false;
        }
        self.attempts += 1;
        true
    }

    /// Close a retry episode after a read completes without SQLite contention.
    pub(super) fn resolve(&mut self) {
        self.attempts = 0;
        self.active = false;
    }

    /// Restore the allowance for an explicit user retry.
    pub(super) fn reset(&mut self) {
        self.resolve();
    }
}

/// Report-derived work required for the currently visible Analytics surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum VisibleRefresh {
    /// Refresh the Summary page only.
    Summary,
    /// Refresh the shared strategy-list base and then the visible tuner axis.
    StrategyBaseAndAxis,
    /// Refresh only a stale tuner axis when its shared base is already current.
    StrategyAxis,
    /// Refresh Calendar and its visible metadata.
    Calendar,
}

/// Choose the smallest refresh that can make the visible surface current.
///
/// Args:
///     tab: Currently visible Analytics tab.
///     data_dirty: Whether the shared Summary/strategy-list base is stale.
///
/// Returns:
///     The exact visible refresh target.
pub(super) fn visible_refresh(tab: Tab, data_dirty: bool) -> VisibleRefresh {
    match tab {
        Tab::Summary => VisibleRefresh::Summary,
        Tab::Strategies if data_dirty => VisibleRefresh::StrategyBaseAndAxis,
        Tab::Strategies => VisibleRefresh::StrategyAxis,
        Tab::Calendar => VisibleRefresh::Calendar,
    }
}

/// Decide whether a completed report read still owes a trailing refresh.
///
/// A successful query is safe to publish even when a newer commit landed while it was running:
/// SQLite gave it a consistent snapshot, so publishing it guarantees forward progress under a
/// continuous write stream. The generation mismatch keeps the surface dirty for the next
/// coalesced pass instead of discarding the only completed result.
///
/// Args:
///     started_generation: Report generation captured immediately before the read started.
///     current_generation: Latest committed report generation at completion.
///     read_failed: Whether any required part of the read failed.
///
/// Returns:
///     `true` when the surface still requires a catch-up read.
pub(super) fn report_result_is_stale(
    started_generation: u64,
    current_generation: u64,
    read_failed: bool,
) -> bool {
    read_failed || started_generation != current_generation
}

/// Decide whether the compact Strategies base may continue into its visible axis.
///
/// The axis is a child of the compound base refresh. Starting it after a partial base failure
/// wastes another scan and lets its success resolve the parent's active Busy retry episode.
///
/// Args:
///     data_failed: Whether the strategy-list aggregate failed.
///     undated_failed: Whether the undated-close metadata failed.
///     cores_failed: Whether the throttled core metadata failed.
///
/// Returns:
///     `true` only when every required base component completed successfully.
pub(super) fn strategy_base_allows_axis(
    data_failed: bool,
    undated_failed: bool,
    cores_failed: bool,
) -> bool {
    !data_failed && !undated_failed && !cores_failed
}

/// Return the remaining core-list throttle interval after the last successful scan.
///
/// Args:
///     last_success: Completion time of the last successful core-list query.
///     now: Current scheduling time.
///
/// Returns:
///     Zero when a query is due now, otherwise the exact remaining throttle duration.
pub(super) fn core_metadata_wait(last_success: Option<Instant>, now: Instant) -> Duration {
    let Some(last_success) = last_success else {
        return Duration::ZERO;
    };
    CORE_METADATA_INTERVAL.saturating_sub(now.duration_since(last_success))
}

/// Next action selected by [`RefreshGate::plan`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RefreshPlan {
    /// No timer or refresh is needed now.
    Idle,
    /// Start the visible surface's refresh immediately with its coalesced presentation policy.
    Now {
        /// Whether user-triggered work requires the blocking busy overlay.
        show_overlay: bool,
    },
    /// Arm the sole timer for the remaining debounce or maximum-wait interval.
    After(Duration),
}

/// Generation-aware debounce state owned by one open Analytics window.
pub(super) struct RefreshGate {
    last_seen_generation: u64,
    last_change_at: Instant,
    pending_since: Option<Instant>,
    pending: bool,
    pending_overlay: bool,
    timer_armed: bool,
}

impl RefreshGate {
    /// Create a clean gate aligned with the generation visible at window construction.
    ///
    /// Args:
    ///     generation: Current committed report generation.
    ///     now: Construction time and initial refresh baseline.
    ///
    /// Returns:
    ///     A gate with no pending automatic refresh.
    pub(super) fn new(generation: u64, now: Instant) -> Self {
        Self {
            last_seen_generation: generation,
            last_change_at: now,
            pending_since: None,
            pending: false,
            pending_overlay: false,
            timer_armed: false,
        }
    }

    /// Record a newly observed committed generation.
    ///
    /// Args:
    ///     generation: Latest value published by the report writer.
    ///     now: Observation time used by the quiet-period debounce.
    ///
    /// Returns:
    ///     `true` only when the generation advanced since the previous observation or refresh.
    pub(super) fn observe_generation(&mut self, generation: u64, now: Instant) -> bool {
        if generation == self.last_seen_generation {
            return false;
        }
        self.last_seen_generation = generation;
        if !self.pending {
            self.pending_since = Some(now);
            self.pending_overlay = false;
        }
        self.last_change_at = now;
        self.pending = true;
        true
    }

    /// Acknowledge all commits visible when a manual or automatic refresh starts.
    ///
    /// An already-running timer remains armed; when it wakes, it re-evaluates `pending` and exits
    /// without duplicating a manual refresh.
    ///
    /// Args:
    ///     generation: Latest committed generation covered by the new refresh.
    ///     now: Refresh start time used to reset the quiet-period baseline.
    pub(super) fn refresh_started(&mut self, generation: u64, now: Instant) {
        self.last_seen_generation = generation;
        self.last_change_at = now;
        self.pending_since = None;
        self.pending = false;
        self.pending_overlay = false;
    }

    /// Queue refresh work without requiring another writer generation.
    ///
    /// This serves both transient database retries and tab-entry catch-up deferred behind an
    /// active scan. Repeated requests retain the original maximum-wait anchor.
    ///
    /// Args:
    ///     now: Failure completion time used as the retry debounce baseline.
    ///     show_overlay: Whether any coalesced user action requires blocking feedback.
    pub(super) fn request_refresh(&mut self, now: Instant, show_overlay: bool) {
        if !self.pending {
            self.pending_since = Some(now);
        }
        self.last_change_at = now;
        self.pending = true;
        self.pending_overlay |= show_overlay;
    }

    /// Choose whether to refresh now, arm one timer, or wait for active database work.
    ///
    /// Args:
    ///     now: Current scheduling time.
    ///     db_active: Whether any Analytics database operation is still running.
    ///
    /// Returns:
    ///     The single action the view should take.
    pub(super) fn plan(&mut self, now: Instant, db_active: bool) -> RefreshPlan {
        if !self.pending || db_active || self.timer_armed {
            return RefreshPlan::Idle;
        }

        let quiet_due = self.last_change_at + QUIET_PERIOD;
        let maximum_due = self.pending_since.unwrap_or(self.last_change_at) + MAX_REFRESH_INTERVAL;
        let due = quiet_due.min(maximum_due);
        if now >= due {
            RefreshPlan::Now {
                show_overlay: self.pending_overlay,
            }
        } else {
            self.timer_armed = true;
            RefreshPlan::After(due.duration_since(now))
        }
    }

    /// Release the timer slot before re-evaluating pending work.
    pub(super) fn timer_fired(&mut self) {
        self.timer_armed = false;
    }
}

#[cfg(test)]
mod tests;
