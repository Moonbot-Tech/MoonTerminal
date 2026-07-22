//! Environment-gated diagnostic channel centered on the detect pipeline. Its primary path is:
//!   feed.detects(flag) → Event::Detect → FeedMsg::Detects → store.detects_rev → ChartTabs::ingest
//! Related UI, rendering, sound, detached-window, and persistence events use the same channel for
//! end-to-end correlation.
//! Inactive by default in EVERY build (like `diag.rs`/`MOON_RENDER_DIAG`). Enable it explicitly with
//! `MOON_DETECT_DIAG=1` → lines are appended to `detect_diag.log` in the cwd. Public builds stay clean.
//!
//! This channel lives in moon-core (the lower-level crate) so callers can instrument both core
//! feed/store operations and moon-ui-gpui consumers.

use std::sync::OnceLock;

/// Returns whether the `MOON_DETECT_DIAG` environment variable is set to any value, read once.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MOON_DETECT_DIAG").is_some())
}

/// Appends a line to `detect_diag.log` (a no-op without the environment variable). File line order
/// reflects append/write order, not necessarily causal event order across threads. Entries have no
/// timestamps; use the regular log for coarse time correlation.
pub fn line(msg: &str) {
    if !enabled() {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("detect_diag.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}
