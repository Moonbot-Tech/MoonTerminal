//! Pure attention-first server aggregation for the Core Status panel.
//!
//! Cores sharing an IP address represent processes on one machine even when their MoonProto
//! ports differ. Unknown endpoints stay separate so unrelated cores are never merged. Non-Ready
//! processes and non-Online servers lead their respective stable display partitions.

use std::collections::HashMap;
use std::net::IpAddr;

use moon_core::feed::{ConnFault, ConnStatus, CoreEndpoint};
use moon_core::session::{ApiKeyExpiry, CoreId, CoreStartupStatus, CoreSysStatus};

use super::startup::{StartupCell, startup_cell};

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
    /// Latest polled startup telemetry. It FREEZES once the core settles, so after a successful
    /// startup it describes how long that core took to come up rather than a running clock.
    pub(super) startup: CoreStartupStatus,
    /// Why this core's last connection attempt ended, when one has ended.
    ///
    /// Retained across the backoff retry, so a row that is connecting again still explains WHY the
    /// previous attempt failed instead of falling back to a bare progress figure.
    pub(super) fault: Option<ConnFault>,
    /// Endpoint decoded by the feed without exposing the exported key.
    pub(super) endpoint: Option<CoreEndpoint>,
    /// Whether this specific core has a sustained above-baseline client↔core ping (the per-core ping
    /// warning), so its own row can show the cause the server-level badge only hints at.
    pub(super) ping_warn: bool,
    /// Whether this core has a sustained above-baseline core→exchange ping (the exch-ping warning).
    pub(super) exch_warn: bool,
    /// This core's current client↔core-ping colour severity (relative to its own baseline and the
    /// axis thresholds), computed by the engine so colour and warning always agree.
    pub(super) ping_sev: crate::backend::core_warn::LatencySeverity,
    /// This core's current core→exchange-ping colour severity.
    pub(super) exch_sev: crate::backend::core_warn::LatencySeverity,
    /// This core's exchange API key, already classified against the current clock.
    pub(super) api_key: ApiKeyState,
    /// Whether this core's key is inside the configured warning horizon, decided by the engine so
    /// the cell mark and the episode agree.
    pub(super) api_warn: bool,
}

/// What is known about one core's exchange API key, as of a given moment.
///
/// A single classification instead of an `Option<ApiKeyExpiry>` read differently at each call site:
/// the two states without a number mean opposite things (the check produced nothing vs the key has
/// nothing to expire), and every consumer that told them apart on its own — the text, the colour,
/// the sort, the server-row aggregate — would be one edit away from conflating them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApiKeyState {
    /// The check produced nothing to show: never asked, the core is down, the check failed, or the
    /// answer carried a date this terminal could not use.
    Unknown,
    /// No expiration at all, or a lifetime long enough to read as none — see
    /// [`API_PERPETUAL_DAYS`].
    Perpetual,
    /// Whole days remaining: zero means less than a day, negative means already expired. One
    /// variant rather than a separate `Expired`, so "how long ago" survives — two keys dead by a
    /// day and by a year must not compare equal in the column that ranks urgency.
    Days(i32),
}

/// A remaining lifetime of at least this many days reads as unlimited rather than as a number.
///
/// Two reasons. A count beyond a year is not information an operator acts on — the column exists
/// to catch a key about to die. And a round `1000` is what two Bybit cores answer here (observed
/// live, with `known == true`): too round to be a real date, most likely a core-side stand-in, and
/// rendering it as "1000" would present that constant as a measurement.
const API_PERPETUAL_DAYS: i32 = 365;

impl ApiKeyState {
    /// Classify one core's stored answer against the current clock.
    ///
    /// An answer the core marked as having no expiration is `Perpetual` — the key is unlimited.
    /// That fact is decided at the wire boundary ([`ApiKeyExpiry::unlimited`]), not inferred here
    /// from a missing date: the same answer can carry a real day count with no date beside it.
    ///
    /// A core that cannot check its exchange answers `success = false`, which never becomes an
    /// `ApiKeyExpiry` at all — so it reaches this function as `None` while it has never answered,
    /// and as its LAST successful answer once it has (the store keeps that until the connection is
    /// replaced).
    ///
    /// Args:
    ///     expiry: The retained answer, or `None` when this core has never answered.
    ///     now_ms: Current Unix milliseconds.
    ///
    /// Returns:
    ///     The state to display, sort and colour by.
    pub(super) fn of(expiry: Option<ApiKeyExpiry>, now_ms: i64) -> Self {
        let Some(expiry) = expiry else {
            return Self::Unknown;
        };
        if expiry.unlimited {
            return Self::Perpetual;
        }
        match expiry.days_left_at(now_ms) {
            Some(days) if days >= API_PERPETUAL_DAYS => Self::Perpetual,
            Some(days) => Self::Days(days),
            // Dated, but the date is unusable — a legacy answer outside the plausible range. Nothing
            // honest to show and nothing to warn on.
            None => Self::Unknown,
        }
    }

    /// Days remaining when there is a number.
    pub(super) fn days(self) -> Option<i32> {
        match self {
            Self::Days(days) => Some(days),
            Self::Unknown | Self::Perpetual => None,
        }
    }

