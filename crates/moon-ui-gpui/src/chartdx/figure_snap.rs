//! Drawing-magnet access to the same bounded market rows uploaded by the renderer.

use moon_chart::drawing_snap::nearest_point;
use moon_chart::view::Rect;
use moon_core::figures::{FigNode, Proj};

use super::ChartEngine;
use super::types::{CandleGpu, CandleStyleGpu};

/// Candle rows retained only because the GPU consumes their upload vector.
/// Tick candidates borrow the native resident ring instead of keeping a second tape.
#[derive(Default)]
pub(super) struct FigureSnapData {
    candles: Vec<CandleGpu>,
}

impl FigureSnapData {
    /// Replace candles on series revision, preserving the platform's actual upload capacity.
    pub(super) fn set_candles(&mut self, rows: &[CandleGpu]) {
        // DX11's CandleLayer keeps only the newest 4096 instances; native backends keep all.
        let start = if cfg!(windows) {
            rows.len().saturating_sub(4096)
        } else {
            0
        };
        self.candles.clear();
        self.candles.extend_from_slice(&rows[start..]);
    }

    /// Retire candle candidates together with a disabled or cleared candle layer.
    pub(super) fn clear_candles(&mut self) {
        self.candles.clear();
    }
}

/// Candle OHLC is plotted at its bucket centre, including coarse history's own timeframe.
/// A hidden trade-only candle contributes nothing; hidden wicks contribute neither high nor low.
fn candle_nodes(
    row: &CandleGpu,
    epoch: f64,
    style: CandleStyleGpu,
) -> impl Iterator<Item = FigNode> {
    let in_zone = row.t_open_rel >= style.zone_start_rel;
    let wicks = !in_zone || style.wicks_in_zone >= 0.5;
    let outline = (style.mode >= 0.5 && style.mode < 1.5) || (style.mode >= 1.5 && in_zone);
    let visible =
        row.t_open_rel < style.hide_start_rel && (outline || style.fill_alpha > 0.0 || wicks);
    let tf = if row.tf_rel > 0.0 {
        row.tf_rel
    } else {
        style.tf_rel_ms
    };
    let time = epoch + f64::from(row.t_open_rel) + f64::from(tf) * 0.5;
    [
        Some(row.open),
        Some(row.close),
        wicks.then_some(row.high),
        wicks.then_some(row.low),
    ]
    .into_iter()
    .flatten()
    .filter(move |_| visible)
    .map(move |price| FigNode::new(time, f64::from(price)))
}

impl ChartEngine {
    /// Query only this pane's current uploaded data, using the figure layer's own projection.
    /// Identity and config checks reject the interval before a switched pane has prepared its data.
    pub(crate) fn nearest_figure_snap(
        &self,
        pane_index: usize,
        pointer: (f32, f32),
        plot: Rect,
        projection: &dyn Proj,
        tolerance: f32,
    ) -> Option<FigNode> {
        let container = self.container.borrow();
        let pane = container.pane(pane_index)?;
        let state = self.state.borrow();
        let rendered = state.panes.get(pane_index)?;
        let data = self.data.borrow();
        if rendered.core != Some(pane.core)
            || rendered.market != pane.market
            || rendered.orderbook_only
            || !rendered.resident_left_rel.is_finite()
            || rendered.applied_candle_cfg != data.candle_view.history_inputs()
        {
            return None;
        }
        // A provider replacement can precede its next prepared frame. Frozen replay has no
        // live-generation dependency, but is invalidated above by its resident-range reset.
        if data.trade_replay.is_none() {
            let revisions = data
                .market_source
                .as_ref()?
                .market_revisions(pane.core, &pane.market)?;
            if revisions.generation != rendered.source_generation {
                return None;
            }
        }
        let epoch = pane.view.epoch_ms;
        let left = projection.time_at_x(pointer.0 - tolerance) - epoch;
        let right = projection.time_at_x(pointer.0 + tolerance) - epoch;
        if !left.is_finite() || !right.is_finite() || left > right {
            return None;
        }
        // Borrow the native ring, including pending uploads and eviction. Time filtering happens
        // before projection; no history read, history clone or candidate vector on pointer motion.
        let ticks = rendered
            .layers
            .tick_samples()
            .filter(|row| {
                row.side < 2 && f64::from(row.time_rel) >= left && f64::from(row.time_rel) <= right
            })
            .map(|row| FigNode::new(epoch + f64::from(row.time_rel), f64::from(row.price)));
        let candles = rendered
            .figure_snap
            .candles
            .iter()
            .flat_map(|row| candle_nodes(row, epoch, rendered.candle_style))
            .filter(|node| node.time_ms >= epoch + left && node.time_ms <= epoch + right);
        nearest_point(ticks.chain(candles), pointer, plot, projection, tolerance)
    }
}

#[cfg(test)]
mod tests;
