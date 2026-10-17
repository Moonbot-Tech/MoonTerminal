//! Retained chart-text demand across client replacements.

use super::*;
use std::time::{Duration, Instant};

/// Repair a silently lost core subscription without returning to per-second re-sends.
const REPAIR_INTERVAL: Duration = Duration::from_secs(30);

/// Last requested overlay market, re-sent on client replacement and every 30 seconds for repair.
///
/// Lives across [`crate::feed::live::run`] retries in the feed spawn loop: `applied` is per client, the
/// wanted market is the coordinator's. Resetting both on each `run` would clear a still-wanted
/// overlay and the UI would not re-issue an unchanged request.
#[derive(Default)]
pub(in crate::feed) struct ChartTextWanted {
    pub(super) market: String,
    pub(super) need_filters: bool,
    pub(super) applied: bool,
    pub(super) last_sent: Option<Instant>,
}

impl ChartTextWanted {
    /// Forget the applied flag so a replacement client is told again.
    ///
    /// The wanted market is kept: it is what the coordinator last asked for, independent of which
    /// MoonClient is connected.
    pub(in crate::feed) fn begin_client(&mut self) {
        self.applied = false;
    }

    /// Record demand and check its repair deadline using the caller's monotonic clock.
    pub(super) fn wants(&mut self, market: String, need_filters: bool, now: Instant) -> bool {
        if self.market != market || self.need_filters != need_filters {
            self.applied = false;
        }
        self.market = market;
        self.need_filters = need_filters;
        self.due(now)
    }

    /// Pending requests retry immediately; successful requests wait for the repair heartbeat.
    pub(super) fn due(&self, now: Instant) -> bool {
        !self.applied || self.repair_wait(now).is_some_and(|wait| wait.is_zero())
    }

    /// Wake an idle feed for repair; failed sends keep the existing retry-on-wake behavior.
    pub(in crate::feed::live) fn repair_wait(&self, now: Instant) -> Option<Duration> {
        self.applied.then(|| {
            self.last_sent.map_or(Duration::ZERO, |last| {
                REPAIR_INTERVAL.saturating_sub(now.saturating_duration_since(last))
            })
        })
    }

    /// Send the retained demand and start the repair interval only after a successful send.
    pub(in crate::feed::live) fn send(&mut self, client: &MoonClient, server_id: u64) {
        let result = if self.need_filters && !self.market.is_empty() {
            client
                .chart_text()
                .set_visible_market(&self.market, true, false)
        } else {
            client.chart_text().clear_visible_market()
        };
        match result {
            Ok(()) => {
                self.applied = true;
                self.last_sent = Some(Instant::now());
            }
            Err(error) => {
                self.applied = false;
                log::debug!(
                    "core {} set chart text market={} filters={} failed: {error}",
                    crate::feed::core_label(server_id),
                    self.market,
                    self.need_filters
                );
            }
        }
    }
}
