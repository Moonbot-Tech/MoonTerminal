//! Restart-safe outgoing answer identities; this cache never grants chat authorization.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

/// Preserve keyboard owners separately from disposable reports and Help.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct History {
    pub navigation: BTreeMap<i64, i64>,
    pub answers: BTreeMap<i64, Answer>,
}

/// Telegram's original send timestamp, never refreshed by callback edits.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct Answer {
    pub id: i64,
    pub sent_at: i64,
}

impl Answer {
    /// Leave a minute of margin for request latency near Telegram's 48-hour limit.
    pub fn deletable(self, now: i64) -> bool {
        self.id > 0
            && self.sent_at > 0
            && now
                .checked_sub(self.sent_at)
                .is_some_and(|age| (0..48 * 60 * 60 - 60).contains(&age))
    }
}

impl History {
    /// Missing, damaged, or oversized metadata fails closed to no cleanup targets.
    pub fn load(path: &Path) -> Self {
        let read = || -> anyhow::Result<Self> {
            anyhow::ensure!(
                std::fs::metadata(path)?.len() <= 1024 * 1024,
                "oversized chat history"
            );
            Ok(serde_json::from_slice(&std::fs::read(path)?)?)
        };
        if !path.exists() {
            return Self::default();
        }
        match read() {
            Ok(history) => history,
            Err(error) => {
                log::warn!("telegram chat history unavailable: {error}");
                Self::default()
            }
        }
    }

    /// Publish message identities atomically so a restart cannot read partial JSON.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        crate::config::write_file_atomic(path, &serde_json::to_vec(self)?, "telegram chat history")
    }
}

#[cfg(test)]
mod tests;
