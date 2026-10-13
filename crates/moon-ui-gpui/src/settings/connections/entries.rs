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
use moon_core::session::core_order::CoreOrder;

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

/// One line of the flattened group/exchange/core tree the Storage page's core-data list draws.
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
pub(in crate::settings) struct EntryLabels<'a> {
    /// Caption for the pending-cores heading.
    pub(in crate::settings) pending: &'a str,
    /// Builds one exchange section's caption from its venue, or `None` for the unidentified bucket.
    pub(in crate::settings) exchange: &'a dyn Fn(Option<&CoreVenue>) -> String,
}

/// Flatten pending rows, then every group's exchange sections, into one drawable sequence.
///
/// Args:
///     servers: Draft server metadata, as gathered by the Cores page and the Storage core-data tree.
///     groups: Visible groups, already sorted by name.
///     order: Rank table for the active core-sort mode.
///     labels: Localized captions for headings.
///
/// Returns:
///     The flat entry sequence, empty only when there are no servers at all.
pub(in crate::settings) fn flatten_entries(
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
        let sections = moon_core::session::core_order::exchange_sections(
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

/// How a core's status dot reads in the Cores list and card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoreDot {
    /// Connected.
    Ok,
    /// Connecting.
    Wait,
    /// Failed, or its key cannot be read.
    Bad,
    /// Switched off.
    Off,
    /// No live status yet.
    Unknown,
}

impl CoreDot {
    /// The dot for a core from its switch, its key and its live status.
    ///
    /// Args:
    ///     active: The core's draft switch.
    ///     key_readable: The draft key parses.
    ///     status: Latest live status, when the runtime reported one.
    ///
    /// Returns:
    ///     The dot state; a switched-off core reads off whatever else holds.
    pub(crate) fn of(
        active: bool,
        key_readable: bool,
        status: Option<&moon_core::feed::ConnStatus>,
    ) -> Self {
        use moon_core::feed::ConnStatus;
        match status {
            _ if !active => Self::Off,
            _ if !key_readable => Self::Bad,
            Some(ConnStatus::Ready) => Self::Ok,
            Some(ConnStatus::Failed(_)) => Self::Bad,
            Some(ConnStatus::Connecting | ConnStatus::Stage(_)) => Self::Wait,
            Some(ConnStatus::Disconnected) | None => Self::Unknown,
        }
    }
}

/// One core in the Cores list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreListRow {
    /// The row's per-session identity (`ConnRow::row_key`), the selection key.
    pub(crate) row_key: u64,
    /// Position in `preview.servers` and `SettingsView.conn`.
    pub(crate) draft_index: usize,
    pub(crate) core_id: CoreId,
    pub(crate) name: String,
    /// The address in effect: the typed override, else the key's endpoint.
    pub(crate) addr: String,
    /// How many of the nine data kinds the core sends (eight filters plus `log_delivery`).
    pub(crate) feeds_on: u8,
    pub(crate) active: bool,
    pub(crate) dot: CoreDot,
}

/// One exchange inside a group of the Cores list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExBlock {
    /// Already-localized caption.
    pub(crate) caption: String,
    pub(crate) rows: Vec<CoreListRow>,
}

/// One window group of the Cores list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GroupBlock {
    pub(crate) name: String,
    pub(crate) active: bool,
    pub(crate) icon: u32,
    pub(crate) exchanges: Vec<ExBlock>,
}

impl GroupBlock {
    /// Cores in the group.
    pub(crate) fn count(&self) -> usize {
        self.exchanges.iter().map(|e| e.rows.len()).sum()
    }
}

/// What the Cores list draws: groups, then exchanges, then cores, and the selected core.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CoresModel {
    pub(crate) groups: Vec<GroupBlock>,
    /// Row key of the core whose card is shown.
    pub(crate) selected: Option<u64>,
}

impl CoresModel {
    /// Every core in list order.
    pub(crate) fn rows(&self) -> impl Iterator<Item = &CoreListRow> {
        self.groups
            .iter()
            .flat_map(|g| g.exchanges.iter().flat_map(|e| e.rows.iter()))
    }

    /// The selected core's row.
    pub(crate) fn selected_row(&self) -> Option<&CoreListRow> {
        let key = self.selected?;
        self.rows().find(|r| r.row_key == key)
    }
}

/// Arrange core rows into groups and exchanges.
///
/// Every row lands in its group, unsaved rows included: a new core shows where it will live.
///
/// Args:
///     rows: The draft cores to show; each names its own `draft_index`.
///     venues: Each core's identified exchange, by draft index.
///     groups: Visible groups sorted by name, as `(name, active, icon)`.
///     group_of: Each core's group name, by draft index.
///     order: Rank table for the core-sort mode.
///     exchange: Builds one exchange caption, or the unidentified bucket's for `None`.
///     selected: The selection to keep.
///
/// Returns:
///     The list model.
pub(crate) fn cores_model(
    rows: Vec<CoreListRow>,
    venues: &[Option<CoreVenue>],
    groups: &[GroupRowMeta],
    group_of: &[String],
    order: &CoreOrder,
    exchange: &dyn Fn(Option<&CoreVenue>) -> String,
    selected: Option<u64>,
) -> CoresModel {
    let mut out = Vec::new();
    for (name, active, icon) in groups {
        let members: Vec<usize> = (0..rows.len())
            .filter(|&i| group_of.get(rows[i].draft_index).is_some_and(|g| g == name))
            .collect();
        let sections = moon_core::session::core_order::exchange_sections(
            members
                .iter()
                .map(|&i| (i, venues.get(rows[i].draft_index).and_then(Option::as_ref))),
        );
        let exchanges = sections
            .into_iter()
            .map(|(venue, mut members)| {
                order.sort_by(&mut members, |i| rows[*i].core_id);
                ExBlock {
                    caption: exchange(venue),
                    rows: members.into_iter().map(|i| rows[i].clone()).collect(),
                }
            })
            .collect();
        out.push(GroupBlock {
            name: name.clone(),
            active: *active,
            icon: *icon,
            exchanges,
        });
    }
    CoresModel {
        groups: out,
        selected,
    }
}

/// The selection after the core rows were rebuilt (every row gets a new key on a rebuild).
///
/// Args:
///     selected_index: Draft index of the selected core before the rebuild.
///     removed: Draft index of a core deleted by this rebuild.
///     new_keys: Row keys after the rebuild, in draft order.
///
/// Returns:
///     The key of the same core when it survived, of the core that took its place when it was
///     deleted, else of the first core; `None` only with no cores at all.
pub(crate) fn reselect(
    selected_index: Option<usize>,
    removed: Option<usize>,
    new_keys: &[u64],
) -> Option<u64> {
    let index = match (selected_index, removed) {
        (Some(s), Some(r)) if s > r => Some(s - 1),
        (s, _) => s,
    };
    index
        .and_then(|i| new_keys.get(i).or(new_keys.last()))
        .or(new_keys.first())
        .copied()
}

/// The selection while rendering: kept when its key still names a core, else the first core.
pub(crate) fn valid_selection(selected: Option<u64>, keys: &[u64]) -> Option<u64> {
    selected
        .filter(|k| keys.contains(k))
        .or(keys.first().copied())
}
