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
use crate::firetest::storm::{MouseStorm, start_mouse_storm};

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