    /// Sort key ordering states by URGENCY, most urgent first.
    ///
    /// A dated key leads, soonest (and expired, being negative) first; then the keys nothing is
    /// known about, which may still hide a problem; then the effectively-unlimited ones, which
    /// cannot. Sorting on [`Self::days`] instead would put every dash AND every infinity ahead of
    /// the counts — `None` sorts first — which is the opposite of what the column is scanned for.
    pub(super) fn urgency(self) -> (u8, i32) {
        match self {
            Self::Days(days) => (0, days),
            Self::Unknown => (1, 0),
            Self::Perpetual => (2, 0),
        }
    }

    /// Whether the key is already past its date.
    pub(super) fn is_expired(self) -> bool {
        self.days().is_some_and(|days| days < 0)
    }
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
    /// Ping warning: any core on this server has a sustained above-baseline client↔core round-trip.
    /// Aggregated from the backend engine (ping is per core; this lights the server's attention state).
    pub(super) ping_warn: bool,
    /// Exch-ping warning: any core on this server has a sustained above-baseline core→exchange ping.
    pub(super) exch_warn: bool,
    /// API-key warning: any core on this server has a key inside the warning horizon. The key
    /// belongs to the core, so the server row only carries the attention state, like ping.
    pub(super) api_warn: bool,
    /// The most urgent key among this server's cores — the one the server row shows and sorts by.
    /// A real day count wins whenever any core has one; `Unknown` when none does and at least one
    /// core is unaccounted for; `Perpetual` only when every core is unlimited.
    pub(super) api_key: ApiKeyState,
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
    /// What this server's startup column shows, rolled up from its cores — see [`group_startup`].
    /// A collapsed group must still tell the truth, so this is not merely the first core's value.
    pub(super) startup: Option<StartupCell>,
}

impl ServerStatusGroup {
    /// Whether any axis is currently warning on this server — the attention pin and the tab badge.
    ///
    /// One place, so a new axis cannot light the badge but miss the sort, or the reverse.
    pub(super) fn has_warn(&self) -> bool {
        self.cpu_warn
            || self.mem_warn
            || self.conn_warn
            || self.ping_warn
            || self.exch_warn
            || self.api_warn
    }
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
                ping_warn: false,
                exch_warn: false,
                api_warn: false,
                api_key: ApiKeyState::Unknown,
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
                startup: None,
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

/// Roll one server's startup column up from its cores.
///
/// A collapsed group hides its rows, so this cannot be "whatever the first core says". Two rules,
/// in order:
///
/// 1. If ANY core is still coming up, show THAT core's progress — the unfinished one is the reason
///    somebody opened this panel, and it must not be averaged away by its finished siblings. Ties
///    break on the least progress, then the longest elapsed, so the worst case wins.
/// 2. Otherwise, if every core has finished, show the LONGEST time any of them took: "this machine
///    took N seconds to come up". A mean would understate the slow core and a first-match would be
///    arbitrary.
///
/// Args:
///     cores: The group's core rows, already collected.
///
/// Returns:
///     The cell to render on the server row, or `None` when no core reports anything.
pub(super) fn group_startup(cores: &[CoreStatusRow]) -> Option<StartupCell> {
    let cells: Vec<StartupCell> = cores
        .iter()
        .map(|core| startup_cell(&core.status, &core.startup))
        .collect();
    let worst_progress = cells
        .iter()
        .filter_map(|cell| match *cell {
            StartupCell::Progress {
                done,
                total,
                elapsed_ms,
            } => Some((done, total, elapsed_ms)),
            _ => None,
        })
        .min_by_key(|(done, _, elapsed_ms)| (*done, std::cmp::Reverse(*elapsed_ms)));
    if let Some((done, total, elapsed_ms)) = worst_progress {
        return Some(StartupCell::Progress {
            done,
            total,
            elapsed_ms,
        });
    }
    cells
        .iter()
        .filter_map(|cell| match *cell {
            StartupCell::Done { elapsed_ms } => Some(elapsed_ms),
            _ => None,
        })
        .max()
        .map(|elapsed_ms| StartupCell::Done { elapsed_ms })
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

    // Both halves of the API-key aggregate derive from the rows this group already holds, so they
    // are decided together — the displayed key and the flag beside it cannot drift apart.
    group.api_key = soonest_key(&group.cores);
    group.api_warn = group.cores.iter().any(|core| core.api_warn);
    group.startup = group_startup(&group.cores);

    let mut has_process_memory = false;
    let mut process_memory_mb = 0u64;
    for value in group.cores.iter().filter_map(|row| row.sys.used_memory_mb) {
        has_process_memory = true;
        process_memory_mb += u64::from(value);
    }
    group.process_memory_mb = has_process_memory.then_some(process_memory_mb);
}

/// The key a server row stands for: the most urgent among its cores, by [`ApiKeyState::urgency`].
///
/// A dated key outranks everything, and `Perpetual` surfaces only when EVERY core says so —
/// otherwise one core's unlimited key would speak for a sibling nobody could check.
///
/// Args:
///     cores: The server's core rows, each already classified.
///
/// Returns:
///     The state to show on the server row, and to sort the server list by.
fn soonest_key(cores: &[CoreStatusRow]) -> ApiKeyState {
    // The most urgent state on the server, by the same ordering the column sorts on — so the row
    // shows what the sort would rank it by, and one core's "unlimited" cannot speak for a sibling
    // nobody could check.
    cores
        .iter()
        .map(|core| core.api_key)
        .min_by_key(|state| state.urgency())
        .unwrap_or(ApiKeyState::Unknown)
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
