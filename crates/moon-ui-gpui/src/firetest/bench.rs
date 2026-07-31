//! How much app there is to measure.
//!
//! A FireTest run on one core with one chart and a run on fifty cores with five charts are not the
//! same test, and for a long time the difference showed up as the run being flaky: the same code
//! passed on a quiet bench and failed on a busy one, because the ceilings were absolute. They are
//! not — nearly every counter scales with something the developer's saved layout decides.
//!
//! So the shape of the bench is measured, recorded in the log beside the numbers, and every idle
//! ceiling is stated PER UNIT of it. What that costs in honesty: the scaling law for each counter
//! is taken from what the counter is documented to do in `diag.rs`, and marked below where it is an
//! assumption rather than a documented fact. A law that turns out wrong shows up as a run that
//! fails on one bench and passes on another — the same symptom this replaces, and the reason the
//! shape is logged on every run rather than only on failure.

use gpui::{App, Context};

use crate::Backend;

/// The units a run's counters scale with, sampled once while the app sits idle.
#[derive(Clone, Copy, Debug)]
pub(super) struct BenchShape {
    /// Connected core sessions. Feed volume, and everything driven by it, scales with this.
    pub(super) cores: usize,
    /// Live chart consumers. Each one renders, presents and observes orders on its own.
    pub(super) charts: usize,
    /// Open OS windows. Each Shell window carries its own header clock and panel set.
    pub(super) windows: usize,
}

impl BenchShape {
    /// Read the current shape off the live app.
    pub(super) fn capture(backend: &mut Backend, cx: &App) -> Self {
        Self {
            cores: backend.session.sessions().len(),
            charts: backend.live_chart_consumers().len(),
            windows: cx.windows().len(),
        }
    }

    /// Divide a measured rate by the number of cores, never by zero.
    ///
    /// Documented law (`diag.rs`): the feed drain and everything it wakes scale with connected
    /// cores.
    pub(super) fn per_core(&self, rate: f64) -> f64 {
        rate / self.cores.max(1) as f64
    }

    /// Divide a measured rate by the number of live charts, never by zero.
    ///
    /// Documented law (`diag.rs`): `chart_order_sync` is "multiplied by the number of open charts
    /// observing each backend notification", and each chart panel renders and presents for itself.
    pub(super) fn per_chart(&self, rate: f64) -> f64 {
        rate / self.charts.max(1) as f64
    }

    /// Divide a measured rate by the number of open windows, never by zero.
    ///
    /// Documented law (`diag.rs`): the header clock is "one roughly 1 Hz timer per Shell window,
    /// so its rate approximates the number of open Shell windows". ASSUMPTION beyond that: a panel
    /// that exists in every window (Shell, Orders, News, Assets) repaints per window.
    pub(super) fn per_window(&self, rate: f64) -> f64 {
        rate / self.windows.max(1) as f64
    }
}

/// Capture the bench shape. Called from the idle stage, whose whole point is that nothing is
/// changing while it runs, so one reading describes the entire measured window.
pub(super) fn capture(backend: &mut Backend, cx: &mut Context<Backend>) -> BenchShape {
    BenchShape::capture(backend, cx)
}
