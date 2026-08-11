//! Pure decisions behind the core picker's saved groups: applying one, and describing one.
//!
//! GPUI-free, like [`super::core_quick`], so the six consumers share one definition and every rule
//! can be tested without a window. The group list's own algebra — sanitizing, unique naming,
//! reordering — is NOT here: it belongs to the type, and lives beside it in
//! `moon_core::config::core_groups`. What stays here is everything that needs a CONSUMER's view of
//! the world, which moon-core has no business knowing about.
//!
//! Three core sets meet here and must never be conflated:
//! - the GROUP's members — saved uids retained when their cores leave the configuration;
//! - `selectable` — what THIS consumer renders, which Report and Analytics deliberately build from
//!   the report replica and which therefore includes deleted cores;
//! - `configured` — every uid in `AppConfig.servers`, the authority on whether a member is
//!   currently missing.
//!
//! Applying a group filters against `selectable`; counting its missing members reads `configured`.
//! Swapping the two makes a group look broken in a panel that merely scopes itself narrowly.

use std::collections::HashSet;

use moon_core::config::AppConfig;
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// What a click on a group row does to the current selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GroupClick {
    /// Replace the selection with the group's applicable members.
    Replace,
    /// Add the group's applicable members to the selection.
    Union,
}

impl GroupClick {
    /// Read the intent off the platform's secondary modifier.
    ///
    /// `secondary()` is Ctrl on Windows and Linux, Cmd on macOS — the same multi-select modifier
    /// the Profit Monitor's core table, the tuner's coin list and the strategies tree already use,
    /// so the gesture is learned once.
    ///
    /// Args:
    ///     secondary: Whether the click carried the platform's secondary modifier.
    ///
    /// Returns:
    ///     The intent to apply.
    pub(crate) fn from_secondary(secondary: bool) -> Self {
        if secondary {
            Self::Union
        } else {
            Self::Replace
        }
    }
}

/// Every uid in the configuration — the authority on whether a saved member still exists.
///
/// One definition, because "what counts as configured" is exactly the distinction this module
/// exists to keep straight.
///
/// Args:
///     config: The live configuration.
///
/// Returns:
///     The configured core uids.
pub(crate) fn configured_uids(config: &AppConfig) -> HashSet<u64> {
    config.servers.iter().map(|server| server.uid).collect()
}

/// How many of a group's members this consumer can actually select.
///
/// Counts without collecting: the menu builds this for every group on every repaint and only ever
/// reads the number.
///
/// Args:
///     group: The saved member uids.
///     selectable: Every core this consumer can select.
///
/// Returns:
///     The applicable member count.
pub(crate) fn applicable_count(group: &[u64], selectable: &HashSet<u64>) -> usize {
    group
        .iter()
        .filter(|core| selectable.contains(core))
        .count()
}

/// Whether the current selection IS this group, so its row can show a tick.
///
/// True only for an exact match against the group's applicable members. A superset — what Ctrl+click
/// builds by adding a second group — is NOT this group, and a tick there would claim a scope the
/// user does not have.
///
/// The implicit-All selection falls out as `false` without a branch of its own: it is empty, and an
/// empty selection cannot equal a non-empty applicable set. Do NOT "fix" that by materializing it
/// into the selectable set first, the way [`saved_group_cores`] legitimately does — saving asks
/// "which cores does All mean right now", while this asks "has the user chosen exactly this group",
/// and All is not a choice of any group even when the two sets coincide.
///
/// This is the one place in the picker where a tick is honest on a row that also acts. It says what
/// IS, not what a click would do, and clicking an already-ticked group is inert ([`apply_core_group`]
/// returns `false` on an equal selection), so the tick never contradicts the gesture.
///
/// Args:
///     group: The saved member uids.
///     selectable: Every core this consumer can select.
///     selected: Current explicit selection; empty means all cores.
///
/// Returns:
///     Whether the selection is exactly this group's applicable members.
pub(crate) fn group_is_applied(
    group: &[u64],
    selectable: &HashSet<u64>,
    selected: &HashSet<u64>,
) -> bool {
    let applicable: HashSet<u64> = group
        .iter()
        .copied()
        .filter(|core| selectable.contains(core))
        .collect();
    !applicable.is_empty() && applicable == *selected
}

