//! Hit targets for GPU-drawn strategy-filter collapse headers.

use std::rc::Rc;

use moon_core::config::ChartLabelsCfg;

use super::ChartEngine;

/// The visible header rectangle and the configuration that gave its row index meaning.
pub(super) struct FilterHeaderHit {
    /// Window logical pixels, shared with the caption and cursor overlay.
    pub rect: [f32; 4],
    /// Row in the captured configuration.
    pub row: usize,
    /// A retained handle prevents a reordered profile from reusing a stale target.
    pub cfg: Rc<ChartLabelsCfg>,
}

impl FilterHeaderHit {
    /// Match only a nonempty drawn rectangle belonging to the configuration being edited.
    fn contains(&self, x: f32, y: f32, cfg: &ChartLabelsCfg) -> bool {
        let [left, top, width, height] = self.rect;
        width > 0.0
            && height > 0.0
            && x >= left
            && x <= left + width
            && y >= top
            && y <= top + height
            && self.cfg.as_ref() == cfg
    }
}

impl ChartEngine {
    /// Resolve a header press in window logical pixels against the current editable profile.
    pub(crate) fn filter_header_at(
        &self,
        pane: usize,
        x: f32,
        y: f32,
        cfg: &ChartLabelsCfg,
    ) -> Option<usize> {
        let data = self.data.borrow();
        let render = data.render.borrow();
        let pane = render.panes.get(pane).filter(|pane| pane.active)?;
        pane.filter_header_hits
            .iter()
            .find(|hit| hit.contains(x, y, cfg))
            .map(|hit| hit.row)
    }

    /// Publish the last drawn header rectangles for the panel's pointing-hand cursor zones.
    pub(crate) fn filter_header_rects(&self, pane: usize) -> Vec<(f32, f32, f32, f32)> {
        let data = self.data.borrow();
        let render = data.render.borrow();
        let Some(pane) = render.panes.get(pane).filter(|pane| pane.active) else {
            return Vec::new();
        };
        pane.filter_header_hits
            .iter()
            .filter(|hit| Rc::ptr_eq(&hit.cfg, &data.chart_labels) || hit.cfg == data.chart_labels)
            .map(|hit| {
                let [x, y, w, h] = hit.rect;
                (x, y, w, h)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
