//! Stage `idle_floor`: with a live chart open and nothing touching it, the UI must go quiet.
//!
//! The only stage that measures the app when nothing is forcing it to work. A live feed still
//! legitimately drives the chart's own pass, so what this catches is the GPUI view layer waking
//! WITHOUT input — a broadcast on every tick, a panel repainting on a revision that did not
//! change, a timer nobody needed.
//!
//! Its act runs at the END of the dwell, after the samples are already in: the row's `min_dwell`
//! IS the measurement window. All the act does is record how much app was being measured, which is
//! what makes the ceilings mean the same thing on a one-core bench and a fifty-core one.

use gpui::Context;

use crate::Backend;

use crate::firetest::plan::StageStep;
use crate::firetest::{Runtime, bench};

/// Record the bench shape the idle window was measured against, then move on.
pub(in crate::firetest) fn measure(
    runtime: &mut Runtime,
    backend: &mut Backend,
    cx: &mut Context<Backend>,
) -> StageStep {
    runtime.bench = Some(bench::capture(backend, cx));
    StageStep::Next
}
