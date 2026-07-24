//! Local per-tag colours AND the tag-visibility filter for the News panel, persisted as
//! `news_tags.json`.
//!
//! Both the colours and the filter are GLOBAL (a tag reads and filters the same in every group/
//! window) and saved immediately on change, mirroring `detects_view.toml`. Colours are stored as a
//! small palette KEY (e.g. `"red"`), not a raw RGB, so the rendered colour follows the active theme;
//! absent/empty keys render neutral. The hidden set stores case-folded tag keys (a tagged card is
//! hidden only when ALL its tags are hidden); `hide_untagged` hides news carrying no tags at all.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::{paths, write_file_atomic};

/// Persisted News-panel tag settings: colours + visibility filter. Unknown colour keys resolve to
/// neutral, so the file stays forward-compatible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewsTagSettings {
    /// Tag key (case-folded) → palette colour key (`red`/`amber`/`green`/`blue`).
    #[serde(default)]
    colors: HashMap<String, String>,
    /// Case-folded tag keys the user hid.
    #[serde(default)]
    hidden: HashSet<String>,
    /// Whether news carrying no tags at all are hidden.
    #[serde(default)]
    hide_untagged: bool,
    /// Runtime change counter (not persisted). Panels fold it into their repaint signature so every
    /// open News view refreshes when a colour or the filter changes, not just the one that edited it.
    #[serde(skip)]
    rev: u64,
}

impl NewsTagSettings {
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

    /// Runtime change counter for repaint gating; advances on every real mutation.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    // ---- colours ------------------------------------------------------------------------------

    /// The colour key assigned to `key`, or `None` when neutral.
    pub fn color(&self, key: &str) -> Option<&str> {
        self.colors
            .get(key)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Assign (`Some`) or clear (`None`) a tag's colour. Returns whether the map actually changed, so
    /// the caller can skip an unnecessary save + repaint. Bumps [`Self::rev`] on a real change.
    pub fn set_color(&mut self, key: &str, palette_key: Option<&str>) -> bool {
        let changed = match palette_key {
            Some(k) if !k.is_empty() => {
                if self.colors.get(key).map(String::as_str) == Some(k) {
                    false
                } else {
                    self.colors.insert(key.to_string(), k.to_string());
                    true
                }
            }
            _ => self.colors.remove(key).is_some(),
        };
        self.bump(changed)
    }

    // ---- visibility filter --------------------------------------------------------------------

    /// Whether tag `key` is hidden.
    pub fn is_hidden(&self, key: &str) -> bool {
        self.hidden.contains(key)
    }

    /// Hide (`true`) or show (`false`) a single tag. Returns whether it changed.
    pub fn set_hidden(&mut self, key: &str, hidden: bool) -> bool {
        let changed = if hidden {
            self.hidden.insert(key.to_string())
        } else {
            self.hidden.remove(key)
        };
        self.bump(changed)
    }

    /// Hide every tag in `keys` (`true`) or clear the whole hidden set (`false`). Returns whether it
    /// changed.
    pub fn set_all_hidden(&mut self, keys: impl IntoIterator<Item = String>, hidden: bool) -> bool {
        let changed = if hidden {
            let before = self.hidden.len();
            self.hidden.extend(keys);
            self.hidden.len() != before
        } else {
            let had = !self.hidden.is_empty();
            self.hidden.clear();
            had
        };
        self.bump(changed)
    }

    /// Whether tagless news are hidden.
    pub fn hide_untagged(&self) -> bool {
        self.hide_untagged
    }

    /// Whether any visibility filter is engaged (some tag hidden, or tagless news hidden) — used to
    /// brighten the Tags trigger.
    pub fn any_filter(&self) -> bool {
        !self.hidden.is_empty() || self.hide_untagged
    }

    /// Set whether tagless news are hidden. Returns whether it changed.
    pub fn set_hide_untagged(&mut self, hide: bool) -> bool {
        let changed = self.hide_untagged != hide;
        self.hide_untagged = hide;
        self.bump(changed)
    }

    /// Bump the revision on a real change and pass `changed` through.
    fn bump(&mut self, changed: bool) -> bool {
        if changed {
            self.rev = self.rev.wrapping_add(1);
        }
        changed
    }
}
