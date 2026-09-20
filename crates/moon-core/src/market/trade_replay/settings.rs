//! The live `[trade_replay]` settings of `storage.toml`: read once from the file on first use,
//! then from process-wide cells the Storage tab moves.
//!
//! One place for both because they are read from different threads — the replay worker, the
//! tuner's fetch job, the coordination tick — and none of them may open the file: a setting
//! the tab just flipped must be what the next request is built with.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::MINUTE_MS;

/// Live value of `[trade_replay] margin_min` — how many minutes of prints a window asks for
/// around a trade, per end ([`super::ReplayWindow::margin_ms`]).
static MARGIN_MIN: AtomicU32 = AtomicU32::new(crate::config::storage::DEFAULT_TRADE_MARGIN_MIN);
/// Live value of `[trade_replay] autoload_missing`.
static TAPE_AUTOLOAD: AtomicBool = AtomicBool::new(false);
static INIT: OnceLock<()> = OnceLock::new();

/// Load the file into the cells once; every setter calls it first, or the file's value would
/// land on top of the tab's on the first read.
fn init() {
    INIT.get_or_init(|| {
        let cfg = crate::config::storage::load();
        MARGIN_MIN.store(cfg.trade_replay.margin_min, Ordering::Relaxed);
        TAPE_AUTOLOAD.store(cfg.trade_replay.autoload_missing, Ordering::Relaxed);
    });
}

/// The configured margin, in milliseconds — what every new [`super::ReplayWindow`] and every
/// close-time capture is built with.
pub fn margin_ms() -> i64 {
    init();
    i64::from(MARGIN_MIN.load(Ordering::Relaxed)) * MINUTE_MS
}

/// The margin a MODEL's window is built with: the chart's margin, but never less than TWICE
/// the model's own pad ([`super::MODEL_PAD_MS`]). The plan's trade tiles and the model's
/// required span are both clipped to the window's focus, so a chart margin under the pad —
/// "the position alone" is a valid setting — would otherwise leave the model without its
/// run-up and its tail and never say so. Twice, because a long position's focus centres the
/// margin on each end ([`super::ReplayWindow::focus_spans`]): half of it lies outside the
/// position, and that half must still be a whole pad.
pub fn model_margin_ms() -> i64 {
    margin_ms().max(2 * super::MODEL_PAD_MS)
}

/// Move the live margin; the Storage tab writes `storage.toml` beside this. Windows already open
/// keep the margin they were built with; the next one asks for the new stretch, and the tile
/// store hands back what earlier windows already fetched of it.
pub fn set_margin_min(minutes: u32) {
    init();
    MARGIN_MIN.store(
        minutes.min(crate::config::storage::MAX_TRADE_MARGIN_MIN),
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
