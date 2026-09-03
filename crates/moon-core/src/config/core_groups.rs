//! Named sets of cores a user saves once and reapplies from the core picker.
//!
//! A group is a name plus core uids — nothing else. It carries no exchange, no ordering intent and
//! no notion of which cores currently exist: it is a bookmark, not a view. Resolving it against
//! whatever a consumer can actually select happens in the UI, at click time.
//!
//! **A group NEVER loses a member because that core is currently absent.** This is the same policy
//! as `layout::WindowLayout::recent_coins`: a core can be deactivated, disconnected, or
//! temporarily removed from the config, and a saved group that quietly forgot it would be
//! impossible to repair — the user would have to remember which core used to be in it. Entries
//! stay on disk and are filtered at READ time. [`sanitize_core_groups`] preserves that boundary by
//! accepting only the saved list, with no configured-core input it could use for pruning.

use serde::{Deserialize, Serialize};

/// Largest number of saved groups kept from one settings file.
///
/// A ceiling on a hand-editable list, not a product limit: the picker renders every group as a
/// menu row above its cores, so an accidental thousand-entry list would bury the cores themselves.
pub const CORE_GROUPS_MAX: usize = 32;

/// Largest group name kept, in characters.
///
/// Characters rather than bytes, so a Cyrillic name is not silently cut to half the length an
/// ASCII one gets. The menu row truncates visually long before this.
pub const CORE_GROUP_NAME_MAX: usize = 48;

/// Largest number of members kept in one group.
///
/// Comfortably past the ~50-core configuration this feature exists for, while still bounding what
/// a corrupted or hand-written file can put in a menu row's count.
pub const CORE_GROUP_MEMBERS_MAX: usize = 512;

/// One saved set of cores, addressed by stable uid.
///
/// Uids, never names or positions: a core keeps its uid across renames and config reordering, and
/// a uid is never reissued after deletion (see [`super::uid_counter::UidCounter`]), so a group can
/// never come to mean a different core than the one it was saved from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreGroup {
    /// User-supplied label, trimmed and non-empty after sanitizing.
    pub name: String,
    /// Member core uids, in the order they were saved, without duplicates.
    // wire-id-exempt: terminal-issued, never a core id — see `config::wire_id`.
    #[serde(default)]
    pub cores: Vec<u64>,
}

/// Bring a saved group list into the shape the rest of the application may assume.
///
/// Applied on the way in from disk and after every runtime mutation, so hand-edited files and UI
/// edits converge on the same invariants:
/// - names are trimmed and capped at [`CORE_GROUP_NAME_MAX`] characters;
/// - a group with no name, or with no members, is dropped — neither can be applied or renamed
///   back into usefulness, and an unnamed row is unclickable in the menu;
/// - members are de-duplicated, keeping first occurrence, so save order is preserved;
/// - a name colliding case-insensitively with an earlier one is RENAMED, never dropped: two rows
///   must not read identically in the picker, but a hand-edited file holding `Scalpers` and
///   `scalpers` must not silently lose one of them on the next launch. Group membership is not
///   recoverable from anywhere else, so the sanitizer never destroys it to enforce a display rule;
/// - the list is capped at [`CORE_GROUPS_MAX`] groups and each group at
///   [`CORE_GROUP_MEMBERS_MAX`] members.
///
/// A member uid naming a core that does not currently exist is KEPT — see the module docstring.
///
/// Args:
///     groups: The saved groups, repaired in place.
///
/// Returns:
///     Whether anything changed, so a caller can mark the config dirty only when it must.
pub fn sanitize_core_groups(groups: &mut Vec<CoreGroup>) -> bool {
    let before = groups.clone();

    let mut seen_names: Vec<String> = Vec::new();
    groups.retain_mut(|group| {
        let trimmed: String = group
            .name
            .trim()
            .chars()
            .take(CORE_GROUP_NAME_MAX)
            .collect();
        // Re-trim: truncation can leave a trailing space that the first trim had inside the name.
        let trimmed = trimmed.trim().to_string();
        if trimmed.is_empty() {
            return false;
        }
        let trimmed = unique_group_name(&seen_names, &trimmed);

        let mut cores = Vec::with_capacity(group.cores.len().min(CORE_GROUP_MEMBERS_MAX));
        for core in &group.cores {
            if cores.len() == CORE_GROUP_MEMBERS_MAX {
                break;
            }
            if !cores.contains(core) {
                cores.push(*core);
            }
        }
        if cores.is_empty() {
            return false;
        }

        seen_names.push(trimmed.to_lowercase());
        group.name = trimmed;
        group.cores = cores;
        true
    });
    groups.truncate(CORE_GROUPS_MAX);

    *groups != before
}

/// Make a proposed group name unique against the ones already in use.
///
/// Comparison is case-insensitive, matching [`sanitize_core_groups`]. A collision gets a ` (2)`,
/// ` (3)` … suffix, and the base is shortened first when the suffix would push the name past
/// [`CORE_GROUP_NAME_MAX`] — the sanitizer truncates from the end and would otherwise cut the
/// suffix straight back off, re-creating the collision it was called to avoid.
///
/// Lives here beside the sanitizer rather than in the UI: it is the constructive half of the same
/// uniqueness rule, and splitting the two across crates is how they drift apart. Deliberately NOT
/// shared with `strategies::tree::ops::unique_name`, which leads with the ordinal (`(2) Base`)
/// because the strategy column truncates from the right; here the trailing form reads better and
/// the length cap is this type's own.
///
/// Args:
///     existing: Names already in use — for a rename, excluding the group being renamed.
///     proposed: The user's text, untrimmed.
///
/// Returns:
///     A trimmed unique name, or an empty string when `proposed` holds no visible characters.
pub fn unique_group_name(existing: &[String], proposed: &str) -> String {
    let base: String = proposed.trim().chars().take(CORE_GROUP_NAME_MAX).collect();
    let base = base.trim().to_string();
    if base.is_empty() {
        return String::new();
    }
    let taken: Vec<String> = existing.iter().map(|name| name.to_lowercase()).collect();
    if !taken.contains(&base.to_lowercase()) {
        return base;
    }
    // Bounded by construction: each candidate is distinct, and the list this runs against is
    // capped at CORE_GROUPS_MAX, so a free ordinal exists well inside the range.
    for n in 2..=CORE_GROUPS_MAX + 2 {
        let suffix = format!(" ({n})");
        let room = CORE_GROUP_NAME_MAX.saturating_sub(suffix.chars().count());
        let stem: String = base.chars().take(room).collect();
        let candidate = format!("{}{suffix}", stem.trim_end());
        if !taken.contains(&candidate.to_lowercase()) {
            return candidate;
        }
    }
    base
}

/// Move one saved group to another position in the list.
///
/// Out-of-range indices and a move onto its own position are refused rather than clamped: both
/// mean the caller's view of the list is stale, and silently moving the group somewhere else would
/// be worse than doing nothing.
///
/// Args:
///     groups: The saved groups, reordered in place.
///     from: Current index.
///     to: Target index.
///
/// Returns:
///     Whether the list changed.
pub fn move_group(groups: &mut [CoreGroup], from: usize, to: usize) -> bool {
    if from == to || from >= groups.len() || to >= groups.len() {
        return false;
    }
    if from < to {
        groups[from..=to].rotate_left(1);
    } else {
        groups[to..=from].rotate_right(1);
    }
    true
}

#[cfg(test)]
mod tests;
