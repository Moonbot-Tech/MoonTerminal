//! App-local custom color history, independent of Settings drafts and portable badge settings.

use std::{collections::HashSet, io::ErrorKind, path::Path};

use serde::{Deserialize, Serialize};

use super::{badges::BadgesConfig, paths, write_file_atomic};

/// Maximum remembered custom colours. Matches the MoonUI colour-picker widget's own
/// `MAX_CUSTOM_COLORS`; a larger stored/seeded list here would just be silently trimmed the next
/// time a picker seeds itself from it.
const CUSTOM_COLORS_MAX: usize = 20;

/// Shared most-recent-first HEX history persisted separately from editable settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomColors {
    /// RGB swatches in most-recent-first order.
    pub custom_colors: Vec<[u8; 3]>,
}

impl CustomColors {
    /// Load the profile history, importing legacy badges only when the new file is absent.
    /// Returns read, parse, or migration-save errors without overwriting unreadable data.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&paths::custom_colors_path(), &paths::badges_path())
    }

    /// Load explicit history and legacy paths for isolated profiles and fixtures.
    /// An existing empty history wins; only NotFound permits a read-only legacy import.
    /// Returns read, parse, or migration-save errors without replacing existing files.
    pub fn load_from(path: &Path, legacy_path: &Path) -> anyhow::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut history: Self = serde_json::from_slice(&bytes)?;
                history.normalize();
                Ok(history)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let mut history = match std::fs::read(legacy_path) {
                    Ok(bytes) => Self {
                        // A corrupt legacy file is already-superseded data, so it must not
                        // disable the new history: `BadgesConfig::load` falls back to the
                        // default for exactly this case, and propagating here would leave
                        // persistence off on every later launch too, since the new file is
                        // never created on the error path.
                        custom_colors: match serde_json::from_slice::<BadgesConfig>(&bytes) {
                            Ok(config) => config.custom_colors,
                            Err(error) => {
                                log::warn!(
                                    "badges.json is unreadable ({error}) -- importing an empty colour history"
                                );
                                Vec::new()
                            }
                        },
                    },
                    Err(error) if error.kind() == ErrorKind::NotFound => Self::default(),
                    Err(error) => return Err(error.into()),
                };
                history.normalize();
                history.save_to(path)?;
                Ok(history)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Preserve the first occurrence of each loaded color and discard entries past the cap.
    fn normalize(&mut self) {
        let mut seen = HashSet::new();
        self.custom_colors.retain(|color| seen.insert(*color));
        self.custom_colors.truncate(CUSTOM_COLORS_MAX);
    }

    /// Atomically save the profile history, returning serialization and filesystem errors.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&paths::custom_colors_path())
    }

    /// Atomically save to an explicit path, returning serialization and filesystem errors.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        write_file_atomic(path, &bytes, "custom_colors.json")
    }

    /// Remember a colour typed into a colour picker's hex field, most-recent-first, capped
    /// at `CUSTOM_COLORS_MAX` (dropping the oldest). Mirrors the MoonUI widget's own dedupe/cap so
    /// the persisted list and a freshly-seeded picker never disagree on order.
    ///
    /// Args:
    ///     color: RGB value the user committed through a picker hex field.
    ///
    /// Returns:
    ///     Whether the list actually changed — `false` when `color` was already the front entry.
    pub fn remember_custom_color(&mut self, color: [u8; 3]) -> bool {
        if self.custom_colors.first() == Some(&color) {
            return false;
        }
        self.custom_colors.retain(|c| *c != color);
        self.custom_colors.insert(0, color);
        self.custom_colors.truncate(CUSTOM_COLORS_MAX);
        true
    }
}

#[cfg(test)]
mod tests;
