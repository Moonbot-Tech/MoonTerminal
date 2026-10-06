//! Retained chart text, market, and orderbook subscriptions.

use crate::Backend;
use moon_core::session::CoreId;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

impl Backend {
    /// One more live panel now draws strategy-filter captions on this core and market.
    pub(crate) fn retain_chart_text(&mut self, core: CoreId, market: &str) {
        if market.is_empty() {
            return;
        }
        let key = (core, market.to_string());
        *self.chart_text_refs.entry(key).or_insert(0) += 1;
        self.chart_text_last.insert(core, market.to_string());
        self.rebuild_chart_text();
    }

    /// One live panel stopped drawing strategy-filter captions on this core and market.
    pub(crate) fn release_chart_text(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_text_refs.get_mut(&key) {
            debug_assert!(*count > 0, "chart text refcount over-release");
            *count = count.saturating_sub(1);
            remove = *count == 0;
        } else {
            debug_assert!(false, "chart text refcount release without owner");
        }
        if remove {
            self.chart_text_refs.remove(&key);
        }
        self.rebuild_chart_text();
    }

    /// Pick one market per core from live refs and send or clear the ChartText relay.
    ///
    /// The protocol allows one market per client. A core whose last retained market still has a
    /// ref keeps it; otherwise any remaining ref wins; zero refs clears the relay.
    fn rebuild_chart_text(&mut self) {
        let mut cores: HashSet<CoreId> = self.chart_text_sent.keys().copied().collect();
        cores.extend(self.chart_text_refs.keys().map(|(core, _)| *core));
        cores.extend(self.chart_text_last.keys().copied());
        for core in cores {
            let next = self.chart_text_market_for(core);
            self.apply_chart_text(core, next.as_deref());
        }
    }

    fn chart_text_market_for(&self, core: CoreId) -> Option<String> {
        if let Some(last) = self.chart_text_last.get(&core)
            && self
                .chart_text_refs
                .get(&(core, last.clone()))
                .copied()
                .unwrap_or(0)
                > 0
        {
            return Some(last.clone());
        }
        self.chart_text_refs
            .iter()
            .find_map(|((c, market), count)| (*c == core && *count > 0).then(|| market.clone()))
    }

    fn apply_chart_text(&mut self, core: CoreId, market: Option<&str>) {
        let next = market.filter(|m| !m.is_empty()).map(str::to_string);
        if self.chart_text_sent.get(&core) == next.as_ref() {
            return;
        }
        match next {
            Some(name) => {
                if self
                    .session
                    .set_chart_text(core, name.clone(), true)
                    .is_ok()
                {
                    self.chart_text_sent.insert(core, name);
                }
            }
            None => {
                if self
                    .session
                    .set_chart_text(core, String::new(), false)
                    .is_ok()
                {
                    self.chart_text_sent.remove(&core);
                }
            }
        }
    }

    pub(crate) fn retain_chart_market(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        *self.chart_market_refs.entry(key).or_insert(0) += 1;
        self.rebuild_desired_markets();
    }

    pub(crate) fn release_chart_market(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_market_refs.get_mut(&key) {
            debug_assert!(*count > 0, "chart market refcount over-release");
            *count = count.saturating_sub(1);
            remove = *count == 0;
        } else {
            debug_assert!(false, "chart market refcount release without owner");
        }
        if remove {
            self.chart_market_refs.remove(&key);
        }
        self.rebuild_desired_markets();
    }

    pub(crate) fn retain_chart_orderbook(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        *self.chart_orderbook_refs.entry(key).or_insert(0) += 1;
        self.rebuild_orderbook_wanted();
    }

    pub(crate) fn release_chart_orderbook(&mut self, core: CoreId, market: &str) {
        let key = (core, market.to_string());
        let mut remove = false;
        if let Some(count) = self.chart_orderbook_refs.get_mut(&key) {
            *count = count.saturating_sub(1);
            remove = *count == 0;
        }
        if remove {
            self.chart_orderbook_refs.remove(&key);
        }
        self.rebuild_orderbook_wanted();
    }

    /// Rebuild `desired_orderbook` from markets with at least one enabled order-book consumer.
    ///
    /// A changed list marks the open-market request set dirty for resending.
    pub(crate) fn rebuild_orderbook_wanted(&mut self) {
        let mut want: Vec<(CoreId, String)> = self
            .chart_orderbook_refs
            .iter()
            .filter(|&((_core, _market), count)| *count > 0)
            .map(|((core, market), _count)| (*core, market.clone()))
            .collect();
        want.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if self.desired_orderbook != want {
            self.desired_orderbook = want;
            self.desired_open_dirty = true;
        }
    }

    pub(crate) fn rebuild_desired_markets(&mut self) {
        let mut desired: Vec<(CoreId, String)> = self
            .chart_market_refs
            .iter()
            .filter(|&((_core, _market), count)| *count > 0)
            .map(|((core, market), _count)| (*core, market.clone()))
            .collect();
        desired.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        if self.desired != desired {
            self.desired = desired;
            self.desired_open_dirty = true;
        }
    }

    pub(crate) fn sync_open_markets_if_due(&mut self) {
        let now = Instant::now();
        // The 1s fallback is intentional: provider-side linger/drop/failover is
        // wall-clock based. The hot path itself is the boolean dirty flag; we no
        // longer hash the whole desired market list every 100ms.
        let due = now.duration_since(self.last_open_sync) >= Duration::from_secs(1);
        if self.desired_open_dirty || due {
            self.desired_open_dirty = false;
            self.last_open_sync = now;
            self.session
                .set_open(&self.desired, &self.desired_orderbook);
            // A full feed respawn (SessionManager::reconnect) mints a new command channel. Markets
            // are re-queued here every second; ChartText is last-writer and would otherwise stay
            // in chart_text_sent without ever reaching the new thread.
            for (core, market) in self.chart_text_sent.clone() {
                let _ = self.session.set_chart_text(core, market, true);
            }
        }
    }
}
