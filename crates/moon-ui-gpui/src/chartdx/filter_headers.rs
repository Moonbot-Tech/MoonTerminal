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

/// Where one strategy-filter or arbitrage column was drawn, for the wheel that scrolls it.
///
/// The union of the column's lines, plus its collapse header when it has lines: a collapsed column
/// registers no band, so the wheel over its header keeps panning the chart.
pub(super) struct ColumnBand {
    /// Window logical pixels, `[left, top, width, height]`, like [`FilterHeaderHit::rect`].
    pub rect: [f32; 4],
    /// Label row the column belongs to.
    pub row: usize,
}

impl ColumnBand {
    /// Grow the band of `row` in `bands` to cover one drawn run, opening it on the first.
    pub(super) fn grow(bands: &mut Vec<ColumnBand>, row: usize, rect: [f32; 4]) {
        let [x, y, w, h] = rect;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let Some(band) = bands.iter_mut().find(|band| band.row == row) else {
            bands.push(ColumnBand { rect, row });
            return;
        };
        let [left, top, width, height] = band.rect;
        let (l, t) = (left.min(x), top.min(y));
        let (r, b) = ((left + width).max(x + w), (top + height).max(y + h));
        band.rect = [l, t, r - l, b - t];
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        let [left, top, width, height] = self.rect;
        x >= left && x <= left + width && y >= top && y <= top + height
    }

    /// The left strip of the plot where the wheel scrolls an expanded strategy-filter column.
    ///
    /// MoonBot scrolls its filter list anywhere in a strip this wide along the plot's left edge,
    /// not only over the glyphs, so a pointer that misses the ink still scrolls. Only a column
    /// that drew lines has a band, so a collapsed list opens no strip and the wheel keeps panning.
    /// The arbitrage column keeps its glyph band alone: its venues are click targets and it is
    /// not anchored to the left edge. `plot` is `[left, top, right, bottom]` in the bands' space.
    pub(super) fn filter_strip(
        bands: &[ColumnBand],
        cfg: &ChartLabelsCfg,
        plot: [f32; 4],
    ) -> Option<ColumnBand> {
        let [left, top, right, bottom] = plot;
        let (width, height) = (FILTER_WHEEL_STRIP_W.min(right - left), bottom - top);
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        // A list placed elsewhere on the plot (a right-aligned zone) keeps its glyph band alone:
        // the left strip would scroll something drawn nowhere near the pointer.
        let band = bands.iter().find(|band| {
            band.rect[0] <= left + width
                && cfg
                    .rows
                    .get(band.row)
                    .is_some_and(|row| row.draws_strategy_filters())
        })?;
        Some(ColumnBand {
            rect: [left, top, width, height],
            row: band.row,
        })
    }

    /// The row whose column takes the wheel at this point: a drawn band first, then the strip.
    pub(super) fn wheel_row_at(
        bands: &[ColumnBand],
        strip: Option<&ColumnBand>,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        bands
            .iter()
            .chain(strip)
            .find(|band| band.contains(x, y))
            .map(|band| band.row)
    }
}

/// Logical pixels of the plot's left strip in which the wheel scrolls the strategy-filter list.
pub(super) const FILTER_WHEEL_STRIP_W: f32 = 300.0;

impl ChartEngine {
    /// The label row whose scrollable column was drawn under this point, in window logical pixels.
    pub(crate) fn label_column_at(&self, pane: usize, x: f32, y: f32) -> Option<usize> {
        let data = self.data.borrow();
        let render = data.render.borrow();
        let pane = render.panes.get(pane).filter(|pane| pane.active)?;
        ColumnBand::wheel_row_at(&pane.column_bands, pane.filter_strip.as_ref(), x, y)
    }

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
