//! Retained history of per-core update attempts, persisted as `core_updates.json`.
//!
//! `SessionManager` is the single capped authority for the live history (see
//! `session::core_update`); this file only BORROWS a snapshot of it to persist and reload one
//! across a restart. It never owns or drains the queue itself.

use serde::{Deserialize, Serialize};

use crate::config::{paths, write_file_atomic};
use crate::session::core_update::CoreUpdateRecord;

/// Persisted snapshot of the update-history ring, in append order.
///
/// `#[serde(default)]` on every field, mirroring `tab_badges::TabBadgeSettings`: a `serde` field
/// without it turns one older file into a hard parse failure and silently resets the whole log
/// (named as a Phase 3 breakage risk in the plan review).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoreUpdateHistory {
    #[serde(default)]
    pub records: Vec<CoreUpdateRecord>,
}

impl CoreUpdateHistory {
    /// Load from `core_updates.json`, or an empty default when the file is absent or unreadable.
    ///
    /// A parse failure is LOGGED before falling back, matching `TabBadgeSettings::load`: resetting
    /// silently would drop the whole retained log with no explanation, and the next save would
    /// overwrite the file that could have explained it.
    pub fn load() -> Self {
        let path = paths::core_updates_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("core_updates.json parse failed ({e}); starting from defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Persist atomically; serialize/write failures are non-fatal and only logged.
    pub fn save(&self) -> anyhow::Result<()> {
        let s = serde_json::to_string_pretty(self)?;
        write_file_atomic(
            &paths::core_updates_path(),
            s.as_bytes(),
            "core_updates.json",
        )
    }
}
