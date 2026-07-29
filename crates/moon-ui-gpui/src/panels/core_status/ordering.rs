//! Pure ordering and naming helpers for the Core Status panel: server display names, the flat
//! table's column comparators, and natural (human) name ordering. No view state, so they live apart
//! from `mod.rs`.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::net::IpAddr;

use moon_core::feed::ConnStatus;
use rust_i18n::t;

use super::model::{CoreStatusRow, ServerStatusGroup};

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
        "cpus" => a.sys.logical_cpu_count.cmp(&b.sys.logical_cpu_count),
        // "core" and any unknown key sort by name.
        _ => a.name.cmp(&b.name),
    }
}
