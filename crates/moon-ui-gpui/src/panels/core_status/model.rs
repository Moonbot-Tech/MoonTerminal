//! Pure attention-first server aggregation for the Core Status panel.
//!
//! Cores sharing an IP address represent processes on one machine even when their MoonProto
//! ports differ. Unknown endpoints stay separate so unrelated cores are never merged. Non-Ready
//! processes and non-Online servers lead their respective stable display partitions.

use std::collections::HashMap;
use std::net::IpAddr;

use moon_core::feed::{ConnStatus, CoreEndpoint};
use moon_core::session::{CoreId, CoreSysStatus};

/// Cached display data for one core before server aggregation.
#[derive(Clone)]
pub(super) struct CoreStatusRow {
    /// Stable core identity used by tree rows and visibility filters.
    pub(super) id: CoreId,
    /// Configured core display name.
    pub(super) name: String,
    /// Latest connection lifecycle state.
    pub(super) status: ConnStatus,
    /// Latest MoonProto process and machine telemetry.
    pub(super) sys: CoreSysStatus,
    /// Endpoint decoded by the feed without exposing the exported key.
    pub(super) endpoint: Option<CoreEndpoint>,
}

/// Stable grouping identity for a known host or one isolated unknown core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ServerKey {
    /// All cores whose configured endpoints share this address.
    Address(IpAddr),
    /// One core whose endpoint has not reached the store yet.
    Unknown(CoreId),
}

impl ServerKey {
    /// Build the grouping identity for one core.
    ///
    /// Args:
    ///     row: Core snapshot whose endpoint determines the server.
    ///
    /// Returns:
    ///     An address-only key, or a core-specific fallback when the endpoint is unknown.
    pub(super) fn for_row(row: &CoreStatusRow) -> Self {
        row.endpoint
            .map(|endpoint| Self::Address(endpoint.address))
            .unwrap_or(Self::Unknown(row.id))
    }

    /// Return a persistence-safe tree identifier.
    ///
    /// Args:
    ///     self: Server identity to encode.
    ///
    /// Returns:
    ///     A stable identifier that does not contain credentials.
    pub(super) fn tree_id(self) -> String {
        match self {
            Self::Address(address) => format!("server:{address}"),
            Self::Unknown(core) => format!("server:unknown:{core}"),
        }
    }
}

/// Coarse server connectivity derived from all processes on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerConnectivity {
    /// Every process in the visible server group is ready.
    Online,
    /// At least one process is ready or still connecting, but the group is not fully ready.
    Degraded,
    /// No process is ready or connecting.
    Offline,
}

/// Aggregated display snapshot for one server and its ordered core children.
#[derive(Clone)]
pub(super) struct ServerStatusGroup {
    /// Stable identity used by visibility and expansion state.
    pub(super) key: ServerKey,
    /// Display name: the custom name or the default `Server N` ordinal, filled by the panel after
    /// aggregation because naming needs config and cross-group ordinal ranking.
    pub(super) display_name: String,
    /// Sustained-CPU warning (machine held high), filled by the panel from cross-tick history.
    pub(super) cpu_warn: bool,
    /// Memory-growth warning (a core's used memory rising), filled by the panel from history.
    pub(super) mem_warn: bool,
    /// Connectivity warning: a core dropped (Disconnected/Failed) while the server still has a ready
    /// core — "one fell off while the rest works". Filled by the panel from the backend engine.
    pub(super) conn_warn: bool,
    /// Shared endpoint address, or `None` for an isolated unknown endpoint.
    pub(super) address: Option<IpAddr>,
    /// Cores ordered attention-first, retaining canonical input order within each partition.
    pub(super) cores: Vec<CoreStatusRow>,
    /// Number of ready cores.
    pub(super) ready_count: usize,
    /// Coarse group connectivity.
    pub(super) connectivity: ServerConnectivity,
    /// Freshest available whole-machine CPU sample.
    pub(super) system_cpu_percent: Option<u8>,
    /// Sum of the latest process-memory samples, widened beyond MoonProto's per-process `u16`.
    pub(super) process_memory_mb: Option<u64>,
    /// Freshest available whole-machine free-memory sample.
    pub(super) free_physical_memory_mb: Option<u16>,
    /// Freshest available logical-CPU count.
    pub(super) logical_cpu_count: Option<u8>,
}

