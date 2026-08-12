//! Pure ordering and naming helpers for the Core Status panel: server display names, the flat
//! table's column comparators, and natural (human) name ordering. No view state, so they live apart
//! from `mod.rs`.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::net::IpAddr;

use moon_core::feed::ConnStatus;
use moon_core::session::CoreSysStatus;
use rust_i18n::t;

use super::model::{CoreStatusRow, ServerStatusGroup};

/// Which By IP column the server list is sorted on. Warnings always pin to the top regardless of the
/// field (handled by the caller), so this only orders within the warned and the quiet partitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GroupSortField {
    /// Server display name — the default, in natural order.
    Name,
    /// Whole-machine system CPU percent.
    Cpu,
    /// Free-memory share of the reconstructed machine total (the "Память своб." column).
    Mem,
    /// Worst client↔core round-trip among the server's ready cores.
    Ping,
    /// Worst core→exchange latency among the server's ready cores.
    Exch,
    /// Ready core count.
    Cores,
    /// The server's most urgent API key, by [`ApiKeyState::urgency`] — which may be a day count,
    /// or neither when nothing is known or every key is unlimited.
    ApiKey,
}

impl GroupSortField {
    /// Return the stable persistence key for one By-IP sort column.
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Cpu => "cpu",
            Self::Mem => "mem",
            Self::Ping => "ping",
            Self::Exch => "exch",
            Self::Cores => "cores",
            Self::ApiKey => "api_key",
        }
    }

    /// Resolve a persisted By-IP key without treating an unknown value as Name.
    pub(super) fn from_key(key: &str) -> Option<Self> {
        match key {
            "name" => Some(Self::Name),
            "cpu" => Some(Self::Cpu),
            "mem" => Some(Self::Mem),
            "ping" => Some(Self::Ping),
            "exch" => Some(Self::Exch),
            "cores" => Some(Self::Cores),
            "api_key" => Some(Self::ApiKey),
            _ => None,
        }
    }
}

/// Restore a valid Flat-mode sort, leaving `None` as the historical attention order.
pub(super) fn restore_flat_sort(
    preference: Option<moon_core::config::TableSortPreference>,
) -> Option<(String, bool)> {
    const KEYS: [&str; 11] = [
        "server",
        "core",
        "status",
        "cpu_proc",
        "cpu_sys",
        "mem_used",
        "free_phys",
        "ping",
        "ping_exch",
        "cpus",
        "api_key",
    ];
    preference.and_then(|preference| {
        KEYS.contains(&preference.column.as_str())
            .then_some((preference.column, preference.ascending))
    })
}

/// Restore a valid By-IP sort, falling back to its historical Name-ascending order.
pub(super) fn restore_group_sort(
    preference: Option<moon_core::config::TableSortPreference>,
) -> (GroupSortField, bool) {
    preference
        .and_then(|preference| {
            GroupSortField::from_key(&preference.column).map(|field| (field, preference.ascending))
        })
        .unwrap_or((GroupSortField::Name, true))
}

/// Worst (highest) latency among a group's READY cores for one accessor, matching the value the
/// server row surfaces. `None` when no ready core has the reading.
fn worst_latency(
    group: &ServerStatusGroup,
    read: impl Fn(&CoreSysStatus) -> Option<u32>,
) -> Option<u32> {
    group
        .cores
        .iter()
        .filter(|core| core.status == ConnStatus::Ready)
        .filter_map(|core| read(&core.sys))
        .max()
}

/// Free-memory percentage of the reconstructed machine total (process RAM sum + free physical),
/// matching the "Память своб." column. `None` until free memory has arrived, so such servers group
/// together at the ascending end.
fn free_pct(group: &ServerStatusGroup) -> Option<u64> {
    let free_mb = u64::from(group.free_physical_memory_mb?);
    let total_mb = group.process_memory_mb.unwrap_or(0) + free_mb;
    if total_mb == 0 {
        return Some(0);
    }
    Some(free_mb * 100 / total_mb)
}

/// Compare two server groups on one sort field, ascending. The caller reverses for descending and
/// applies the warnings-first pin separately; a name tiebreak keeps the order stable when the field
/// ties (so equal metrics don't reshuffle each tick).
///
/// Args:
///     a: First group.
///     b: Second group.
///     field: The active sort column.
///
/// Returns:
///     The ascending ordering for that field, then by name.
pub(super) fn compare_groups(
    a: &ServerStatusGroup,
    b: &ServerStatusGroup,
    field: GroupSortField,
) -> Ordering {
    match field {
        GroupSortField::Name => Ordering::Equal,
        GroupSortField::Cpu => a.system_cpu_percent.cmp(&b.system_cpu_percent),
        GroupSortField::Mem => free_pct(a).cmp(&free_pct(b)),
        GroupSortField::Ping => worst_latency(a, |sys| sys.round_trip_ms)
            .cmp(&worst_latency(b, |sys| sys.round_trip_ms)),
        GroupSortField::Exch => worst_latency(a, |sys| sys.order_api_latency_ms.map(u32::from))
            .cmp(&worst_latency(b, |sys| {
                sys.order_api_latency_ms.map(u32::from)
            })),
        GroupSortField::Cores => a
            .ready_count
            .cmp(&b.ready_count)
            .then_with(|| a.cores.len().cmp(&b.cores.len())),
        // The very key the server row displays, ordered the same way — so the column cannot sort by
        // one thing and show another. Not Ready-gated, unlike the latencies: a key keeps ageing
        // while its core is down.
        GroupSortField::ApiKey => a.api_key.urgency().cmp(&b.api_key.urgency()),
    }
    .then_with(|| natural_cmp(&a.display_name, &b.display_name))
}

