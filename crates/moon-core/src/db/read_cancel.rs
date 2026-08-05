//! Opt-in cancellation for replaceable report-replica reads.
//!
//! UI work installs a request token around a public database call. A reader opened inside that
//! scope receives a SQLite progress handler; ordinary readers, writes, and explicitly long-lived
//! searches remain unchanged because no token is present in their thread-local context.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::Connection;

thread_local! {
    /// Cancellation token inherited by readers opened inside the current synchronous call.
    static CURRENT: RefCell<Option<ReadCancellation>> = const { RefCell::new(None) };
    /// Whether this thread's SQLite progress callback requested the next interrupt error.
    static REQUESTED_INTERRUPT: Cell<bool> = const { Cell::new(false) };
}

/// Cooperative cancellation token for one replaceable database request.
#[derive(Clone, Debug, Default)]
pub struct ReadCancellation(Arc<AtomicBool>);

impl ReadCancellation {
    /// Create a live request token.
    ///
    /// Returns:
    ///     Token whose cancellation flag is initially clear.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask SQLite work carrying this token to stop at its next progress callback.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether cancellation has been requested.
    ///
    /// Returns:
    ///     True after any clone of this token calls [`Self::cancel`].
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Whether two handles identify the same request.
    ///
    /// Args:
    ///     other: Token to compare with this one.
    ///
    /// Returns:
    ///     True when both handles share one cancellation flag.
    pub fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Run one synchronous database entry point with opt-in cancellation context.
///
/// Nested scopes restore their caller's token even when the callback unwinds.
///
/// Args:
///     cancellation: Token installed on every report reader opened by `read`.
///     read: Synchronous database work executed on the current worker thread.
///
/// Returns:
///     The callback's unchanged result.
pub fn with_read_cancellation<T>(cancellation: ReadCancellation, read: impl FnOnce() -> T) -> T {
    struct Restore(Option<ReadCancellation>);

    impl Drop for Restore {
        /// Restore the enclosing cancellation scope when this call finishes or unwinds.
        fn drop(&mut self) {
            CURRENT.with(|current| {
                current.replace(self.0.take());
            });
        }
    }

    let previous = CURRENT.with(|current| current.replace(Some(cancellation)));
    let _restore = Restore(previous);
    read()
}

/// Install the current request's progress callback on a fully attached reader.
///
/// Args:
///     connection: Fresh report reader after every required database attachment.
///
/// Errors:
///     Returns a classified SQLite setup failure when callback registration fails.
pub(super) fn install_current(connection: &Connection) -> super::ReadResult<()> {
    let cancellation = CURRENT.with(|current| current.borrow().clone());
    if let Some(cancellation) = cancellation {
        connection
            .progress_handler(
                2_000,
                Some(move || {
                    let requested = cancellation.is_cancelled();
                    if requested {
                        REQUESTED_INTERRUPT.with(|interrupted| interrupted.set(true));
                    }
                    requested
                }),
            )
            .map_err(|error| {
                super::read_fail::read_fail("reports: install read cancellation", error)
            })?;
    }
    Ok(())
}

/// Consume the marker set by the progress handler that requested an SQLite interrupt.
///
/// Returns:
///     True only once for each observed requested interrupt.
pub(super) fn take_requested_interrupt() -> bool {
    REQUESTED_INTERRUPT.with(|interrupted| interrupted.replace(false))
}

#[cfg(test)]
mod tests;
