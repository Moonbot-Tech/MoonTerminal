//! Stage `start`: hold until the cores have settled, instead of sleeping a fixed interval.
//!
//! A fixed pre-roll is wrong in both directions. With one core it wastes the difference; with
//! twenty it starts measuring while sessions are still connecting, and every later stage inherits
//! a machine that is still busy coming up.
//!
//! So this reads the authoritative signal (`ConnStatus`) instead of guessing from the clock, and
//! keeps only a small scaled budget as a backstop.

use std::time::{Duration, Instant};

use gpui::Context;

use crate::Backend;
use crate::firetest::Runtime;
use crate::firetest::plan::StageStep;

/// Fixed part of the settle budget, counted from the moment sessions REGISTER rather than from
/// process start. That is what keeps it small: an earlier version waited five seconds purely to
/// cover the gap before the first session reported, and every bench paid it whether it had one core
/// or twenty. What remains covers the initial data burst that follows a connection.
const SETTLE_BASE: Duration = Duration::from_secs(1);

/// How long to wait for the first session to register before giving up on the idea that any will.
/// A configuration with no servers must not stall the run.
const REGISTER_MAX: Duration = Duration::from_secs(10);

/// Per-core part of the settle budget. Connections are set up concurrently, so this is not the cost
/// of a connection; it is the observed drift of the slowest one as the pool grows.
const SETTLE_PER_CORE: Duration = Duration::from_millis(100);

/// How much longer than the floor the stage will wait for stragglers before measuring anyway. A
/// core stuck in `Connecting` must delay the run, not cancel it.
const SETTLE_EXTENSION_MAX: Duration = Duration::from_secs(30);

/// Stage `start`: wait for every core to reach a settled state, bounded by a core-scaled budget.
///
/// Returns `Next`, never `Fail`, when the budget runs out. A core stuck in `Connecting` forever is
/// a fact about the bench, not a defect in the change under test, and the stages that actually need
/// a working core carry their own deadlines: `open_chart` fails with "no active visible
/// core/window", which says something true. Failing here would blame the diff for the network.
pub(in crate::firetest) fn await_cores(
    runtime: &mut Runtime,
    backend: &mut Backend,
    _cx: &mut Context<Backend>,
) -> StageStep {
    let total = backend.session.sessions().len();
    if total == 0 {
        // Nobody has reported yet. This is the ambiguity the whole stage is built around: zero
        // sessions is NOT "nothing to wait for", and an earlier version read it that way and
        // skipped the wait entirely on every run.
        if runtime.phase_since.elapsed() >= REGISTER_MAX {
            return StageStep::Next;
        }
        runtime.wait_log("waiting for core sessions to register");
        return StageStep::Stay;
    }
    // Everything below is timed from REGISTRATION, not from process start, so a bench with one core
    // does not pay for the startup a bench with twenty needs.
    let seen = *runtime.cores_seen_at.get_or_insert_with(Instant::now);
    let pending = backend.session.cores_pending();
    let elapsed = seen.elapsed();
    let floor = SETTLE_BASE + SETTLE_PER_CORE * total as u32;

    // The floor is a MINIMUM, not the decision. `Ready` only means the socket came up: the core
    // then pushes its initial state, orders and the rest, and during that burst both the core and
    // the link are loaded. Nothing exposes "initial sync complete", so this waits it out by the
    // clock and says so rather than pretending to read a flag.
    if elapsed < floor {
        runtime.wait_log(&format!(
            "settling: {pending}/{total} cores pending, {}ms of {}ms floor",
            elapsed.as_millis(),
            floor.as_millis()
        ));
        return StageStep::Stay;
    }
    if pending == 0 {
        return StageStep::Next;
    }
    if elapsed >= floor + SETTLE_EXTENSION_MAX {
        runtime.wait_log(&format!(
            "cores did not settle: {pending}/{total} still connecting, measuring anyway"
        ));
        return StageStep::Next;
    }
    runtime.wait_log(&format!(
        "waiting for cores to settle: {pending}/{total} pending"
    ));
    StageStep::Stay
}
