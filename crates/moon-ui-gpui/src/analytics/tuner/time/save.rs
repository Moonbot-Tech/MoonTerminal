//! The "By time" axis' write actions: Save and Make-a-copy over the schedule fields.
//! Both feed the SHARED confirmation dialog (`open_change_dialog` / `open_copy_with`);
//! only the change-set assembly — which fields, from `TimeTunerState` — is the axis' own.

use gpui::{Context, Window};

use super::super::super::AnalyticsView;

impl AnalyticsView {
    /// "To strategy" for the "By time" axis: writes v1 into the `WorkingWeekTime`
    /// (day span) and `WorkingTime` (time window) fields of the selected strategy through
    /// the SAME confirmation dialog (`open_change_dialog`) as the thresholds. Only
    /// NON-EMPTY and changed fields are written (an empty field is left alone, the
    /// schedule is never wiped). The MoonBot field formats are unconfirmed — the strings
    /// are visible in the confirmation dialog.
    pub(in crate::analytics::tuner) fn time_open_save_dialog(&mut self, cx: &mut Context<Self>) {
        let targets = self.selected_targets();
        if targets.is_empty() {
            return;
        }
        // Single: same fields that light "Save" amber (`is_dirty`) so the button state and
        // the write permission agree. Multi: the forced set pushes the schedule to every
        // target regardless of the anchor's current values, and disables IgnoreTime/
        // IgnoreFilters so it actually applies.
        let bulk = targets.len() > 1;
        let changes = if bulk {
            self.time_tuner.changes_forced()
        } else {
            self.time_tuner.changes()
        };
        if changes.is_empty() {
            log::info!("analytics: 'Save' (time) — nothing to write (fields empty or = current)");
            return;
        }
        self.open_change_dialog(targets, changes, None, Vec::new(), Vec::new(), false, cx);
    }

    /// "Make a copy" for the "By time" axis: the same changes as Save (schedule +
    /// IgnoreTime/IgnoreFilters), but the target is a NEW strategy (a copy of the selected
    /// one). Shares the filter path through `open_copy_with`; empty changes are fine (a
    /// plain duplicate).
    pub(in crate::analytics::tuner) fn time_open_copy_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Copy is single-target only (button hidden in multi-select) — take the anchor.
        let Some(target) = self.selected_targets().into_iter().next() else {
            return;
        };
        let mut changes = self.time_tuner.changes();
        changes.push(self.analyzer_comment());
        self.open_copy_with(target, changes, Vec::new(), window, cx);
    }
}
