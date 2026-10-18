//! Startup tape retention always runs once before autoload may fetch missing history.
//! A short delay lets core catalogs resolve the retention owners before the worker starts.

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
    /// Waiting for the cores.
    Waiting(Instant),
    /// The cleanup is on the background executor.
    Running,
    /// Ran or failed — nothing more this process.
    Done,
}

static PHASE: OnceLock<Mutex<Phase>> = OnceLock::new();

/// Initialize the first waiting deadline and recover a poisoned scheduler lock.
fn lock() -> std::sync::MutexGuard<'static, Phase> {
    PHASE
        .get_or_init(|| Mutex::new(Phase::Waiting(Instant::now() + FIRST_DELAY)))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether startup retention has finished, including a reported failure.
pub(crate) fn clear_for_autoload() -> bool {
    *lock() == Phase::Done
}

/// Wait for catalogs, then run retention once on the background executor.
pub(crate) fn tick(backend: &Backend, cx: &App) {
    let mut phase = lock();
    match *phase {
        Phase::Waiting(due) if due <= Instant::now() => {
            *phase = Phase::Running;
            let context = CleanupContext::of(backend);
            cx.background_executor()
                .spawn(async move {
                    match context.run() {
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

#[cfg(test)]
mod tests;
