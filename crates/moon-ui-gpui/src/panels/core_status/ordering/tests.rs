//! Tests for the By IP group sort comparator.
//!
//! Explicit imports (no `use super::*`) per the crate's test convention: the panel's parent module
//! re-exports `gpui::*`, whose own `test` would shadow the built-in attribute.

use std::cmp::Ordering;

use moon_core::feed::ConnStatus;
use moon_core::session::CoreSysStatus;

use super::{GroupSortField, compare_groups};
use crate::backend::core_warn::LatencySeverity;
use crate::panels::core_status::model::{
    CoreStatusRow, ServerConnectivity, ServerKey, ServerStatusGroup,
};

/// Build a minimal server group with the fields the comparator reads; `rtts` sets each core's
/// round-trip (and whether it is Ready), so `worst_latency`'s Ready gate can be exercised.
fn group(
    name: &str,
    cpu: Option<u8>,
    proc_mb: Option<u64>,
    free_mb: Option<u16>,
    rtts: &[(u32, bool)],
) -> ServerStatusGroup {
    let cores = rtts
        .iter()
        .enumerate()
        .map(|(i, &(rtt, ready))| CoreStatusRow {
            id: i as u64,
            name: format!("c{i}"),
            status: if ready {
                ConnStatus::Ready
            } else {
                ConnStatus::Disconnected
            },
            sys: CoreSysStatus {
                round_trip_ms: Some(rtt),
                ..CoreSysStatus::default()
            },
            endpoint: None,
            ping_warn: false,
            exch_warn: false,
            ping_sev: LatencySeverity::Normal,
            exch_sev: LatencySeverity::Normal,
        })
        .collect::<Vec<_>>();
    let ready_count = cores
        .iter()
        .filter(|c| c.status == ConnStatus::Ready)
        .count();
    ServerStatusGroup {
        key: ServerKey::Unknown(0),
        display_name: name.to_string(),
        cpu_warn: false,
        mem_warn: false,
        conn_warn: false,
        ping_warn: false,
        exch_warn: false,
        address: None,
        cores,
        ready_count,
        connectivity: ServerConnectivity::Online,
        system_cpu_percent: cpu,
        process_memory_mb: proc_mb,
        free_physical_memory_mb: free_mb,
        logical_cpu_count: None,
    }
}

/// CPU sorts by the system percentage, and an absent percentage sorts below any value (so unknown
/// servers group at the ascending end).
#[test]
fn cpu_orders_by_system_percent() {
    let hot = group("hot", Some(80), None, None, &[]);
    let cool = group("cool", Some(20), None, None, &[]);
    let unknown = group("unknown", None, None, None, &[]);

    assert_eq!(
        compare_groups(&hot, &cool, GroupSortField::Cpu),
        Ordering::Greater
    );
    assert_eq!(
        compare_groups(&unknown, &cool, GroupSortField::Cpu),
        Ordering::Less,
        "no CPU reading sorts below a known one"
    );
}

/// Memory sorts by the FREE share of the reconstructed total (process RAM + free), not the raw free
/// megabytes: 100 MB free of 200 total (50%) ranks above 150 MB free of 600 total (25%).
#[test]
fn mem_orders_by_free_percentage() {
    let roomy = group("roomy", Some(0), Some(100), Some(100), &[]); // 100/(100+100) = 50%
    let tight = group("tight", Some(0), Some(450), Some(150), &[]); // 150/(450+150) = 25%

    assert_eq!(
        compare_groups(&roomy, &tight, GroupSortField::Mem),
        Ordering::Greater,
        "higher free share ranks higher despite fewer absolute free MB elsewhere"
    );
}

/// Ping sorts by the WORST round-trip among READY cores only: a disconnected core's high stale RTT
/// must not count.
#[test]
fn ping_uses_worst_ready_core_only() {
    // Ready 120 ms, plus a disconnected core stuck at 9000 ms that must be ignored.
    let a = group("a", None, None, None, &[(120, true), (9000, false)]);
    // Ready 300 ms.
    let b = group("b", None, None, None, &[(300, true)]);

    assert_eq!(
        compare_groups(&a, &b, GroupSortField::Ping),
        Ordering::Less,
        "a's worst READY ping (120) is below b's (300); the stale 9000 is ignored"
    );
}

/// Name uses natural order, and any field falls back to the name when it ties, so equal metrics keep
/// a stable order instead of reshuffling.
#[test]
fn name_is_natural_and_the_tiebreak() {
    let s2 = group("Server 2", Some(50), None, None, &[]);
    let s10 = group("Server 10", Some(50), None, None, &[]);

    assert_eq!(
        compare_groups(&s2, &s10, GroupSortField::Name),
        Ordering::Less,
        "Server 2 < Server 10 in natural order"
    );
    // Equal CPU → the comparator falls back to the natural name order.
    assert_eq!(
        compare_groups(&s2, &s10, GroupSortField::Cpu),
        Ordering::Less,
        "equal CPU breaks the tie by name"
    );
}
