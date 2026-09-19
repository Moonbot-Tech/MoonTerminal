//! Route a strategy-filter header press through the existing caption-config save relay.

use gpui::Context;

use super::ChartPanel;

impl ChartPanel {
    /// Consume a header press before chart gestures; coordinates arrive in slot device pixels.
    pub(super) fn try_toggle_strategy_filters(
        &mut self,
        pos: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.input.pane_at(pos.0, pos.1) else {
            return false;
        };
        let Some((origin, scale, _)) = self.chart_origin_logical() else {
            return false;
        };
        let mut cfg = self.effective_labels(cx);
        let Some(row_ix) = self.chart.filter_header_at(
            pane,
            pos.0 / scale + origin.0,
            pos.1 / scale + origin.1,
            &cfg,
        ) else {
            return false;
        };
        let Some(row) = cfg
            .rows
            .get_mut(row_ix)
            .filter(|row| row.draws_strategy_filters())
        else {
            return false;
        };
        row.collapsed = !row.collapsed;
        // This relay applies the change now and lets the owning docked/detached host persist it.
        self.apply_labels_edit(cfg, cx);
        true
    }
}