/// Apply one group click to the selection.
///
/// A group with no applicable member here is INERT: it returns `false` and leaves the selection
/// exactly as it was. It must never clear it — clearing means ALL cores, so a group naming three
/// cores none of which this panel scopes would otherwise BROADEN the query to everything, which is
/// the opposite of what its name promises. Same reasoning as `core_broadcast::next_core_filter`.
///
/// `Union` from the implicit-All state (an empty selection) narrows to the group, because the set
/// is read literally rather than as "all": there is no explicit selection to add to yet. That is
/// the behaviour `core_combo::toggle_exchange_cores` already documents for a first exchange click,
/// so the two gestures agree.
///
/// `Replace` onto an already-equal selection reports `false` and does NOT clear, diverging from
/// `next_core_filter`, which treats a repeat click as a toggle back to All. A group row is not a
/// toggle: its label names a destination, and clicking it twice must not land somewhere else.
///
/// Args:
///     selected: Explicit selection, updated in place; empty means all cores.
///     group: The saved member uids.
///     selectable: Every core this consumer can select.
///     intent: Replace or add.
///
/// Returns:
///     Whether the selection changed and the consumer must reload.
pub(crate) fn apply_core_group(
    selected: &mut HashSet<u64>,
    group: &[u64],
    selectable: &[u64],
    intent: GroupClick,
) -> bool {
    let selectable: HashSet<u64> = selectable.iter().copied().collect();
    let applicable: Vec<u64> = group
        .iter()
        .copied()
        .filter(|core| selectable.contains(core))
        .collect();
    if applicable.is_empty() {
        return false;
    }
    let next: HashSet<u64> = match intent {
        GroupClick::Replace => applicable.into_iter().collect(),
        GroupClick::Union => selected.iter().copied().chain(applicable).collect(),
    };
    if next == *selected {
        return false;
    }
    *selected = next;
    true
}

/// How many of a group's members name a core that is not currently configured.
///
/// Read against the CONFIGURED cores, never against a consumer's rendered list: Orders and Alerts
/// scope themselves to one server group, so a member absent from their list is usually alive and
/// merely out of scope. Only absence from `AppConfig.servers` counts as missing.
///
/// Args:
///     group: The saved member uids.
///     configured: Every uid in `AppConfig.servers`.
///
/// Returns:
///     The number of members with no configured core.
pub(crate) fn group_dead_count(group: &[u64], configured: &HashSet<u64>) -> usize {
    group
        .iter()
        .filter(|core| !configured.contains(core))
        .count()
}

/// The trailing fact for one group row: how many cores, and how many are currently missing.
///
/// One definition for the menu row and the management modal, which would otherwise drift in
/// separator or order while claiming to state the same thing. The core count arrives already
/// localized, because in the menu it must match the formatter its own trigger uses.
///
/// Args:
///     cores_label: The localized core count.
///     dead: How many members name no configured core.
///
/// Returns:
///     The fact, with the missing-member warning appended only when there is one.
pub(crate) fn group_facts(cores_label: String, dead: usize) -> String {
    match dead {
        0 => cores_label,
        dead => format!(
            "{cores_label} · {}",
            t!("common.core_pick.group_dead_n", n = dead)
        ),
    }
}

/// Resolve the current selection into the member list a new group should store.
///
/// The implicit-All selection is materialized here rather than saved as "empty means all": a group
/// is a bookmark of specific cores, and one that meant "whatever exists later" would silently
/// change meaning every time a core is added.
///
/// Members are filtered against CONFIGURED cores, so saving from Report — whose list deliberately
/// includes cores deleted long ago — cannot resurrect them into a new group. Order follows
/// `selectable`, which is already canonical, so two saves of the same set produce the same file.
///
/// Args:
///     selected: Current explicit selection; empty means all cores.
///     selectable: Every core this consumer can select, in canonical order.
///     configured: Every uid in `AppConfig.servers`.
///
/// Returns:
///     The member uids to store, empty when the selection resolves to no live core.
pub(crate) fn saved_group_cores(
    selected: &HashSet<u64>,
    selectable: &[u64],
    configured: &HashSet<u64>,
) -> Vec<u64> {
    selectable
        .iter()
        .copied()
        .filter(|core| saves_core(selected, configured, *core))
        .collect()
}

/// Whether one selectable core belongs in a newly saved group.
///
/// Split out so the Save row's enabled gate can ask the same question with `any` instead of
/// building the whole list on every repaint just to test it for emptiness.
///
/// Args:
///     selected: Current explicit selection; empty means all cores.
///     configured: Every uid in `AppConfig.servers`.
///     core: The candidate.
///
/// Returns:
///     Whether the core would be stored.
pub(crate) fn saves_core(selected: &HashSet<u64>, configured: &HashSet<u64>, core: u64) -> bool {
    (selected.is_empty() || selected.contains(&core)) && configured.contains(&core)
}
