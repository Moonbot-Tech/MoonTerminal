//! Durable ownership of a bot hand-over; every transition is synchronous and atomic.
//!
//! No dirty queue is used: the marker reaches disk before transport suspension or remote work.

use std::path::Path;

use moon_remote::ssh::Target;
use moon_remote::station::bot::BotState;
use serde::{Deserialize, Serialize};

/// Only the destination and ownership phase; credentials and chat IDs never enter this file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) erase_pending: bool,
}

impl Pending {
    /// Capture a destination before the terminal relinquishes polling.
    pub(super) fn new(target: &Target) -> Self {
        Self {
            host: target.host.clone(),
            port: target.port,
            erase_pending: false,
        }
    }

    /// Recover the original destination even if the configured server was forgotten.
    pub(super) fn target(&self) -> Target {
        Target {
            host: self.host.clone(),
            port: self.port,
        }
    }
}

/// The only actions permitted after an authoritative read of the marked station.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Erase,
    Resume,
    Hold,
}

/// A token on a stopped/older/starting station is still ownership, never permission to poll locally.
pub(super) fn decide(pending: &Pending, state: &BotState) -> Decision {
    if state.has_token && (state.stopped || state.no_api) {
        Decision::Hold
    } else if pending.erase_pending || state.polling() {
        Decision::Erase
    } else if !state.has_token && state.bot.is_none() {
        Decision::Resume
    } else {
        Decision::Hold
    }
}

/// Read strictly: only a missing file or an atomic null means no outstanding hand-over.
pub(super) fn load(path: &Path) -> anyhow::Result<Option<Pending>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let pending: Option<Pending> = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid station hand-over journal"))?;
    if let Some(pending) = &pending {
        anyhow::ensure!(
            !pending.host.trim().is_empty() && pending.port != 0,
            "invalid station hand-over destination"
        );
    }
    Ok(pending)
}

/// Publish ownership or a cleared journal before permitting any transport to resume.
pub(super) fn save(path: &Path, pending: Option<&Pending>) -> anyhow::Result<()> {
    moon_core::config::write_file_atomic(path, &serde_json::to_vec(&pending)?, "station hand-over")
}

#[cfg(test)]
mod tests;
