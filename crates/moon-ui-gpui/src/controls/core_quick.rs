//! Pure decisions behind the core picker: its rows and its per-exchange state.
//!
//! Every function here is GPUI-free so the six consumers of [`super::core_combo`] share one
//! definition instead of six near-copies, and so the rules can be tested without a window.
//!
//! An EMPTY selection means ALL cores (see [`super::core_broadcast`]). That single convention is
//! why this module exists. With no filter set the selection is empty, so the first click on a core
//! ISOLATES it rather than removing it, and there is no representable "all fifty explicitly
//! selected" state to take one core out of — which is exactly what a user running fifty cores
//! wants. Saving the implicit-All selection as a core group and applying it materializes that
//! state; see [`super::core_groups::saved_group_cores`].
//!
//! There is deliberately no "deselect all" and no "invert" row: both can produce the empty set,
//! which reads as ALL, so either would sometimes do the precise opposite of its label. Clearing the
//! filter is the All row's job, through [`toggle_core_selection`]'s `None` arm, where the meaning
//! is honest.

use std::collections::HashSet;

#[cfg(test)]
mod tests;

/// How much of one exchange section is explicitly selected, with its member count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GroupCheck {
    /// Every one of the section's members is selected.
    All(usize),
    /// No member is selected, including a section with no members at all.
    None,
    /// `.0` of `.1` members are selected.
    Partial(usize, usize),
}

/// Collect one exchange section's member ids, in the section's own rendered order.
///
/// A named helper rather than an inline `.collect()` at the call site, so "every rendered member,
/// in order" stays a pinned contract instead of drifting silently the next time the exchange-row
/// loop is edited.
///
/// Args:
///     members: One section's rendered core rows, in canonical order.
///
/// Returns:
///     Every member's core id, in the same order.
pub(crate) fn section_core_ids(members: &[(u64, &str)]) -> Vec<u64> {
    members.iter().map(|(core, _)| *core).collect()
}

/// Apply one click on a core row or on the All row.
///
/// `None` is the All row and CLEARS the selection, handing the consumer back its whole scope
/// rather than naming every core in it. `Some(core)` toggles that one core, which from the
/// implicit-All state ISOLATES it. To form an all-but-one selection, the user first materializes
/// the explicit full set by saving the implicit-All selection as a core group and applying it,
/// then removes one core.
///
/// Args:
///     selected: Explicit selection, updated in place; empty means all cores.
///     core: The clicked core, or `None` for the All row.
///
/// Returns:
///     Whether the selection changed and the consumer must reload.
pub(crate) fn toggle_core_selection(selected: &mut HashSet<u64>, core: Option<u64>) -> bool {
    match core {
        None if selected.is_empty() => false,
        None => {
            selected.clear();
            true
        }
        Some(core) => {
            if !selected.remove(&core) {
                selected.insert(core);
            }
            true
        }
    }
}

/// Summarize how much of one exchange section is EXPLICITLY selected.
///
/// A section with no members reports [`GroupCheck::None`] rather than the vacuously true `All`, so
/// an empty or stale group cannot render as completely selected.
///
/// The implicit-All state (an empty selection) reports `None` for every section, deliberately: the
/// core rows beneath the heading render their checkboxes from the same explicit set and are all
/// unticked there, so a heading reading `8/8` above eight empty checkboxes would contradict the
/// rows it summarizes. The heading states what is explicitly selected, exactly like the rows.
///
/// Args:
///     members: Core ids rendered in the section, in member order.
///     selected: Current explicit selection; empty means all cores.
///
/// Returns:
///     The section's selection state.
pub(crate) fn group_check_state(members: &[u64], selected: &HashSet<u64>) -> GroupCheck {
    let total = members.len();
    match members
        .iter()
        .filter(|core| selected.contains(core))
        .count()
    {
        // A section with no members lands here, before the `n == total` arm can call it complete.
        0 => GroupCheck::None,
        n if n == total => GroupCheck::All(total),
        n => GroupCheck::Partial(n, total),
    }
}

/// The trailing state text for one exchange row, or nothing when none of it is selected.
///
/// A partially selected section reads `3/8`, a complete one `8/8`. The row is a LABEL, which never
/// draws a checkbox — an exchange heading is an action, and an action that looks like state is
/// what this picker exists to stop doing.
///
/// Args:
///     state: The section's selection state.
///
/// Returns:
///     The trailing text, or `None` for an unselected or empty section.
pub(crate) fn exchange_state_label(state: GroupCheck) -> Option<String> {
    match state {
        GroupCheck::None => None,
        GroupCheck::All(total) => Some(format!("{total}/{total}")),
        GroupCheck::Partial(on, total) => Some(format!("{on}/{total}")),
    }
}
