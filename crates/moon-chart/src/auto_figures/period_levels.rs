//! `PeriodLevel`: the opening and closing prices of UTC calendar periods — the previous day's
//! close, the day's open, the week's open, the previous week's close.
//!
//! A STUB for now: the contract (`moon_core::config::auto_figures::PeriodLevelParams`) and this
//! registry row exist so the terminal and the presets can be built against the kind; the search
//! itself is filled by the detector stage (`docs-internal/AUTO_FIGURES.md` §13.2), cutting days
//! and ISO weeks the way `extremes` does. Until then the row finds nothing — never a wrong level.

use moon_core::config::auto_figures::{DetectorKind, DetectorParams};
use moon_core::market::candles::ChartCandle;

use super::{DetectCtx, Detector, Found};

/// The `PeriodLevel` registry row.
pub struct PeriodLevel;

impl Detector for PeriodLevel {
    fn kind(&self) -> DetectorKind {
        DetectorKind::PeriodLevel
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
