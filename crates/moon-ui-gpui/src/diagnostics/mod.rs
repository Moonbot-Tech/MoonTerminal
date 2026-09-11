//! Development and crash diagnostics: the native crash handler, the ring of window messages it
//! reports from, and the debug tool window.

pub(crate) mod crash;
pub(crate) mod debug_window;
#[cfg(windows)]
pub(crate) mod msg_ring;

/// Milliseconds since the first call in this process — the crash handler's install, which is the
/// first thing startup does after the working directory is settled. Only differences are ever
/// printed: the age of a recorded message, the uptime on a crash line.
pub(crate) fn now_ms() -> u64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    u64::try_from(
        ORIGIN
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
