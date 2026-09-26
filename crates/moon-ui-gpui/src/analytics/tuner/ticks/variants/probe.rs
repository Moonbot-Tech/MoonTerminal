//! Observation channel for the axis' search (`MOON_TUNER_SEARCH_PROBE=search`), gated by the
//! environment like `MOON_ANALYTICS_PROBE` and inert unless set: the first time a load folds
//! with fit rows, "Search all" is pressed as a click would press it, and the run's progress is
//! logged once a second off the handle — what the search itself has done, beside what the row
//! shows — so a run that stops moving can be read from the log instead of watched by hand.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use moon_core::db::tuner::threshold_search::SearchHandle;

/// The channel's spec when armed: `search` presses at the first fold; `search:<s>` at the first
/// fold that many seconds after the first one — a window left quiet, as a human would find it;
/// `search:<s>:<n>` with `n` restarts instead of the box's, for a run long enough to meet
/// whatever the channel is watching for.
fn armed_spec() -> Option<(Duration, Option<usize>)> {
    static ON: OnceLock<Option<(Duration, Option<usize>)>> = OnceLock::new();
    *ON.get_or_init(|| {
        let v = std::env::var("MOON_TUNER_SEARCH_PROBE").ok()?;
        let mut parts = v.split(':');
        if parts.next()? != "search" {
            return None;
        }
        let delay = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let restarts = parts.next().and_then(|s| s.parse().ok());
        Some((Duration::from_secs(delay), restarts))
    })
}

fn armed_delay() -> Option<Duration> {
    armed_spec().map(|(delay, _)| delay)
}

/// The restarts the probe's run uses in place of the box's, when the spec names them.
pub(super) fn restarts() -> Option<usize> {
    armed_spec().and_then(|(_, n)| n)
}

/// Whether the channel is armed.
fn armed() -> bool {
    armed_delay().is_some()
}

/// Whether the search should be pressed now: once per process, and only when armed.
pub(super) fn fire() -> bool {
    static FIRED: AtomicBool = AtomicBool::new(false);
    static FIRST: OnceLock<std::time::Instant> = OnceLock::new();
    let Some(delay) = armed_delay() else {
        return false;
    };
    FIRST.get_or_init(std::time::Instant::now).elapsed() >= delay
        && !FIRED.swap(true, Ordering::Relaxed)
}

/// Log a run's progress once a second, on a thread of its own, until it finishes or stops —
/// only when armed.
pub(super) fn watch(handle: SearchHandle, total: usize, seq: u64) {
    if !armed() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("ticks-search-probe".into())
        .spawn(move || {
            let mut last = usize::MAX;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let done = handle.completed();
                if done != last {
                    last = done;
                    log::info!(
                        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                        "[x] ticks search probe: #{seq} {done}/{total} restart(s) done"
                    );
                }
                if done >= total || handle.is_cancelled() {
                    break;
                }
            }
        });
}

/// Log the search row's status as the frame paints it — only when it changed, and only when
/// armed: what the row SHOWS, beside what the run has done ([`watch`]).
pub(in super::super) fn painted(status: &str) {
    if !armed() {
        return;
    }
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let Ok(mut last) = LAST.lock() else { return };
    if *last != status {
        status.clone_into(&mut last);
        log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks search probe: painted {status:?}"
        );
    }
}

/// Every kind's sections and their fields as the first connected core's live schema files them,
/// one line per section — the answer to "does this kind have that section", read off the core.
pub(super) fn dump_schema(store: &moon_core::session::CoreStore) {
    let Some(schema) = store.cores().find_map(|(_, core)| core.schema.as_ref()) else {
        log::info!(target: moon_core::diagnostics::TICKS_AXIS_TARGET, "[x] ticks schema: none");
        return;
    };
    for kind in &schema.kinds {
        for section in &kind.sections {
            let fields: Vec<&str> = section.fields.iter().map(|f| f.name.as_str()).collect();
            log::info!(
                target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                "[x] ticks schema: {} ({}) / {}: {}",
                kind.name,
                kind.ordinal,
                section.title,
                fields.join(",")
            );
        }
    }
}