/// Fill each group's display name from a custom name or a stable `Server N` ordinal.
///
/// Ordinals rank address servers by sorted address so a name stays put under attention-first
/// reordering. Unknown-endpoint servers keep a core-qualified fallback label.
///
/// Args:
///     groups: Aggregated server snapshots to name in place.
///     names: Custom names keyed by endpoint IP string.
///
/// Returns:
///     Nothing; `display_name` is set on every group.
pub(super) fn assign_server_names(
    groups: &mut [ServerStatusGroup],
    names: &HashMap<String, String>,
) {
    let mut addresses = groups
        .iter()
        .filter_map(|group| group.address)
        .collect::<Vec<IpAddr>>();
    addresses.sort();
    for group in groups.iter_mut() {
        group.display_name = match group.address {
            Some(address) => {
                let ip = address.to_string();
                names.get(&ip).cloned().unwrap_or_else(|| {
                    let ordinal = addresses
                        .iter()
                        .position(|candidate| *candidate == address)
                        .map(|index| index + 1)
                        .unwrap_or(0);
                    t!("core_status.server_n", n = ordinal).to_string()
                })
            }
            None => {
                let core = group
                    .cores
                    .first()
                    .map(|core| core.name.as_str())
                    .unwrap_or("-");
                t!("core_status.unknown_server", core = core).to_string()
            }
        };
    }
}

/// Return a stable ordinal for sorting the flat table's status column.
///
/// Args:
///     s: Current connection status.
///
/// Returns:
///     A rank placing ready cores first and failed cores last.
fn status_ord(s: &ConnStatus) -> u64 {
    match s {
        ConnStatus::Ready => 0,
        ConnStatus::Connecting | ConnStatus::Stage(_) => 1,
        ConnStatus::Disconnected => 2,
        ConnStatus::Failed(_) => 3,
    }
}

/// Compare two names in natural order: digit runs compare as numbers, everything else
/// case-insensitively. So `Server 2` sorts before `Server 10`, and `F1` before `Server 1`.
///
/// Args:
///     a: First name.
///     b: Second name.
///
/// Returns:
///     The natural ordering of the two names.
pub(super) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                // Compare digit runs numerically without parsing: strip leading zeros, then longer
                // run wins, then lexically.
                let da = take_digits(&mut a);
                let db = take_digits(&mut b);
                let na = da.trim_start_matches('0');
                let nb = db.trim_start_matches('0');
                match na.len().cmp(&nb.len()).then_with(|| na.cmp(nb)) {
                    Ordering::Equal => {}
                    ord => return ord,
                }
            }
            (Some(ca), Some(cb)) => match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                Ordering::Equal => {
                    a.next();
                    b.next();
                }
                ord => return ord,
            },
        }
    }
}

/// Consume and return a run of consecutive ASCII digits from a peekable char iterator.
fn take_digits<I: Iterator<Item = char>>(it: &mut std::iter::Peekable<I>) -> String {
    let mut digits = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            digits.push(c);
            it.next();
        } else {
            break;
        }
    }
    digits
}

/// Compare two flat rows on one column key for header-click sorting.
///
/// `None` metrics sort before any value, so cores that have not reported a field group together.
///
/// Args:
///     a: First row.
///     b: Second row.
///     key: Column key from the table header.
///
/// Returns:
///     The ascending ordering for that column.
pub(super) fn compare_flat_rows(a: &CoreStatusRow, b: &CoreStatusRow, key: &str) -> Ordering {
    match key {
        // "server" is handled by name in `sorted_flat_rows`, not here.
        "status" => status_ord(&a.status).cmp(&status_ord(&b.status)),
        "cpu_proc" => a.sys.process_cpu_percent.cmp(&b.sys.process_cpu_percent),
        "cpu_sys" => a.sys.system_cpu_percent.cmp(&b.sys.system_cpu_percent),
        "mem_used" => a.sys.used_memory_mb.cmp(&b.sys.used_memory_mb),
        "free_phys" => a
            .sys
            .free_physical_memory_mb
            .cmp(&b.sys.free_physical_memory_mb),
        "ping" => a.sys.round_trip_ms.cmp(&b.sys.round_trip_ms),
        "ping_exch" => a.sys.order_api_latency_ms.cmp(&b.sys.order_api_latency_ms),
        "cpus" => a.sys.logical_cpu_count.cmp(&b.sys.logical_cpu_count),
        // By URGENCY, not by the cell text and not by the raw number: "9" must not sort above "45",
        // an expired key leads, and the two states with no number must trail the counts rather than
        // heading the column — a dash and an infinity are the LAST things to look at here.
        "api_key" => a.api_key.urgency().cmp(&b.api_key.urgency()),
        // "core" and any unknown key sort by name.
        _ => a.name.cmp(&b.name),
    }
}

#[cfg(test)]
mod tests;
