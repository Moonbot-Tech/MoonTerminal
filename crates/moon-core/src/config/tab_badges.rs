//! Unread counters on dock tabs: what each panel has not been looked at yet, and the per-panel
//! display switches, persisted as `tab_badges.json`.
//!
//! A dock tab shows its panel's unread count only while the panel is HIDDEN behind a sibling tab —
//! once its content is on screen there is nothing to announce. What "unread" means is the panel's
//! business (News watermarks publication time; another panel may watermark a row count), so this
//! file only stores the opaque watermark under a `<panel>/<group>` key and never interprets it.
//!
//! Two display switches live here, both keyed by panel name and both GLOBAL (a panel reads the same
//! in every group/window), mirroring `news_tags.json`:
//! - `hidden` — counters switched off for that panel entirely;
//! - `merged` — counters shown as ONE total instead of split per tag colour (News only, for now).
//!
//! Absent keys mean the default: counters shown, split per colour.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::{paths, write_file_atomic};

/// Persisted dock-tab badge state: per-panel display switches plus the per-panel/group read
/// watermark.
///
/// Missing fields fall back to defaults, so a file written by an older build still loads. Unknown
/// fields do NOT survive: serde drops them on read and the next save writes the file without them,
/// which matters only if an older build is run against a newer file.
///
/// Watermarks are keyed by the group NAME, like the rest of the panel state in this app. Renaming a
/// group therefore starts its counters over rather than carrying them across, and the stale entry
/// is left behind — a few bytes of a small file, against the alternative of resurrecting a whole
/// ring as unread.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TabBadgeSettings {
    /// Panel names whose tab counters the user switched off.
    #[serde(default)]
    hidden: HashSet<String>,
    /// Panel names whose counters are shown as one total instead of split per tag colour.
    #[serde(default)]
    merged: HashSet<String>,
    /// `<panel>/<group>` → the panel's own read watermark. Opaque here; only the panel that wrote
    /// it knows whether it counts milliseconds or rows.
    #[serde(default)]
    seen: HashMap<String, i64>,
    /// Runtime change counter (not persisted). Panels fold it into their repaint signature so a
    /// switch flipped on one tab refreshes every view that reads it.
    #[serde(skip)]
    rev: u64,
}

impl TabBadgeSettings {
    /// Load from `tab_badges.json`, or an empty default when the file is absent or unreadable.
    ///
    /// A parse failure is LOGGED before falling back: resetting silently would show a full ring as
    /// unread with no explanation, and the next save overwrites the file that could explain it.
    pub fn load() -> Self {
        let path = paths::tab_badges_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("tab_badges.json parse failed ({e}); starting from defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Persist atomically; serialize/write failures are non-fatal and only logged.
    pub fn save(&self) {
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) =
                    write_file_atomic(&paths::tab_badges_path(), s.as_bytes(), "tab_badges.json")
                {
                    log::warn!("tab_badges.json save failed: {e}");
                }
            }
            Err(e) => log::warn!("tab_badges.json serialize failed: {e}"),
        }
    }

    /// Runtime change counter for repaint gating; advances on every real mutation.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    // ---- display switches -------------------------------------------------------------------

    /// Whether `panel` shows tab counters at all. Default `true`.
    pub fn counters_visible(&self, panel: &str) -> bool {
        !self.hidden.contains(panel)
    }

    /// Show (`true`) or hide (`false`) `panel`'s tab counters. Returns whether it changed.
    pub fn set_counters_visible(&mut self, panel: &str, visible: bool) -> bool {
        let changed = if visible {
            self.hidden.remove(panel)
        } else {
            self.hidden.insert(panel.to_string())
        };
        self.bump(changed)
    }

    /// Whether `panel`'s counters are merged into a single total. Default `false` (split by colour).
    pub fn counters_merged(&self, panel: &str) -> bool {
        self.merged.contains(panel)
    }

    /// Merge (`true`) or split (`false`) `panel`'s counters. Returns whether it changed.
    pub fn set_counters_merged(&mut self, panel: &str, merged: bool) -> bool {
        let changed = if merged {
            self.merged.insert(panel.to_string())
        } else {
            self.merged.remove(panel)
        };
        self.bump(changed)
    }

    // ---- read watermark ---------------------------------------------------------------------

    /// The read watermark for `panel` in `group`, or `0` when nothing was ever marked read.
    pub fn watermark(&self, panel: &str, group: &str) -> i64 {
        self.seen
            .get(&Self::key(panel, group))
            .copied()
            .unwrap_or(0)
    }

    /// Raise the watermark for `panel` in `group`. Returns whether it changed.
    ///
    /// MONOTONIC on purpose: a reconnect replays the core's ring and a translated copy can arrive
    /// later than the original, so a lower value is a stale reading of the same feed, not a rewind.
    /// Letting it fall would resurrect news the user has already seen.
    pub fn mark_read(&mut self, panel: &str, group: &str, watermark: i64) -> bool {
        let key = Self::key(panel, group);
        let changed = self.seen.get(&key).copied().unwrap_or(0) < watermark;
        if changed {
            self.seen.insert(key, watermark);
        }
        self.bump(changed)
    }

    /// Compose the `<panel>/<group>` storage key.
    fn key(panel: &str, group: &str) -> String {
        format!("{panel}/{group}")
    }

    /// Bump the revision on a real change and pass `changed` through.
    fn bump(&mut self, changed: bool) -> bool {
        if changed {
            self.rev = self.rev.wrapping_add(1);
        }
        changed
    }
}

#[cfg(test)]
mod tests;
