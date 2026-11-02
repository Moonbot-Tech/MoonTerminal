//! The user's acceptance of Moonbot's user agreement, persisted as `eula.json`.
//!
//! The connect wizard asks for it once, before it installs the first new core, and again only
//! when the agreement it refers to changes — [`CURRENT`] names that text. A moved core is not
//! asked about: its user accepted the agreement in the Moonbot it moves from.

use serde::{Deserialize, Serialize};

use crate::config::{paths, write_file_atomic};

/// The agreement the wizard's checkbox refers to. Bump it when the text changes, and every user
/// is asked again.
pub const CURRENT: &str = "moonbot-eula-2026-11";
/// Where the wizard's "read" link leads until the text has a page of its own.
pub const READ_URL: &str = "https://moonbot.pro";

/// One acceptance: which text, and when.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EulaAcceptance {
    #[serde(default)]
    pub version: String,
    /// Unix milliseconds of the acceptance.
    #[serde(default)]
    pub accepted_at_ms: i64,
}

impl EulaAcceptance {
    /// The stored acceptance, or `None` when there is none or it cannot be read — which only
    /// makes the wizard ask again.
    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(paths::eula_path()).ok()?;
        serde_json::from_str(&text)
            .map_err(|e| log::warn!("eula.json parse failed ({e}); the agreement is asked again"))
            .ok()
    }

    /// Whether this acceptance covers the current agreement.
    pub fn covers_current(&self) -> bool {
        self.version == CURRENT
    }

    /// Record that the user accepted the current agreement at `now_ms`.
    pub fn accept_current(now_ms: i64) -> anyhow::Result<Self> {
        let accepted = Self {
            version: CURRENT.to_owned(),
            accepted_at_ms: now_ms,
        };
        let text = serde_json::to_string_pretty(&accepted)?;
        write_file_atomic(&paths::eula_path(), text.as_bytes(), "eula.json")?;
        Ok(accepted)
    }
}

#[cfg(test)]
mod tests;
