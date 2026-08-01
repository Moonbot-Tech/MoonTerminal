//! Stage `start`: hold until the cores have settled, instead of sleeping a fixed interval.
//!
//! A fixed pre-roll is wrong in both directions. With one core it wastes the difference; with
//! twenty it starts measuring while sessions are still connecting, and every later stage inherits
//! a machine that is still busy coming up. That is how `order_cancel_lag` came to fail by TIMEOUT
//! rather than by lag on a bench with 22 servers — a run that reports "too slow" when the real
//! answer is "not ready yet" is worse than no run.
//!
//! So this reads the authoritative signal (`ConnStatus`) instead of guessing from the clock, and
//! keeps only a scaled budget as a backstop.

use std::time::Duration;

use gpui::Context;

use crate::Backend;
use crate::firetest::Runtime;
use crate::firetest::plan::StageStep;

/// Fixed part of the settle budget — what a machine needs before the first session even reports.
const SETTLE_BASE: Duration = Duration::from_secs(5);

/// Per-core part of the settle budget. Connections are set up concurrently, so this is not the cost
/// of a connection; it is the observed drift of the slowest one as the pool grows.
const SETTLE_PER_CORE: Duration = Duration::from_millis(100);

/// How much longer than the floor the stage will wait for stragglers before measuring anyway. A
/// core stuck in `Connecting` must delay the run, not cancel it.
const SETTLE_EXTENSION_MAX: Duration = Duration::from_secs(30);

/// Stage `start`: wait for every core to reach a settled state, bounded by a core-scaled budget.
///
/// Returns `Next` — never `Fail` — when the budget runs out. A core stuck in `Connecting` forever
/// is a fact about the bench, not a defect in the change under test, and the stages that actually
/// need a working core carry their own deadlines (`open_chart` fails with "no active visible
/// core/window", which says something true). Failing here would blame the diff for the network.
pub(in crate::firetest) fn await_cores(
    runtime: &mut Runtime,
    backend: &mut Backend,
    _cx: &mut Context<Backend>,
) -> StageStep {
    let total = backend.session.sessions().len();
    let pending = backend.session.cores_pending();
    let elapsed = runtime.phase_since.elapsed();
    // `total.max(1)`: a run that reaches here before ANY session has registered must not conclude
    // there is nothing to wait for. Zero sessions one second in means "not reported yet", and an
    // earlier version of this stage read it as "nothing configured" and skipped the whole wait.
    let floor = SETTLE_BASE + SETTLE_PER_CORE * total.max(1) as u32;

    // The floor is a MINIMUM, not the decision. `Ready` only means the socket came up: the core
    // then pushes its initial state — orders and the rest — and during that burst both the core and
    // the link are loaded. Nothing exposes "initial sync complete", so this waits it out by the
    // clock and says so rather than pretending to read a flag.
    if elapsed < floor {
        runtime.wait_log(&format!(
            "settling: {pending}/{total} cores pending, {}s of {}s floor",
            elapsed.as_secs(),
            floor.as_secs()
        ));
        return StageStep::Stay;
    }
    if pending == 0 {
        return StageStep::Next;
    }
    if elapsed >= floor + SETTLE_EXTENSION_MAX {
        runtime.wait_log(&format!(
            "cores did not settle: {pending}/{total} still connecting — measuring anyway"
        ));
        return StageStep::Next;
    }
    runtime.wait_log(&format!(
        "waiting for cores to settle: {pending}/{total} pending"
    ));
    StageStep::Stay
}
