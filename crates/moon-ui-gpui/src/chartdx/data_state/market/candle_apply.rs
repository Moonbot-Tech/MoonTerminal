//! Apply candle and price-line reads to retained GPU buffers.

use super::*;

/// How a candle read landed in a pane's retained rows.
pub(crate) enum CandleApply {
    /// The whole list was replaced.
    Full,
    /// The rows from this index on were replaced.
    Patch(usize),
    /// A tail patch did not fit the retained rows; nothing changed and a full read is due.
    Rejected,
}

/// Applies one candle read to the retained GPU rows and volume samples.
///
/// `patch_from == None` replaces both; `Some(from)` replaces them from `from` on, and is rejected
/// when the retained rows are shorter, were converted against another epoch, or have fallen out of
/// step with the samples.
#[allow(clippy::too_many_arguments)] // two retained buffers, their epoch and one read's parts
pub(crate) fn apply_candle_read(
    rows: &mut Vec<CandleGpu>,
    samples: &mut Vec<moon_chart::VolumeSample>,
    rows_epoch: &mut f64,
    patch_from: Option<usize>,
    candles: &[moon_core::market::ChartCandle],
    tf: &[f32],
    epoch_ms: f64,
    series_tf_ms: f64,
) -> CandleApply {
    match patch_from {
        None => {
            fill_candle_upload(candles, tf, epoch_ms, rows);
            moon_chart::collect_samples(candles, tf, series_tf_ms, samples);
            *rows_epoch = epoch_ms;
            CandleApply::Full
        }
        Some(from) => {
            if from > rows.len() || *rows_epoch != epoch_ms || samples.len() != rows.len() {
                return CandleApply::Rejected;
            }
            rows.truncate(from);
            samples.truncate(from);
            extend_candle_upload(candles, tf, epoch_ms, rows);
            moon_chart::extend_samples(candles, tf, series_tf_ms, samples);
            CandleApply::Patch(from)
        }
    }
}

/// Applies one price line's read to its mirror and says what the GPU must receive.
///
/// Returns the update to ship and, for an append, the mirror index the new points start at. A
/// disabled line empties its mirror; a replacement, or points converted against another epoch,
/// rebuild it; an append extends it, and a mirror grown past twice the capacity drops its oldest
/// points and ships whole, since every slot then moves.
pub(crate) fn apply_price_line(
    mirror: &mut Vec<PriceLinePoint>,
    update: PriceLineUpdate,
    points: &[moon_core::feed::PricePoint],
    enabled: bool,
    cap: usize,
    epoch_ms: f64,
    mirror_epoch: f64,
) -> (PriceLineUpdate, usize) {
    if !enabled {
        // Readers drain whether or not the toggle is on; an already empty line has nothing to say.
        if mirror.is_empty() {
            return (PriceLineUpdate::None, 0);
        }
        mirror.clear();
        return (PriceLineUpdate::Replace, 0);
    }
    match update {
        PriceLineUpdate::None => (PriceLineUpdate::None, 0),
        PriceLineUpdate::Replace => {
            fill_price_upload(points, epoch_ms, mirror);
            (PriceLineUpdate::Replace, 0)
        }
        // The batch holds only new rows, not the window: leave the mirror for the full re-read the
        // caller forces.
        PriceLineUpdate::Append if mirror_epoch != epoch_ms => (PriceLineUpdate::None, 0),
        PriceLineUpdate::Append => {
            let from = mirror.len();
            extend_price_upload(points, epoch_ms, mirror);
            if mirror.len() > 2 * cap {
                let excess = mirror.len() - cap;
                mirror.drain(..excess);
                (PriceLineUpdate::Replace, 0)
            } else {
                (PriceLineUpdate::Append, from)
            }
        }
    }
}
