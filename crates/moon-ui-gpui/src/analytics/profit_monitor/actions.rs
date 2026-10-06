//! Profit Monitor actions.

use super::*;

impl ProfitMonitorView {
    /// Record which cores closed a trade since the previous snapshot and start their highlight.
    ///
    /// The signal is the report's own newest close date per core, so a row lights up for the same
    /// reason its `(last)` value changes: one authoritative timestamp, not a trade count that a
    /// retention sweep or a period edge could also move.
    ///
    /// Args:
    ///     cx: View context used to arm the repaint chain.
    pub(super) fn observe_arrivals(&mut self, cx: &mut Context<Self>) {
        let split_cores;
        let cores = match &self.data {
            ProfitLoadState::Ready { data, .. } => &data.cores,
            ProfitLoadState::Split(_) => {
                split_cores = rows::currency_arrivals(&self.currencies);
                &split_cores
            }
            _ => {
                self.rebaseline_arrivals();
                return;
            }
        };
        // An EMPTY previous snapshot is not a baseline to diff against. A table going from nothing
        // to forty rows is report replication catching up, not forty cores trading in one instant,
        // and treating it as arrivals lights the whole window at once. Nothing is lost: when the
        // table is empty, the row APPEARING is already the signal — the highlight exists to point
        // at a change inside a table that is already populated.
        let baseline = self.seen_trades.as_ref().filter(|seen| !seen.is_empty());
        let (seen, arrived) = arrivals(baseline, cores);
        self.seen_trades = Some(seen);
        if !self.prefs.flash || arrived.is_empty() {
            return;
        }
        self.flash.mark(arrived);
        crate::pulse::arm_with(
            self,
            cx,
            |this| this.flash.armed(),
            |this| this.flash.live(),
            Self::on_flash_tick,
        );
    }

    /// Forget the arrival baseline, so the next snapshot only records.
    ///
    /// Every caller is a case where the QUESTION changed rather than the answer: a new period, a
    /// new valuation or zone, a local-midnight rollover, or a read with no comparable rows at all.
    pub(super) fn rebaseline_arrivals(&mut self) {
        self.seen_trades = None;
        self.flash.clear();
    }

    /// Per-tick work of the shared pulse chain: drop finished stamps and dirty the cached table.
    ///
    /// The body is a cached SIBLING view. `cx.notify()` inside the pulse marks this view and its
    /// ancestors, which leaves that child clean and lets GPUI reuse the still-tinted subtree — so
    /// the tint has to be invalidated here or the fade never moves. Pruning first is deliberate:
    /// the tick that drops the last live stamp is the one that must erase the tint, and
    /// [`crate::pulse::Arrivals::live`] then ends the chain on the following tick.
    ///
    /// Args:
    ///     cx: View context used to invalidate the cached body.
    pub(super) fn on_flash_tick(&mut self, cx: &mut Context<Self>) {
        self.flash.prune();
        self.invalidate_content(cx);
    }

    /// Select and persist one period, then replace the database snapshot immediately.
    ///
    /// Args:
    ///     period: New monitor period.
    ///     cx: View context used to persist and reload.
    pub(super) fn set_period(&mut self, period: MonitorPeriod, cx: &mut Context<Self>) {
        if self.period == period {
            return;
        }
        self.period = period;
        self.busy_retries.reset();
        self.backend.update(cx, |backend, _| {
            backend.layout.profit_monitor_period = Some(period.id().to_string());
            backend.layout_dirty = true;
        });
        self.reload(false, cx);
        self.start_clock_refresh(cx);
    }

    /// Select and persist one grouping axis without touching the database.
    ///
    /// Args:
    ///     group: New grouping axis.
    ///     cx: View context used to persist and repaint.
    pub(super) fn set_group(&mut self, group: GroupMode, cx: &mut Context<Self>) {
        if self.group == group {
            return;
        }
        self.group = group;
        self.release_profit_width();
        self.invalidate_content(cx);
        self.backend.update(cx, |backend, _| {
            backend.layout.profit_monitor_group = Some(group.id().to_string());
            backend.layout_dirty = true;
        });
        cx.notify();
    }

    /// Toggle and persist the ordering selected through one table heading.
    ///
    /// Args:
    ///     column: Clicked visible column.
    ///     cx: View context used to persist and repaint.
    pub(super) fn toggle_sort(&mut self, column: MonitorSortColumn, cx: &mut Context<Self>) {
        let sort = next_sort(self.sort, column);
        self.sort = Some(sort);
        self.invalidate_content(cx);
        self.backend.update(cx, |backend, _| {
            backend.layout.profit_monitor_sort =
                Some((sort.column.id().to_string(), sort.descending));
            backend.layout_dirty = true;
        });
        cx.notify();
    }
}
