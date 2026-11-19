//! `RoundLevels`: round prices near the last close, on a grid derived from the price's order of
//! magnitude.
//!
//! A STUB for now: the contract (`moon_core::config::auto_figures::RoundLevelsParams`) and this
//! registry row exist so the terminal and the presets can be built against the kind; the search
//! itself is filled by the detector stage (`docs-internal/AUTO_FIGURES.md` §13.2). Until then the
//! row finds nothing — never a wrong level.

use moon_core::config::auto_figures::{DetectorKind, DetectorParams};
use moon_core::market::candles::ChartCandle;

use super::{DetectCtx, Detector, Found};

/// The `RoundLevels` registry row.
pub struct RoundLevels;

impl Detector for RoundLevels {
    fn kind(&self) -> DetectorKind {
        DetectorKind::RoundLevels
    }

    /// Filled by the detector stage; finds nothing until then.
    fn detect(
        &self,
        _bars: &[ChartCandle],
        _params: &DetectorParams,
        _ctx: &DetectCtx,
    ) -> Vec<Found> {
        Vec::new()
    }
}
