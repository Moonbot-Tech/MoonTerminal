//! Stage `arrival_flash`: what the new-chart border flash costs, measured instead of assumed.
//!
//! The flash used to repaint the owning stack at vblank rate and was moved into the chart's own
//! pass for exactly that reason. What it costs THERE was never measured: a present is a WINDOW
//! present, so ten flashes a second re-run `prepare_gpu`, `prepare_text` and `draw` for every
//! canvas in the window, and the storm phases cannot see it because they force presents anyway.
//!
//! Waiting for a real detect would not answer it either — a run that happened to see no arrival
//! reads exactly like a free flash. So this stage LIGHTS the flash itself, on every live chart, and
//! keeps it lit for a fixed window that the verdict compares against `idle_floor`: same charts,
//! same feed, same cold present mode, one difference.
//!
//! `MOON_ARRIVAL_FLASH=0` turns the flash off process-wide. The stage still runs and still asks for
//! it — which is what makes the pair a controlled A/B: with the switch off the pulse counter must
//! read zero, and the phase becomes a second idle floor.

use std::time::{Duration, Instant};

use gpui::Context;
use moon_ui::MoonPalette;

use crate::Backend;

use crate::firetest::Runtime;
use crate::firetest::logging::firetest_info;
use crate::firetest::plan::StageStep;
use crate::firetest::storm::MouseStorm;

/// How often the flash is re-lit while the window runs.
///
/// Shorter than the 2600 ms the own-pass expires it after, so the border never goes dark mid-window
/// and the phase measures a continuously flashing chart — the worst case a stream of detects
/// produces, not the average of one arrival and four quiet seconds.
const REARM: Duration = Duration::from_millis(2400);

/// The in-flight flash window: when it started, when the flash was last re-lit, and how many times.
pub(in crate::firetest) struct FlashRun {
    started: Instant,
    last_lit: Instant,
    ignitions: usize,
    /// The most charts seen flashing at once during the window.
    ///
    /// Not the count at entry: live detects open charts WHILE the window runs — measured, 4 of 10
    /// runs grew mid-phase and one went 1 → 4. Every counter here is a process-wide sum over
    /// canvases, so dividing the phase by the entry count reports a per-chart rate up to four times
    /// too high. The peak is the conservative divisor: it understates the per-chart cost rather
    /// than inventing one.
    charts: usize,
}

/// Stage `arrival_flash`.
///
/// Acts on every tick rather than after a dwell: the flash has to be lit for the window to measure
/// anything, and the row's own clock ends it.
pub(in crate::firetest) fn drive(
    runtime: &mut Runtime,
    backend: &mut Backend,
    cx: &mut Context<Backend>,
) -> StageStep {
    // Resolved inside the branches that light the flash, never once per tick: this stage runs at
    // tick rate for its whole window and its CPU average is one of the numbers being compared, so
    // work done here to be tidy shows up as the flash's cost.
    let Some(mut run) = runtime.flash.take() else {
        let charts = light(
            backend,
            Some(Instant::now()),
            MoonPalette::active(cx).accent,
        );
        if charts == 0 {
            return StageStep::Fail(
                "arrival_flash found no live chart to flash; the phase would measure nothing"
                    .into(),
            );
        }
        let now = Instant::now();
        runtime.flash = Some(FlashRun {
            started: now,
            last_lit: now,
            ignitions: 1,
            charts,
        });
        firetest_info(&format!(
            "[firetest] arrival_flash start charts={charts} window_ms={} rearm_ms={}",
            runtime.config.flash.as_millis(),
            REARM.as_millis()
        ));
        return StageStep::Stay;
    };

    if run.started.elapsed() >= runtime.config.flash {
        // Cleared explicitly, not left to expire: `baseline` runs next and must not inherit a
        // border that is still 2.6 s from going out. The clear also reports the count, so a chart
        // that arrived after the last re-light still widens the divisor.
        run.charts = run
            .charts
            .max(light(backend, None, MoonPalette::active(cx).accent));
        runtime.flash_charts = Some(run.charts);
        firetest_info(&format!(
            "[firetest] arrival_flash done charts={} ignitions={} window_ms={}",
            run.charts,
            run.ignitions,
            run.started.elapsed().as_millis()
        ));
        return StageStep::Next;
    }
    if run.last_lit.elapsed() >= REARM {
        run.last_lit = Instant::now();
        run.ignitions += 1;
        run.charts = run.charts.max(light(
            backend,
            Some(run.last_lit),
            MoonPalette::active(cx).accent,
        ));
    }
    runtime.flash = Some(run);
    StageStep::Stay
}

/// Stage `flash_storm`: the mouse storm again, with the border flashing the whole way through.
///
/// The two costs were measured apart — a flash with no cursor, a cursor with no flash — and apart
/// is not how they happen: a detect opens a chart while the user is already moving over it. Both
/// ask the own-pass for presents and both rebuild the readout, so together they can cost more than
/// the sum, and neither existing phase can see it.
///
/// Scored through the SAME `check_storm` against the SAME baseline as the other two storms, so it
/// inherits every per-frame ceiling by construction rather than needing its own.
pub(in crate::firetest) fn drive_storm(
    runtime: &mut Runtime,
    backend: &mut Backend,
    cx: &mut Context<Backend>,
) -> StageStep {
    let Some(mut run) = runtime.flash.take() else {
        let charts = light(
            backend,
            Some(Instant::now()),
            MoonPalette::active(cx).accent,
        );
        if charts == 0 {
            return StageStep::Fail(
                "flash_storm found no live chart to flash; the phase would measure a plain storm"
                    .into(),
            );
        }
        // Lit BEFORE the storm starts, so no sampled second of this phase is cursor-only.
        match runtime.start_mouse_storm() {
            Ok(storm) => runtime.storm = Some(storm),
            Err(error) => return StageStep::Fail(error),
        }
        let now = Instant::now();
        runtime.flash = Some(FlashRun {
            started: now,
            last_lit: now,
            ignitions: 1,
            charts,
        });
        firetest_info(&format!("[firetest] flash_storm start charts={charts}"));
        return StageStep::Stay;
    };

    let storm_done = runtime.storm.as_ref().is_some_and(MouseStorm::is_done);
    if storm_done || run.started.elapsed() >= runtime.config.storm {
        runtime.stop_storm();
        run.charts = run
            .charts
            .max(light(backend, None, MoonPalette::active(cx).accent));
        firetest_info(&format!(
            "[firetest] flash_storm done charts={} ignitions={} window_ms={}",
            run.charts,
            run.ignitions,
            run.started.elapsed().as_millis()
        ));
        return StageStep::Next;
    }
    if run.last_lit.elapsed() >= REARM {
        run.last_lit = Instant::now();
        run.ignitions += 1;
        run.charts = run.charts.max(light(
            backend,
            Some(run.last_lit),
            MoonPalette::active(cx).accent,
        ));
    }
    runtime.flash = Some(run);
    StageStep::Stay
}

/// Stamp (or clear) the flash on every live chart, returning how many took it.
fn light(backend: &mut Backend, at: Option<Instant>, accent: u32) -> usize {
    backend
        .live_chart_consumers()
        .into_iter()
        .filter(|chart| chart.set_firetest_arrival_flash(at, accent))
        .count()
}
