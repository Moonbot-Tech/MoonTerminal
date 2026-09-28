//! The station's signals: SIGTERM and SIGINT ask it to stop, SIGHUP to re-read its configuration.
//!
//! The handlers only raise flags (`signal-hook`'s `flag::register`, async-signal-safe); the main
//! loop reads them between two drains, so nothing runs inside a handler. On a platform without
//! these signals — the developer's Windows build — nothing is registered and neither flag is ever
//! raised.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Signals {
    stop: Arc<AtomicBool>,
    reload: Arc<AtomicBool>,
}

impl Signals {
    /// Register the handlers. Fails only if the operating system refuses one.
    pub fn install() -> anyhow::Result<Self> {
        let signals = Self {
            stop: Arc::new(AtomicBool::new(false)),
            reload: Arc::new(AtomicBool::new(false)),
        };
        #[cfg(unix)]
        {
            use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
            signal_hook::flag::register(SIGTERM, Arc::clone(&signals.stop))?;
            signal_hook::flag::register(SIGINT, Arc::clone(&signals.stop))?;
            signal_hook::flag::register(SIGHUP, Arc::clone(&signals.reload))?;
        }
        Ok(signals)
    }

    /// Whether a stop was asked. Stays raised.
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Whether a reload was asked since the last call; lowers the flag.
    pub fn take_reload(&self) -> bool {
        self.reload.swap(false, Ordering::Relaxed)
    }
}
