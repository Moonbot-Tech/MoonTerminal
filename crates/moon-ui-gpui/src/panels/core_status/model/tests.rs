//! Regression tests for Core Status server aggregation.

use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::{ConnStatus, CoreEndpoint};
use moon_core::session::CoreSysStatus;

use super::{CoreStatusRow, ServerConnectivity, ServerKey, aggregate_servers};

/// Build one core snapshot for aggregation tests.
fn row(
    id: u64,
    address: Option<[u8; 4]>,
    port: u16,
    status: ConnStatus,
    sys: CoreSysStatus,
) -> CoreStatusRow {
    CoreStatusRow {
        id,
        name: format!("Core {id}"),
        status,
        sys,
        endpoint: address.map(|octets| CoreEndpoint {
            address: IpAddr::V4(Ipv4Addr::from(octets)),
            port,
        }),
    }
}

/// `model.rs:ServerKey::for_row` must ignore `CoreEndpoint::port`; including it splits two processes
/// on one machine into duplicate server rows.
#[test]
fn same_ip_with_different_ports_forms_one_server() {
    let rows = [
        row(
            11,
            Some([10, 20, 30, 40]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            12,
            Some([10, 20, 30, 40]),
            4000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
    ];

    let groups = aggregate_servers(&rows);

    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].key,
        ServerKey::Address(IpAddr::V4(Ipv4Addr::new(10, 20, 30, 40)))
    );
    assert_eq!(
        groups[0]
            .cores
            .iter()
            .map(|core| core.id)
            .collect::<Vec<_>>(),
        vec![11, 12]
    );
}

/// `model.rs:finish_group` must use the newest sample independently per machine metric and a `u64`
/// process-memory sum; one whole sample or a `u16` sum shows stale data or overflows.
#[test]
fn metrics_use_freshest_sources_and_wide_process_sum() {
    let rows = [
        row(
            21,
            Some([192, 0, 2, 1]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus {
                system_cpu_percent: Some(18),
                used_memory_mb: Some(40_000),
                free_physical_memory_mb: Some(24_000),
                logical_cpu_count: None,
                updated_ms: 200,
                ..CoreSysStatus::default()
            },
        ),
        row(
            22,
            Some([192, 0, 2, 1]),
            3001,
            ConnStatus::Ready,
            CoreSysStatus {
                system_cpu_percent: Some(91),
                used_memory_mb: Some(30_000),
                free_physical_memory_mb: None,
                logical_cpu_count: Some(32),
                updated_ms: 300,
                ..CoreSysStatus::default()
            },
        ),
    ];

    let server = &aggregate_servers(&rows)[0];

    assert_eq!(server.system_cpu_percent, Some(91));
    assert_eq!(server.free_physical_memory_mb, Some(24_000));
    assert_eq!(server.logical_cpu_count, Some(32));
    assert_eq!(server.process_memory_mb, Some(70_000));
}

/// `model.rs:ServerKey::for_row` must keep the core-specific unknown fallback; merging missing
/// endpoints invents a fictitious server with misleading totals.
#[test]
fn unknown_endpoints_remain_isolated() {
    let rows = [
        row(
            31,
            None,
            0,
            ConnStatus::Disconnected,
            CoreSysStatus::default(),
        ),
        row(
            32,
            None,
            0,
            ConnStatus::Disconnected,
            CoreSysStatus::default(),
        ),
    ];

    let groups = aggregate_servers(&rows);

    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].key, ServerKey::Unknown(31));
    assert_eq!(groups[1].key, ServerKey::Unknown(32));
}

/// `model.rs:aggregate_servers` must order servers by address (matching the `Server N` ordinal), so
/// the By IP list is stable instead of jumping when connectivity changes.
#[test]
fn servers_are_ordered_by_address() {
    let rows = [
        row(
            3,
            Some([10, 0, 0, 3]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            1,
            Some([10, 0, 0, 1]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            2,
            Some([10, 0, 0, 2]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
    ];

    let addresses = aggregate_servers(&rows)
        .iter()
        .filter_map(|group| group.address)
        .collect::<Vec<_>>();

    assert_eq!(
        addresses,
        vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
        ]
    );
}

/// `model.rs:aggregate_servers` must order a server's cores by name, not by arrival.
#[test]
fn cores_within_server_ordered_by_name() {
    let address = Some([10, 0, 0, 9]);
    let rows = [
        row(
            74,
            address,
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            72,
            address,
            3001,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            71,
            address,
            3002,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            73,
            address,
            3003,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
    ];

    let groups = aggregate_servers(&rows);

    assert_eq!(
        groups[0]
            .cores
            .iter()
            .map(|core| core.id)
            .collect::<Vec<_>>(),
        vec![71, 72, 73, 74]
    );
}

/// `model.rs:connectivity` must treat a partially ready server as degraded; classifying any ready
/// child as online hides a failed sibling.
#[test]
fn partial_readiness_is_degraded() {
    let rows = [
        row(
            41,
            Some([203, 0, 113, 5]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            42,
            Some([203, 0, 113, 5]),
            3001,
            ConnStatus::Failed("offline".to_string()),
            CoreSysStatus::default(),
        ),
    ];

    let server = &aggregate_servers(&rows)[0];

    assert_eq!(server.ready_count, 1);
    assert_eq!(server.connectivity, ServerConnectivity::Degraded);
    // The connectivity WARNING (conn_warn) now comes from the backend engine, tested beside it.
}
