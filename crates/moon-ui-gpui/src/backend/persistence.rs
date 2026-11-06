//! Ordering boundary between automatic snapshots and deliberate config/key changes.

use crate::Backend;

impl Backend {
    /// Complete earlier snapshots before a deliberate synchronous write can supersede them.
    /// Routine saves only enqueue; Settings, pairing and key changes may wait at their commit boundary.
    pub(crate) fn finish_pending_persistence(&mut self) {
        let acknowledgement = self.persistence.borrow_mut().finish_pending();
        if let Some(acknowledgement) = acknowledgement {
            crate::startup::apply_persistence_ack(self, acknowledgement);
        }
    }
}
