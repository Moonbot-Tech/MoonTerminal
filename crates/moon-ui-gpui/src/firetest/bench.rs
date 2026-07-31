//! How much app there is to measure.
//!
//! A FireTest run on one core with one chart and a run on fifty cores with five charts are not the
//! same test, and for a long time the difference showed up as the run being flaky: the same code
//! passed on a quiet bench and failed on a busy one, because the ceilings were absolute.
//!
//! So the shape of the bench is measured, recorded in the log beside the numbers, and a LEVEL is
//! compared per unit of it. Two warnings, both learned the hard way:
//!
//! * A DELTA must not be divided. Baseline is `c·b` and the storm is `c·b + s`, so the subtraction
//!   already removes the unit; dividing again just makes a bigger bench more permissive.
//! * A law must come from what the counter is documented to do, not from it looking like it
//!   scales. `backend_notify` looks core-driven and is in fact globally coalesced, so dividing it
//!   by cores produced a check that could never fail.

use gpui::{App, Context};

use crate::Backend;

/// The units a run's LEVELS scale with, sampled once while the app sits idle.
#[derive(Clone, Copy, Debug)]
pub(super) struct BenchShape {
    /// Connected core sessions.
    pub(super) cores: usize,
    /// Live chart consumers. Each one renders and presents on its own.
    pub(super) charts: usize,
    /// Shell (group) windows — the ones that carry a header clock and a panel set. Deliberately
    /// NOT every GPUI window: tool and detached windows do not repaint the per-window panels, so
    /// counting them would loosen every per-window ceiling for free.
    pub(super) windows: usize,
}

impl BenchShape {
    /// Read the current shape off the live app.
    pub(super) fn capture(backend: &mut Backend, _cx: &App) -> Self {
        Self {
            cores: backend.session.sessions().len(),
            charts: backend.live_chart_consumers().len(),
            windows: backend.group_windows.len(),
        }
    }

    /// Divide a measured level by the number of live charts, never by zero.
    ///
    /// Documented law (`diag.rs`): `chart_order_sync` is "multiplied by the number of open charts
    /// observing each backend notification", and each chart panel renders and presents for itself.
    pub(super) fn per_chart(&self, rate: f64) -> f64 {
        rate / self.charts.max(1) as f64
    }

    /// Divide a measured level by the number of Shell windows, never by zero.
    ///
    /// Documented law (`diag.rs`): the header clock is "one roughly 1 Hz timer per Shell window,
    /// so its rate approximates the number of open Shell windows". ASSUMPTION beyond that: a panel
    /// that exists in every Shell window (Orders, News, Assets) repaints once per window.
    pub(super) fn per_window(&self, rate: f64) -> f64 {
        rate / self.windows.max(1) as f64
    }
}

/// Capture the bench shape. Called from the idle stage, whose whole point is that nothing is
/// changing while it runs, so one reading describes the entire measured window.
pub(super) fn capture(backend: &mut Backend, cx: &mut Context<Backend>) -> BenchShape {
    BenchShape::capture(backend, cx)
}
