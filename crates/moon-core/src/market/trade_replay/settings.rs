//! The live `[trade_replay]` settings of `storage.toml`: read once from the file on first use,
//! then from process-wide cells the Storage tab moves.
//!
//! One place for both because they are read from different threads — the replay worker, the
//! tuner's fetch job, the coordination tick — and none of them may open the file: a setting
//! the tab just flipped must be what the next request is built with.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Live value of `[trade_replay] margin_s` — how many seconds of prints a window asks for
/// around a trade, per end ([`super::ReplayWindow::margin_ms`]).
static MARGIN_S: AtomicU32 = AtomicU32::new(crate::config::storage::DEFAULT_TRADE_MARGIN_S);
/// Live value of `[trade_replay] autoload_missing`.
static TAPE_AUTOLOAD: AtomicBool = AtomicBool::new(false);
/// Live value of `[trade_replay] long_position_min`.
static LONG_POSITION_MIN: AtomicU32 =
    AtomicU32::new(crate::config::storage::DEFAULT_LONG_POSITION_MIN);
/// Live value of `[trade_replay] cleanup_at_startup`.
static CLEANUP_AT_STARTUP: AtomicBool = AtomicBool::new(false);
static INIT: OnceLock<()> = OnceLock::new();

/// Load the file into the cells once; every setter calls it first, or the file's value would
/// land on top of the tab's on the first read.
fn init() {
    INIT.get_or_init(|| {
        let cfg = crate::config::storage::load();
        MARGIN_S.store(cfg.trade_replay.margin_s, Ordering::Relaxed);
        TAPE_AUTOLOAD.store(cfg.trade_replay.autoload_missing, Ordering::Relaxed);
        LONG_POSITION_MIN.store(cfg.trade_replay.long_position_min, Ordering::Relaxed);
        CLEANUP_AT_STARTUP.store(cfg.trade_replay.cleanup_at_startup, Ordering::Relaxed);
        // Once per launch, so a file migrated from `margin_min` shows what it was read as.
        log::info!(
            "[x] trade-replay settings: margin {} s, long position from {} min, tape autoload {}, cleanup at startup {}",
            cfg.trade_replay.margin_s,
            cfg.trade_replay.long_position_min,
            cfg.trade_replay.autoload_missing,
            cfg.trade_replay.cleanup_at_startup
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
pub fn set_margin_s(secs: u32) {
    init();
    MARGIN_S.store(
        crate::config::storage::snap_trade_margin_s(secs),
        Ordering::Relaxed,
    );
}

/// Whether the terminal fetches the tape of recent closed trades on its own once the cores are
/// up — `[trade_replay] autoload_missing`.
pub fn tape_autoload() -> bool {
    init();
    TAPE_AUTOLOAD.load(Ordering::Relaxed)
}

/// Move the live autoload switch; the Storage tab writes the file beside this.
pub fn set_tape_autoload(on: bool) {
    init();
    TAPE_AUTOLOAD.store(on, Ordering::Relaxed);
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
pub fn set_long_position_min(minutes: u32) {
    init();
    LONG_POSITION_MIN.store(
        crate::config::storage::clamp_long_position_min(minutes),
        Ordering::Relaxed,
    );
}

/// Whether the terminal runs the trade-tape cleanup on its own once the cores are up —
/// `[trade_replay] cleanup_at_startup`.
pub fn cleanup_at_startup() -> bool {
    init();
    CLEANUP_AT_STARTUP.load(Ordering::Relaxed)
}

/// Move the live startup-cleanup switch; the Storage tab writes the file beside this.
pub fn set_cleanup_at_startup(on: bool) {
    init();
    CLEANUP_AT_STARTUP.store(on, Ordering::Relaxed);
}
