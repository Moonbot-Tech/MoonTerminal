//! Immutable configuration captured after normalization on its owning thread.

use super::AppConfig;

/// A complete configuration whose UID assignment already reached the live authority.
#[derive(Clone)]
pub struct ConfigSnapshot(pub(super) Box<AppConfig>);

impl ConfigSnapshot {
    /// Write this snapshot through the existing guarded multi-file save in the serial worker.
    pub fn save(&self) -> anyhow::Result<()> {
        self.0.write_prepared()
    }
}

#[cfg(test)]
mod tests;
