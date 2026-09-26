//! The startup cleanup: the Storage tab's trade-tape cleanup run on the terminal's own
//! initiative once the cores are up, behind `[trade_replay] cleanup_at_startup`.
//!
//! Once per process, from the coordination tick, the way the tape autoload runs
//! (`analytics::tuner::ticks::fetch::autoload`) — and BEFORE it: the cleanup keeps only what the
//! tuner's rows claim at the margin in force, the autoload then fetches what those rows still
//! lack, so nothing the autoload just paid the venues for is what the cleanup removes. The
//! autoload asks [`clear_for_autoload`] before its first pass and waits while the cleanup is
//! pending or running. The switch is read on the first tick: on, the cleanup is due after
//! [`FIRST_DELAY`], the same wait the autoload gives the cores to come up and report their
//! catalogs, so most rows resolve through the live catalog rather than by name; off, nothing
//! runs until the next launch — "at startup" means at startup, and flipping the switch on
//! mid-session must not rewrite the file the moment the checkbox is pressed.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::App;

use super::CleanupContext;
use crate::Backend;

/// How long after the first tick the cleanup runs — for the cores to come up and report their
/// catalogs. The autoload's own first delay, kept equal on purpose: the autoload is due at the
/// same moment and yields to the cleanup, so the two do not add up.
const FIRST_DELAY: Duration = Duration::from_secs(20);

/// Where the startup cleanup stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The switch has not been read yet.
    Armed,
    /// The switch was on: waiting for the cores.
    Waiting(Instant),
    /// The cleanup is on the background executor.
    Running,
    /// Ran, failed, or was off at the first tick — nothing more this process.
    Done,
}

static PHASE: OnceLock<Mutex<Phase>> = OnceLock::new();

fn lock() -> std::sync::MutexGuard<'static, Phase> {
    PHASE
        .get_or_init(|| Mutex::new(Phase::Armed))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the tape autoload may start its first pass: the startup cleanup is over, or was
/// never going to run.
pub(crate) fn clear_for_autoload() -> bool {
    *lock() == Phase::Done
}

/// The coordination tick's call: read the switch once, wait, run once. Cheap when nothing is
/// due — a lock and a clock compare.
pub(crate) fn tick(backend: &Backend, cx: &App) {
    let mut phase = lock();
    match *phase {
        Phase::Armed => {
            *phase = if moon_core::market::trade_replay::cleanup_at_startup() {
                Phase::Waiting(Instant::now() + FIRST_DELAY)
            } else {
                Phase::Done
            };
        }
        Phase::Waiting(due) if due <= Instant::now() => {
            *phase = Phase::Running;
            let context = CleanupContext::of(backend);
            cx.background_executor()
                .spawn(async move {
                    match context.run(true) {
                        Ok(preview) => log::info!(
                            "[x] startup trades cleanup: {} print(s) / {} bytes removed of {}",
                            preview.report.prints_dropped,
                            preview.report.bytes_dropped,
                            preview.report.bytes_total
                        ),
                        Err(error) => {
                            log::warn!("[x] startup trades cleanup failed: {error:#}")
                        }
                    }
                    *lock() = Phase::Done;
                })
                .detach();
        }
        Phase::Waiting(_) | Phase::Running | Phase::Done => {}
    }
}
