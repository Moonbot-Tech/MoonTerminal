//! Local per-tag colours for the News panel, persisted as `news_tags.json`.
//!
//! A user assigns a colour to a news tag by name. The assignment is GLOBAL (a tag reads the same in
//! every group/window) and stored as a small palette KEY (e.g. `"red"`), not a raw RGB, so the
//! rendered colour follows the active theme. Absent/empty keys render neutral. The file is tiny and
//! saved immediately on change, mirroring `detects_view.toml`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::{paths, write_file_atomic};

/// Tag name → palette colour key. Keys are the UI's fixed palette (`red`/`amber`/`green`/`blue`/
/// `teal`); an unknown key resolves to neutral, so the file stays forward-compatible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewsTagColors {
    #[serde(default)]
    colors: HashMap<String, String>,
    /// Runtime change counter (not persisted). Panels fold it into their repaint signature so every
    /// open News view refreshes when a colour changes, not just the one that made the edit.
    #[serde(skip)]
    rev: u64,
}

impl NewsTagColors {
    /// Load from `news_tags.json`, or an empty default on any read/parse failure.
    pub fn load() -> Self {
        match std::fs::read_to_string(paths::news_tags_path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist atomically; serialize/write failures are non-fatal and only logged.
    pub fn save(&self) {
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) =
                    write_file_atomic(&paths::news_tags_path(), s.as_bytes(), "news_tags.json")
                {
                    log::warn!("news_tags.json save failed: {e}");
                }
            }
            Err(e) => log::warn!("news_tags.json serialize failed: {e}"),
        }
    }

    /// The colour key assigned to `tag`, or `None` when neutral.
    pub fn color(&self, tag: &str) -> Option<&str> {
        self.colors
            .get(tag)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Runtime change counter for repaint gating; advances on every real [`Self::set`] change.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Assign (`Some`) or clear (`None`) a tag's colour. Returns whether the map actually changed, so
    /// the caller can skip an unnecessary save + repaint. Bumps [`Self::rev`] on a real change.
    pub fn set(&mut self, tag: &str, key: Option<&str>) -> bool {
        let changed = match key {
            Some(k) if !k.is_empty() => {
                if self.colors.get(tag).map(String::as_str) == Some(k) {
                    false
                } else {
                    self.colors.insert(tag.to_string(), k.to_string());
                    true
                }
            }
            _ => self.colors.remove(tag).is_some(),
        };
        if changed {
            self.rev = self.rev.wrapping_add(1);
        }
        changed
    }
}
