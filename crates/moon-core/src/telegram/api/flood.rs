//! One bot credential's Telegram flood deadline, shared by its independent API clients.

use super::ApiError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A successful call on one client cannot clear a flood wait observed by another.
#[derive(Clone, Default)]
pub(crate) struct FloodWait {
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl FloodWait {
    /// Extend the shared deadline for Telegram's explicit retry_after; other failures stay local.
    pub(super) fn observe(&self, error: &ApiError, now: Instant) {
        let ApiError::Telegram {
            retry_after_secs: Some(seconds),
            ..
        } = error
        else {
            return;
        };
        let until = now + Duration::from_secs(u64::from(*seconds));
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *deadline = Some(deadline.map_or(until, |previous| previous.max(until)));
    }

    /// Remaining mandatory delay at now; expired deadlines add no local pacing.
    pub(super) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .and_then(|deadline| deadline.checked_duration_since(now))
            .filter(|remaining| !remaining.is_zero())
    }
}

#[cfg(test)]
mod tests;
