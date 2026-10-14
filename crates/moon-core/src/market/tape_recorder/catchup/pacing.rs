//! Machine-load hysteresis applies only to the serial catch-up fetch thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOPPED: AtomicBool = AtomicBool::new(false);

/// Release pacing waits when the recorder host starts shutting down.
pub(super) fn stop() {
    STOPPED.store(true, Ordering::Relaxed);
}

/// Catch-up yields above this percentage of all machine cores.
pub(crate) const CATCHUP_CPU_CAP_PCT: f32 = 75.0;

/// A ten-point release margin prevents repeatedly starting and stopping at the ceiling.
pub(super) fn paused(was_paused: bool, cpu: f32) -> bool {
    cpu.is_finite()
        && if was_paused {
            cpu >= 65.0
        } else {
            cpu > CATCHUP_CPU_CAP_PCT
        }
}

/// Yield for at most thirty seconds so sustained load cannot starve recovery indefinitely.
pub(super) fn wait() {
    thread_local! { static PAUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    let start = Instant::now();
    PAUSED.with(|p| {
        p.set(wait_with(
            p.get(),
            crate::metrics::system_cpu_pct,
            || start.elapsed(),
            || std::thread::sleep(Duration::from_millis(250)),
            || STOPPED.load(Ordering::Relaxed),
        ))
    });
}

/// Injectable sampling and elapsed time pin bounded pauses without a thirty-second test.
fn wait_with(
    mut was_paused: bool,
    mut sample: impl FnMut() -> f32,
    elapsed: impl Fn() -> Duration,
    mut sleep: impl FnMut(),
    stopped: impl Fn() -> bool,
) -> bool {
    while !stopped() && elapsed() < Duration::from_secs(30) {
        was_paused = paused(was_paused, sample());
        if !was_paused {
            break;
        }
        sleep();
    }
    was_paused
}

#[cfg(test)]
mod tests;
