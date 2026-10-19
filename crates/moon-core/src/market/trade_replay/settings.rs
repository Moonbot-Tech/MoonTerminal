//! The live `[trade_replay]` settings of `storage.toml`: read once from the file on first use,
//! then from process-wide cells the Storage tab moves.
//!
//! One place for both because they are read from different threads — the replay worker, the
//! tuner's fetch job, the coordination tick — and none of them may open the file: a setting
//! the tab just flipped must be what the next request is built with.

use crate::station_api::TapeWindow;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

/// Return the live tape window in the station API's seconds and minutes.
pub fn window() -> TapeWindow {
    init();
    TapeWindow {
        margin_s: MARGIN_S.load(Ordering::Relaxed),
        long_position_min: LONG_POSITION_MIN.load(Ordering::Relaxed),
    }
}

/// Reconcile each setting independently, then enforce the local picker bounds.
pub fn adopt_larger(local: TapeWindow, station: TapeWindow) -> TapeWindow {
    normalize(TapeWindow {
        margin_s: local.margin_s.max(station.margin_s),
        long_position_min: local.long_position_min.max(station.long_position_min),
    })
}

/// Apply the picker bounds to every shared-window writer and reconciliation.
pub fn normalize(window: TapeWindow) -> TapeWindow {
    use crate::config::storage::{clamp_long_position_min, snap_trade_margin_s};
    TapeWindow {
        margin_s: snap_trade_margin_s(window.margin_s),
        long_position_min: clamp_long_position_min(window.long_position_min),
    }
}

/// Live value of `[trade_replay] margin_s` — how many seconds of prints a window asks for
/// around a trade, per end ([`super::ReplayWindow::margin_ms`]).
static MARGIN_S: AtomicU32 = AtomicU32::new(crate::config::storage::DEFAULT_TRADE_MARGIN_S);
/// Live value of `[trade_replay] long_position_min`.
static LONG_POSITION_MIN: AtomicU32 =
    AtomicU32::new(crate::config::storage::DEFAULT_LONG_POSITION_MIN);
static INIT: OnceLock<()> = OnceLock::new();
/// Process-wide tuner floor pushed by the station's controller, independent of local storage.
static MIN_TAIL_S: AtomicU32 = AtomicU32::new(crate::db::tuner::ticks::tail::DEFAULT_MIN_TAIL_S);

/// Apply the floor, invalidating catch-up only when the recorded horizon changes.
pub fn set_min_tail_s(s: u32) {
    use crate::db::tuner::ticks::tail::{clamp_min_tail_s, record_tail_ms};
    let margin = margin_ms();
    let s = clamp_min_tail_s(s);
    let before = MIN_TAIL_S.swap(s, Ordering::Relaxed);
    if record_tail_ms(before, margin) != record_tail_ms(s, margin) {
        crate::market::tape_recorder::catchup::window_changed();
    }
}

/// Current process-wide tuner floor in seconds.
pub fn min_tail_s() -> u32 {
    MIN_TAIL_S.load(Ordering::Relaxed)
}

/// Tail required by both the recorder and the tuner, including the configured margin.
pub fn record_tail_ms() -> i64 {
    crate::db::tuner::ticks::tail::record_tail_ms(min_tail_s(), margin_ms())
}

/// Load the file into the cells once; every setter calls it first, or the file's value would
/// land on top of the tab's on the first read.
fn init() {
    INIT.get_or_init(|| {
        let cfg = crate::config::storage::load();
        MARGIN_S.store(cfg.trade_replay.margin_s, Ordering::Relaxed);
        LONG_POSITION_MIN.store(cfg.trade_replay.long_position_min, Ordering::Relaxed);
        // Once per launch, so a file migrated from `margin_min` shows what it was read as.
        log::info!(
            "[x] trade-replay settings: margin {} s, long position from {} min",
            cfg.trade_replay.margin_s,
            cfg.trade_replay.long_position_min
        );
    });
}

/// The configured margin, in milliseconds — what every new [`super::ReplayWindow`] is built
/// with: a chart's, a tuner's, the close-time capture's, and the cleanup's claims. Its floor is
/// the model's pad ([`super::MODEL_PAD_MS`], `config::storage::TRADE_MARGIN_STEPS_S`), so every
/// position carries the whole run-up and tail — a long one gets the margin on both sides of each
/// end ([`super::ReplayWindow::focus_spans`]).
pub fn margin_ms() -> i64 {
    init();
    i64::from(MARGIN_S.load(Ordering::Relaxed)) * 1_000
}

/// Move the live margin; the Storage tab writes `storage.toml` beside this. Windows already open
/// keep the margin they were built with; the next one asks for the new stretch, and the tile
/// store hands back what earlier windows already fetched of it. Snapped onto the step list like
/// the file is on load, so the cell never holds a value the tab cannot show.
/// A changed effective value invalidates the station's settled catch-up windows.
pub fn set_margin_s(secs: u32) {
    init();
    let secs = crate::config::storage::snap_trade_margin_s(secs);
    if MARGIN_S.swap(secs, Ordering::Relaxed) != secs {
        crate::market::tape_recorder::catchup::window_changed();
    }
}

/// How long a position must be held to be walked as its two ends — `[trade_replay]
/// long_position_min`, in milliseconds. Captured where a window is built
/// ([`super::replay_window_ms`] → [`super::ReplayWindow::long_position_ms`]) and read from the
/// window from then on, like the margin: a request is clustered, walked, judged and drawn at
/// different moments, and every one of them must split it the same way.
pub fn long_position_ms() -> i64 {
    init();
    i64::from(LONG_POSITION_MIN.load(Ordering::Relaxed)) * 60_000
}

/// Move the live threshold; the Storage tab writes `storage.toml` beside this. Bounded like
/// the file is on load.
/// A changed effective value invalidates the station's settled catch-up windows.
pub fn set_long_position_min(minutes: u32) {
    init();
    let minutes = crate::config::storage::clamp_long_position_min(minutes);
    if LONG_POSITION_MIN.swap(minutes, Ordering::Relaxed) != minutes {
        crate::market::tape_recorder::catchup::window_changed();
    }
}

#[cfg(test)]
mod tests;
