//! Chart consumers and application-wide Live/Pause transitions.

use crate::Backend;
use crate::chartdx::ChartDataHandle;
use moon_core::config::CoreGroup;

impl Backend {
    /// Apply one edit to the saved core groups, sanitize the result, and persist it.
    ///
    /// The dialogs use this centralized write path so their edits cannot store a shape the loader
    /// would have to repair or forget `config_dirty`. The edit works on a copy: the dirty flag is
    /// raised only when the sanitized result actually differs, so a rename to the same text or a
    /// refused reorder costs no disk write.
    ///
    /// The result is mirrored into an open Settings PREVIEW, exactly as `update_group_trade` does.
    /// Settings saves the preview it cloned when it opened and then replaces the live config with
    /// it, so an edit written only here would be rolled back by a Settings save that touched
    /// nothing related.
    ///
    /// Args:
    ///     edit: The mutation, reporting whether it changed anything worth sanitizing.
    ///
    /// Returns:
    ///     Whether the saved list actually changed. `false` covers a refused edit AND one the
    ///     sanitizer undid — at the group ceiling an append survives the closure and not the
    ///     sanitize, and a caller reporting success there would close its dialog over nothing.
    pub(crate) fn edit_core_groups(
        &mut self,
        edit: impl FnOnce(&mut Vec<CoreGroup>) -> bool,
    ) -> bool {
        let mut groups = self.config.core_groups.clone();
        if !edit(&mut groups) {
            return false;
        }
        moon_core::config::sanitize_core_groups(&mut groups);
        if groups == self.config.core_groups {
            return false;
        }
        if let Some(preview) = self.preview.as_mut() {
            preview.core_groups = groups.clone();
        }
        self.config.core_groups = groups;
        self.config_dirty = true;
        true
    }

    pub(crate) fn register_chart_consumer(&mut self, chart: ChartDataHandle) {
        self.chart_consumers.retain(ChartDataHandle::is_alive);
        if self
            .chart_consumers
            .iter()
            .any(|existing| existing == &chart)
        {
            return;
        }
        // Condition 2 of idle auto-return: leftover Pause must not greet the next live chart.
        // The 100 ms tick also restores when the population is empty, but a close-then-open
        // inside that window would already have a consumer again, so the empty check would miss.
        let live_before = self
            .chart_consumers
            .iter()
            .filter(|existing| !existing.is_historical())
            .count();
        let incoming_live = !chart.is_historical();
        self.chart_consumers.push(chart);
        if live_before == 0 && incoming_live {
            self.raise_follow();
        }
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn register_debug_main_chart(&mut self, group: String, chart: ChartDataHandle) {
        self.debug_main_chart_handles.insert(group, chart);
    }

    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    pub(crate) fn debug_main_chart_shift_hz(&self, group: &str) -> Option<f32> {
        self.debug_main_chart_handles
            .get(group)
            .filter(|chart| chart.is_alive())
            .and_then(ChartDataHandle::camera_shift_hz)
    }

    pub(crate) fn live_chart_consumers(&mut self) -> Vec<ChartDataHandle> {
        self.chart_consumers.retain(ChartDataHandle::is_alive);
        self.chart_consumers.clone()
    }

    /// Flip the application-wide Live/Pause flag from the toolbar or `ToggleLive`.
    ///
    /// Turning Pause on is a deliberate leave: the idle auto-return must not undo it. Turning
    /// Live on clears that mark so a later pan can auto-return again.
    pub(crate) fn toggle_follow(&mut self) {
        if self.follow {
            self.follow = false;
            self.follow_persistent = true;
        } else {
            self.raise_follow();
        }
    }

    /// Turn the application-wide Live back on and record the resume for the per-chart scope.
    ///
    /// Every backend path that restores Live goes through here so `live_resume_gen` misses none:
    /// a per-chart engine re-anchors its panes on a generation it has not seen yet (#688).
    pub(crate) fn raise_follow(&mut self) {
        if !self.follow {
            self.live_resume_gen = self.live_resume_gen.wrapping_add(1);
        }
        self.follow = true;
        self.follow_persistent = false;
    }

    /// The generation of the latest Live resume; see [`Self::raise_follow`].
    pub(crate) fn live_resume_gen(&self) -> u64 {
        self.live_resume_gen
    }

    /// Restore Live when every live chart is gone, or when a pan/zoom leave has sat idle for
    /// [`moon_chart::view::AUTO_RESUME_LIVE_MS`].
    ///
    /// A toolbar/hotkey Pause is left alone while any live chart remains. Historical trade
    /// windows do not count as charts and do not keep a stale Pause alive. The transition itself
    /// is the only dirtying: a tick that decides nothing notifies nothing.
    ///
    /// Args:
    ///     now_ms: Current Unix time in milliseconds, the same clock the views stamp against.
    ///
    /// Returns:
    ///     Whether Live was restored on this tick.
    pub(crate) fn tick_auto_live(&mut self, now_ms: f64) -> bool {
        self.chart_consumers.retain(ChartDataHandle::is_alive);
        let charts_open = self
            .chart_consumers
            .iter()
            .filter(|chart| !chart.is_historical())
            .count();
        if !moon_chart::view::should_return_to_live(
            self.last_live_chart_interaction_ms,
            now_ms,
            self.follow,
            self.follow_persistent,
            charts_open,
        ) {
            return false;
        }
        self.raise_follow();
        true
    }
}
