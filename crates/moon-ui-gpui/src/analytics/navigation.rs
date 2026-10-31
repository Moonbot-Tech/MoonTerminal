//! Active-tab navigation retires obsolete ordinary reads before scheduling the visible page.

use gpui::{Context, Window};

use super::bg::ReadLane;
use super::refresh::RefreshUrgency;
use super::{AnalyticsView, ProfitLoadState, Tab};

impl AnalyticsView {
    /// Activate a page without making it wait for a now-hidden full-period base scan.
    ///
    /// Cancellation still drains through the shared database interlock before the replacement
    /// starts. Repeated tab clicks therefore change the desired destination instead of launching
    /// parallel scans. Tuner searches, drafts and writes keep their existing ownership.
    ///
    /// Args:
    ///     tab: Page selected by the user.
    ///     window: Window owning the shared period pickers.
    ///     cx: Analytics context used to persist selection and schedule visible work.
    pub(super) fn activate_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.cancel_unrelated_prefetch(tab);
        self.retire_hidden_reads();
        self.tab = tab;
        self.backend.update(cx, |backend, _| {
            backend.ui_session.analytics.tab = tab;
        });
        self.sync_period_pickers(window, cx);
        let request = self.snapshot_request(tab);
        // Observe the captured publication before a fresh hit clears only this page's dirty flag.
        // Its hidden peers and tuner axes must still owe that same publication.
        self.observe_report_publication(request.report_generation);
        let request_owned = self.restore_or_join_snapshot(&request, cx);
        let needs_refresh = match tab {
            Tab::Summary => {
                if self.active_period() != self.data_period {
                    // An old period cannot remain visible under the newly restored label.
                    self.data = ProfitLoadState::default();
                    self.summary_derived = None;
                    self.data_as_of = None;
                    self.summary_catching_up = false;
                    self.data_dirty = true;
                }
                self.data_dirty
            }
            Tab::Strategies => {
                if self.active_period() != self.strategy_data_period {
                    self.strategy_data = ProfitLoadState::default();
                    self.strategy_dirty = true;
                }
                self.strategy_dirty
            }
            Tab::Calendar => self.cal_days.data().is_none() || self.cal_dirty,
        };
        if needs_refresh && !request_owned {
            // The gate dispatches whichever page is visible when cancellation acknowledges.
            self.request_report_refresh(RefreshUrgency::User, true, cx);
        } else {
            // A rapid return to a current page must not inherit a queued read for the page left
            // between clicks. Its own dirty flags retain any work that page still owes.
            self.acknowledge_navigation_refresh(request.report_generation, cx);
            if tab == Tab::Strategies && !self.strategy_dirty {
                self.request_axis_if_stale(self.strat_mode, cx);
            }
        }
        self.schedule_prefetch(cx);
        cx.notify();
    }

    /// Cancel unfinished ordinary reads for the page being left, retaining edits and explicit jobs.
    fn retire_hidden_reads(&mut self) {
        if self.tab == Tab::Strategies {
            self.retire_hidden_tuner_reads();
        }
        match self.tab {
            Tab::Summary if self.latest_reads.contains(ReadLane::Summary) => {
                self.latest_reads.cancel(&[ReadLane::Summary]);
                self.summary_seq = self.summary_seq.wrapping_add(1);
                self.data_dirty = true;
            }
            Tab::Strategies if self.latest_reads.contains(ReadLane::StrategyBase) => {
                self.latest_reads.cancel(&[ReadLane::StrategyBase]);
                self.strategy_seq = self.strategy_seq.wrapping_add(1);
                self.strategy_dirty = true;
            }
            Tab::Calendar if self.latest_reads.contains(ReadLane::Calendar) => {
                self.latest_reads.cancel(&[ReadLane::Calendar]);
                self.cal_seq = self.cal_seq.wrapping_add(1);
                self.cal_dirty = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