/// Group canonically ordered core rows into attention-first address-only server snapshots.
///
/// Machine-wide fields select the newest core sample containing that field; process memory is
/// summed because each core represents a separate MoonBot process.
///
/// Args:
///     rows: Ordered core snapshots in the current panel scope.
///
/// Returns:
///     Attention servers before Online servers, with stable group and core order inside each
///     partition.
pub(super) fn aggregate_servers(rows: &[CoreStatusRow]) -> Vec<ServerStatusGroup> {
    let mut groups = Vec::<ServerStatusGroup>::new();
    let mut positions = HashMap::<ServerKey, usize>::new();

    for row in rows {
        let key = ServerKey::for_row(row);
        let position = *positions.entry(key).or_insert_with(|| {
            let position = groups.len();
            groups.push(ServerStatusGroup {
                key,
                display_name: String::new(),
                cpu_warn: false,
                mem_warn: false,
                conn_warn: false,
                address: match key {
                    ServerKey::Address(address) => Some(address),
                    ServerKey::Unknown(_) => None,
                },
                cores: Vec::new(),
                ready_count: 0,
                connectivity: ServerConnectivity::Offline,
                system_cpu_percent: None,
                process_memory_mb: None,
                free_physical_memory_mb: None,
                logical_cpu_count: None,
            });
            position
        });
        groups[position].cores.push(row.clone());
    }

    for group in &mut groups {
        finish_group(group);
        group.cores.sort_by(|a, b| a.name.cmp(&b.name));
    }
    // Order servers by name: address servers by IP (which matches the `Server N` ordinal), then
    // unknown-endpoint servers last.
    groups.sort_by_key(|group| (group.address.is_none(), group.address));
    groups
}

/// Order flat-mode rows attention-first.
///
/// Args:
///     rows: Current filtered core snapshots.
///
/// Returns:
///     Attention rows before Ready rows, retaining canonical order inside each partition.
pub(super) fn ordered_flat_rows(rows: &[CoreStatusRow]) -> Vec<CoreStatusRow> {
    let mut visible = rows.to_vec();
    visible.sort_by_key(|row| row.status == ConnStatus::Ready);
    visible
}

/// Derive summary fields after all ordered children have entered one group.
///
/// Args:
///     group: Partially built group whose `cores` collection is complete.
///
/// Returns:
///     Nothing; summary fields are updated in place.
fn finish_group(group: &mut ServerStatusGroup) {
    group.ready_count = group
        .cores
        .iter()
        .filter(|row| row.status == ConnStatus::Ready)
        .count();
    group.connectivity = connectivity(&group.cores, group.ready_count);
    group.system_cpu_percent =
        freshest_metric(&group.cores, |sys| sys.system_cpu_percent).map(|(_, value)| value);
    group.free_physical_memory_mb =
        freshest_metric(&group.cores, |sys| sys.free_physical_memory_mb).map(|(_, value)| value);
    group.logical_cpu_count =
        freshest_metric(&group.cores, |sys| sys.logical_cpu_count).map(|(_, value)| value);

    let mut has_process_memory = false;
    let mut process_memory_mb = 0u64;
    for value in group.cores.iter().filter_map(|row| row.sys.used_memory_mb) {
        has_process_memory = true;
        process_memory_mb += u64::from(value);
    }
    group.process_memory_mb = has_process_memory.then_some(process_memory_mb);
}

/// Select the newest available value for one machine-wide metric.
///
/// Args:
///     cores: Core snapshots from one server.
///     read: Accessor for one optional metric.
///
/// Returns:
///     The source timestamp and value, or `None` when no core has the metric.
fn freshest_metric<T: Copy>(
    cores: &[CoreStatusRow],
    read: impl Fn(&CoreSysStatus) -> Option<T>,
) -> Option<(i64, T)> {
    cores
        .iter()
        .filter_map(|row| read(&row.sys).map(|value| (row.sys.updated_ms, value)))
        .max_by_key(|(updated_ms, _)| *updated_ms)
}

/// Reduce per-core connection states into the server badge state.
///
/// Args:
///     cores: All core snapshots in the group.
///     ready_count: Precomputed count of ready cores.
///
/// Returns:
///     Online, degraded, or offline connectivity.
fn connectivity(cores: &[CoreStatusRow], ready_count: usize) -> ServerConnectivity {
    if ready_count == cores.len() && !cores.is_empty() {
        ServerConnectivity::Online
    } else if ready_count > 0
        || cores
            .iter()
            .any(|row| matches!(row.status, ConnStatus::Connecting | ConnStatus::Stage(_)))
    {
        ServerConnectivity::Degraded
    } else {
        ServerConnectivity::Offline
    }
}

#[cfg(test)]
mod tests;
