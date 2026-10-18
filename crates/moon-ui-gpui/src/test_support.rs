//! Shared isolation for tests that read or change the process-wide tape window.

/// All tests reading or mutating the shared trade window hold this lock through restoration.
pub(crate) static SHARED_WINDOW_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restore the window on unwind before releasing the crate-wide isolation lock.
pub(crate) struct SharedWindowGuard {
    window: moon_core::station_api::TapeWindow,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl SharedWindowGuard {
    /// Serialize a test's process-wide window edits and remember their previous values.
    pub(crate) fn take() -> Self {
        let lock = SHARED_WINDOW_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self {
            window: moon_core::market::trade_replay::window(),
            _lock: lock,
        }
    }
}

impl Drop for SharedWindowGuard {
    /// Restore both fields before the lock is released.
    fn drop(&mut self) {
        moon_core::market::trade_replay::set_margin_s(self.window.margin_s);
        moon_core::market::trade_replay::set_long_position_min(self.window.long_position_min);
    }
}
