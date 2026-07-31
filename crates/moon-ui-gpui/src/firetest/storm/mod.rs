//! The native mouse storm: a background thread that moves the real OS cursor in a circle over the
//! chart's real bounds.
//!
//! This deliberately goes through the platform's input path — `SetCursorPos` plus a posted
//! `WM_MOUSEMOVE` on Windows, CoreGraphics events on macOS, XTest on X11 — instead of calling the
//! chart's mouse handler directly. A direct call would exercise the handler while skipping exactly
//! the window/event plumbing where a "wake the whole scene on every move" regression lives.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub(super) use win::start_mouse_storm;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(super) use macos::start_mouse_storm;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(super) use linux::start_mouse_storm;

/// A running storm thread. Dropping the handle does not stop the thread; [`MouseStorm::stop`] does.
///
/// The fields stay private to this module: the per-platform submodules construct it and are
/// descendants, so nothing outside the storm ever touches the flags directly.
pub(super) struct MouseStorm {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl MouseStorm {
    /// Whether the storm thread has finished on its own (its duration elapsed).
    pub(super) fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    /// Ask the storm thread to stop. Consumes the handle so a stage cannot keep polling a storm it
    /// has already ended.
    pub(super) fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
