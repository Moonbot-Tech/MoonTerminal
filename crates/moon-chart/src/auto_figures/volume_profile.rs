//! `VolumeProfile`: the point of control and the value-area edges of a UTC day's volume profile,
//! and the "naked" POCs of past days price has not traded at since.
//!
//! A STUB for now: the contract (`moon_core::config::auto_figures::VolumeProfileParams`) and this
//! registry row exist so the terminal and the presets can be built against the kind; the search
//! itself is filled by the detector stage (`docs-internal/AUTO_FIGURES.md` §13.2). Until then the
//! row finds nothing — never a wrong level.

use moon_core::config::auto_figures::{DetectorKind, DetectorParams};
use moon_core::market::candles::ChartCandle;

use super::{DetectCtx, Detector, Found};

/// The `VolumeProfile` registry row.
pub struct VolumeProfile;

impl Detector for VolumeProfile {
    fn kind(&self) -> DetectorKind {
        DetectorKind::VolumeProfile
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
