//! The perf stages: `baseline`, `mouse_storm`, and the `static_text_*` repeat of both.
//!
//! What these stages set up is the reason the verdict means anything. The chart is put into a
//! high-present mode WITHOUT a cursor first, so the storm is compared against an already-hot chart
//! rather than an idle one; a red result then reads as "the cursor added work", not "the market was
//! busy". The static-text repeat runs the same storm with a large retained text layer attached, so
//! a regression that only shows up under text load is not invisible.

use gpui::Context;

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;
use crate::firetest::plan::StageStep;
use crate::firetest::storm::{MouseStorm, start_mouse_storm};

/// Stages `baseline` and `static_text_warmup`: the measurement window has elapsed, so aim a storm
/// at the chart and move on. Both stages exist to hold the chart hot for a fixed time first, which
/// is why the row's `min_dwell` carries the duration and this only starts the storm.
pub(in crate::firetest) fn start_storm(
    runtime: &mut Runtime,
    _backend: &mut Backend,
    _cx: &mut Context<Backend>,
) -> StageStep {
    match runtime.start_mouse_storm() {
        Ok(storm) => {
            runtime.storm = Some(storm);
            StageStep::Next
        }
        Err(err) => StageStep::Fail(err),
    }
}

/// Stages `mouse_storm` and `static_text_storm`: hold until the storm thread finishes or its
/// configured duration is up, then release the cursor. The duration is a run knob rather than a
/// constant, which is why it is checked here instead of being the row's `min_dwell`.
pub(in crate::firetest) fn await_storm(
    runtime: &mut Runtime,
    _backend: &mut Backend,
    _cx: &mut Context<Backend>,
) -> StageStep {
    let done = runtime.storm.as_ref().is_some_and(MouseStorm::is_done);
    if done || runtime.phase_since.elapsed() >= runtime.config.storm {
        runtime.stop_storm();
        StageStep::Next
    } else {
        StageStep::Stay
    }
}

/// Stage `static_text_gap`: attach the retained text layer, failing the run if nothing took it —
/// a silently skipped overlay would leave the second storm measuring the first storm's conditions.
pub(in crate::firetest) fn attach_text_overlay(
    runtime: &mut Runtime,
    backend: &mut Backend,
    cx: &mut Context<Backend>,
) -> StageStep {
    if runtime.enable_text_overlay(backend, cx) == 0 {
        StageStep::Fail("chart opened but static text stress overlay did not attach".to_string())
    } else {
        StageStep::Next
    }
}

impl Runtime {
    /// Start a storm over the observed chart bounds. Fails when no probe has arrived.
    pub(in crate::firetest) fn start_mouse_storm(&mut self) -> Result<MouseStorm, String> {
        let Some(probe) = self.probe else {
            return Err("missing chart probe before mouse storm".to_string());
        };
        start_mouse_storm(probe, self.config.storm, self.config.mouse_hz)
    }

    /// Stop the running storm, if any.
    pub(in crate::firetest) fn stop_storm(&mut self) {
        if let Some(storm) = self.storm.take() {
            storm.stop();
        }
    }

    /// Toggle the forced high-present mode on every live chart. Idempotent: repeated calls with the
    /// same value do nothing, so a per-tick call from a stage does not spam the chart consumers.
    pub(in crate::firetest) fn set_present_pressure(
        &mut self,
        backend: &mut Backend,
        enabled: bool,
    ) {
        if self.present_pressure_enabled == enabled {
            return;
        }
        self.present_pressure_enabled = enabled;
        for chart in backend.live_chart_consumers() {
            chart.set_firetest_force_present(enabled);
        }
    }

    /// Attach the retained static-text stress layer to one live chart.
    ///
    /// Returns how many charts accepted it — `0` means nothing attached, which the caller treats as
    /// a failed stage rather than a silently skipped one.
    pub(in crate::firetest) fn enable_text_overlay(
        &mut self,
        backend: &mut Backend,
        cx: &mut Context<Backend>,
    ) -> usize {
        if self.text_overlay_enabled {
            return 0;
        }
        self.text_overlay_enabled = true;
        let count = self.config.text_labels;
        if count == 0 {
            firetest_info("[firetest] text overlay disabled");
            return 0;
        }
        let mut applied = 0usize;
        if let Some(chart) = backend.live_chart_consumers().into_iter().next() {
            if chart.set_firetest_text_labels(count) {
                applied += 1;
            }
        }
        firetest_info(&format!(
            "[firetest] text overlay labels={count} applied_to={applied}"
        ));
        if applied > 0 {
            cx.notify();
        }
        applied
    }
}
