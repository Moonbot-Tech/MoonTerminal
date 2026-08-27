//! Flattened row model for the virtualized core list: a pure pass from draft server/group
//! metadata to the sequence `MoonVirtualList` draws, modelled on
//! `analytics/profit_monitor/sections.rs`. Nothing here touches `App`, `Window`, or `t!` -- captions
//! that need localization are resolved by the caller and passed in through [`EntryLabels`], the same
//! seam `analytics/profit_monitor/sections.rs::SectionLabels` uses, so this module stays testable
//! without a locale.

use std::collections::HashMap;

use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

use super::tab::{GroupRowMeta, ServerRowMeta, member_counts, pending_server_indices};
use crate::core_order::CoreOrder;

/// Partition saved draft rows by group name in ONE pass.
///
/// The group loop below needs each group's own member indices. Asking for them per group means
/// rescanning every server once per group -- O(groups x servers) on a page that is rebuilt on every
/// wheel notch, which is the same quadratic shape `member_counts` exists to avoid. Pending rows
/// (uid 0) are excluded here because they render in their own section above every group.
///
/// Args:
///     servers: Every current draft server row.
///
/// Returns:
///     Persisted draft indices grouped by their draft group name.
fn saved_indices_by_group(servers: &[ServerRowMeta]) -> HashMap<&str, Vec<usize>> {
    let mut by_group: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, (_, uid, _, group, _)) in servers.iter().enumerate() {
        if *uid != 0 {
            by_group.entry(group.as_str()).or_default().push(index);
        }
    }
    by_group
}

/// One line of the virtualized Connections list.
///
/// `pub(crate)` rather than `pub(super)`: `SettingsView` caches the flattened sequence
/// (`SettingsView::conn_entries`) for the eviction and focus-scroll handlers in
/// `connections/mod.rs`, which need to name the type from `settings::mod`, one level above
/// `connections` itself.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ConnEntry {
    /// Heading above unsaved cores, shown while at least one exists.
    PendingHeader {
        /// Already-localized caption.
        caption: String,
        /// How many pending rows follow.
        member_count: usize,
    },
    /// A window-group branch heading.
    GroupHeader {
        /// The group's name, also its identity for lookups such as the icon picker.
        name: String,
        active: bool,
        icon: u32,
        member_count: usize,
    },
    /// An exchange subsection heading inside one group branch.
    ExchangeHeader {
        /// Position of the owning group among drawn groups, for a stable element id.
        group_index: usize,
        /// Position of this exchange within its group, for a stable element id.
        exchange_index: usize,
        /// Already-localized caption.
        caption: String,
        member_count: usize,
        /// Whether the venue was identified, driving the highlight and dot color.
        identified: bool,
    },
    /// One editable core row.
    CoreRow {
        /// Position in `preview.servers` and `SettingsView.conn` -- mutation and state lookup stay
        /// positional; see the module-level reasoning on why identity is uid/row-key instead.
        draft_index: usize,
        core_id: CoreId,
        /// Draft `ServerConfig.uid`; zero for an unsaved row.
        uid: u64,
        active: bool,
        /// Every core row sits under a pending or group/exchange heading today, so this is always
        /// `true` -- kept as a field rather than hard-coded so the factory needs no special case if
        /// a future entry kind ever draws one flush with its heading.
        indented: bool,
    },
}

/// Localized captions the pure pass cannot produce on its own.
pub(super) struct EntryLabels<'a> {
    /// Caption for the pending-cores heading.
    pub(super) pending: &'a str,
    /// Builds one exchange section's caption from its venue, or `None` for the unidentified bucket.
    pub(super) exchange: &'a dyn Fn(Option<&CoreVenue>) -> String,
}

/// Flatten pending rows, then every group's exchange sections, into one drawable sequence.
///
/// Args:
///     servers: Draft server metadata, as gathered by `connections_tab`.
///     groups: Visible groups, already sorted by name.
///     order: Rank table for the active core-sort mode.
///     labels: Localized captions for headings.
///
/// Returns:
///     The flat entry sequence, empty only when there are no servers at all.
pub(super) fn flatten_entries(
    servers: &[ServerRowMeta],
    groups: &[GroupRowMeta],
    order: &CoreOrder,
    labels: EntryLabels<'_>,
) -> Vec<ConnEntry> {
    let mut entries = Vec::new();

    let mut pending = pending_server_indices(servers);
    order.sort_by(&mut pending, |index| servers[*index].0);
    if !pending.is_empty() {
        entries.push(ConnEntry::PendingHeader {
            caption: labels.pending.to_string(),
            member_count: pending.len(),
        });
        entries.extend(pending.into_iter().map(|i| core_row_entry(servers, i)));
    }

    let counts = member_counts(servers);
    let by_group = saved_indices_by_group(servers);
    for (group_index, (name, active, icon)) in groups.iter().enumerate() {
        let member_count = counts.get(name.as_str()).copied().unwrap_or_default();
        entries.push(ConnEntry::GroupHeader {
            name: name.clone(),
            active: *active,
            icon: *icon,
            member_count,
        });
        let group_members = by_group
            .get(name.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let sections = crate::core_order::exchange_sections(
            group_members.iter().map(|&i| (i, servers[i].4.as_ref())),
        );
        for (exchange_index, (venue, mut members)) in sections.into_iter().enumerate() {
            let identified = venue.is_some();
            let caption = (labels.exchange)(venue);
            order.sort_by(&mut members, |index| servers[*index].0);
            entries.push(ConnEntry::ExchangeHeader {
                group_index,
                exchange_index,
                caption,
                member_count: members.len(),
                identified,
            });
            entries.extend(members.into_iter().map(|i| core_row_entry(servers, i)));
        }
    }

    entries
}

/// Build one `CoreRow` entry from a draft server index.
///
/// Args:
///     servers: Every current draft server row.
///     i: Index of the row to represent.
///
/// Returns:
///     A core entry preserving the source index for editor-state lookup.
fn core_row_entry(servers: &[ServerRowMeta], i: usize) -> ConnEntry {
    let (id, uid, active, _, _) = &servers[i];
    ConnEntry::CoreRow {
        draft_index: i,
        core_id: *id,
        uid: *uid,
        active: *active,
        indented: true,
    }
}
