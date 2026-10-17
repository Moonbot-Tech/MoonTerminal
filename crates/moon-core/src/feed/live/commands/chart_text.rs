//! Retained chart-text demand across client replacements.

use super::*;

/// Last requested strategy-filter overlay market, re-sent when MoonClient is replaced.
///
/// Lives across [`crate::feed::live::run`] retries in the feed spawn loop: `applied` is per client, the
/// wanted market is the coordinator's. Resetting both on each `run` would clear a still-wanted
/// overlay and the UI would not re-issue an unchanged request.
#[derive(Default)]
pub(in crate::feed) struct ChartTextWanted {
    pub(super) market: String,
    pub(super) need_filters: bool,
    pub(super) applied: bool,
}

impl ChartTextWanted {
    /// Forget the applied flag so a replacement client is told again.
    ///
    /// The wanted market is kept: it is what the coordinator last asked for, independent of which
    /// MoonClient is connected.
    pub(in crate::feed) fn begin_client(&mut self) {
        self.applied = false;
    }

    /// Record a coordinator request; `false` when this client already holds exactly that one,
    /// so the repeat is not sent again. A failed send leaves `applied` false, so it retries.
    pub(super) fn wants(&mut self, market: String, need_filters: bool) -> bool {
        if self.applied && self.market == market && self.need_filters == need_filters {
            return false;
        }
        self.market = market;
        self.need_filters = need_filters;
        true
    }

    pub(super) fn send(&mut self, client: &MoonClient, server_id: u64) {
        let result = if self.need_filters && !self.market.is_empty() {
            client
                .chart_text()
                .set_visible_market(&self.market, true, false)
        } else {
            client.chart_text().clear_visible_market()
        };
        match result {
            Ok(()) => self.applied = true,
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
