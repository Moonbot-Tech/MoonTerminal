//! Durable closed-trade markers composed into the existing userdata layer.

use std::rc::Rc;

use moon_chart::layers::{MARKER_SHAPE_CROSS, MARKER_SHAPE_KNOT, MarkerInstance};
use moon_core::db::ChartTradeRecord;
use moon_core::session::CoreId;

use super::ChartDataState;

/// Build exact-core entry and exit markers from absolute durable records.
///
/// Args:
///     records: Durable records that may contain distracting other-core rows.
///     core: Exact pane core accepted by the marker layer.
///     epoch_ms: Absolute chart epoch subtracted from record timestamps.
///     pixel_scale: Device-pixel scale for marker dimensions.
///     entry_color: Theme-derived entry marker color.
///     exit_color: Theme-derived exit marker color.
///
/// Returns:
///     Exact-core entry/exit markers in record order.
fn build_trade_history_markers(
    records: &[ChartTradeRecord],
    core: CoreId,
    epoch_ms: f64,
    pixel_scale: f32,
    entry_color: [f32; 4],
    exit_color: [f32; 4],
) -> Vec<MarkerInstance> {
    let mut markers = Vec::with_capacity(records.len().saturating_mul(2));
    for record in records.iter().filter(|record| record.core_uid == core) {
        // Shape distinguishes entry from exit; size independently distinguishes long from short,
        // so the history remains readable without relying on colour alone.
        let size = if record.is_short { 5.0 } else { 7.0 } * pixel_scale;
        let thickness = 1.5 * pixel_scale;
        markers.push(MarkerInstance::at_price(
            (record.buy_date as f64 * 1_000.0 - epoch_ms) as f32,
            record.buy_price as f32,
            size,
            thickness,
            MARKER_SHAPE_CROSS,
            entry_color,
        ));
        markers.push(MarkerInstance::at_price(
            (record.close_date as f64 * 1_000.0 - epoch_ms) as f32,
            record.sell_price as f32,
            size,
            thickness,
            MARKER_SHAPE_KNOT,
            exit_color,
        ));
    }
    markers
}

impl ChartDataState {
    /// Replace the exact-target durable history and invalidate userdata only on a real change.
    ///
    /// Args:
    ///     records: Exact-target durable history owned by the chart panel.
    ///
    /// Returns:
    ///     Whether the record set changed.
    pub(super) fn set_trade_history(&mut self, records: Rc<Vec<ChartTradeRecord>>) -> bool {
        if Rc::ptr_eq(&self.trade_history, &records) || self.trade_history == records {
            return false;
        }
        self.trade_history = records;
        self.trade_history_revision = self.trade_history_revision.wrapping_add(1);
        let mut render = self.render.borrow_mut();
        for pane in &mut render.panes {
            pane.last_trade_history_sig = u64::MAX;
            pane.gpu_prepare_dirty = true;
        }
        render.needs_present = true;
        true
    }

    /// Return a non-sentinel signature for the current durable-history marker set.
    ///
    /// Returns:
    ///     Current revision, with the forced-dirty sentinel mapped to zero.
    pub(super) fn trade_history_sig(&self) -> u64 {
        if self.trade_history_revision == u64::MAX {
            0
        } else {
            self.trade_history_revision
        }
    }

    /// Append entry crosses and exit knots for records owned by this exact pane core.
    ///
    /// Args:
    ///     core: Exact pane core; records from other cores are ignored.
    ///     epoch_ms: Absolute chart epoch used to rebase timestamps.
    ///     markers: Existing order/figure/news marker union to extend.
    ///
    /// Returns:
    ///     Nothing; markers are appended in place unless the pane is orderbook-only.
    pub(super) fn append_trade_history_geometry(
        &self,
        core: CoreId,
        epoch_ms: f64,
        markers: &mut Vec<MarkerInstance>,
    ) {
        if self.orderbook_only {
            return;
        }
        let rgba = |rgb: [u8; 3]| {
            [
                rgb[0] as f32 / 255.0,
                rgb[1] as f32 / 255.0,
                rgb[2] as f32 / 255.0,
                0.95,
            ]
        };
        let entry_color = rgba(self.theme.label_positive);
        let exit_color = rgba(self.theme.label_negative);
        markers.extend(build_trade_history_markers(
            &self.trade_history,
            core,
            epoch_ms,
            self.last_ppp,
            entry_color,
            exit_color,
        ));
    }
}

#[cfg(test)]
mod tests;
