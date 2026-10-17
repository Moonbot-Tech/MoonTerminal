//! Tracks window requests without delivering background trace backfill to the UI.

use std::collections::HashSet;

/// Bounded request filter that bypasses filtering for this connection after overflow.
#[derive(Default)]
pub(super) struct UiTraceAsks {
    set: HashSet<i64>,
    overflow: bool,
}

impl UiTraceAsks {
    /// Records an ask, falling back to unfiltered delivery rather than evicting an awaited UID.
    pub(super) fn ask(&mut self, uid: i64) {
        if self.set.len() < 1024 {
            self.set.insert(uid);
        } else {
            self.overflow = true;
        }
    }

    /// Consumes a tracked ask, retaining bypass for late replies outside the bounded set.
    pub(super) fn deliver(&mut self, uid: i64) -> bool {
        let asked = self.set.remove(&uid);
        if self.overflow { true } else { asked }
    }
}

#[cfg(test)]
mod tests;
