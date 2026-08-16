//! Diagnostic channel centered on the detect pipeline (`channels.detect`). Its primary path is:
//!   feed.detects(flag) → Event::Detect → FeedMsg::Detects → store.detects_rev → ChartTabs::ingest
//! Related UI, rendering, sound, detached-window, and persistence events use the same channel for
//! end-to-end correlation.
//! Inactive by default in EVERY build. Enable it with `channels.detect` in `cfg/diagnostics.toml`
//! (or `MOON_DETECT_DIAG=1`) → lines are appended to `logs/detect_diag.log` beside the application's
//! own logs. Public builds stay clean.
//!
//! This channel lives in moon-core (the lower-level crate) so callers can instrument both core
//! feed/store operations and moon-ui-gpui consumers.

/// Whether the detect channel is on, as set by `channels.detect` in `cfg/diagnostics.toml` or by
/// the matching environment variable.
///
/// Reads a live atomic rather than caching the environment once: the switch can be flipped while
/// the terminal runs, and a detect worth tracing is rarely one a restart would preserve.
pub fn enabled() -> bool {
    crate::diagnostics::detect()
}

/// Appends a line to `logs/detect_diag.log` (a no-op while the channel is off). File line order
/// reflects append/write order, not necessarily causal event order across threads. Entries have no
/// timestamps; use the regular log for coarse time correlation.
pub fn line(msg: &str) {
    if !enabled() {
        return;
    }
    crate::diagnostics::channel_line("detect_diag.log", msg);
}
