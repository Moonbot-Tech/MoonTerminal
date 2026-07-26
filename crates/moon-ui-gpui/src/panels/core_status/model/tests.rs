//! Regression tests for Core Status server aggregation.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::{ConnStatus, CoreEndpoint};
use moon_core::session::CoreSysStatus;

use super::{
    CoreStatusRow, ServerConnectivity, ServerKey, aggregate_servers, normal_core_boundary,
    normal_server_boundary, visible_flat_rows,
};

/// Build one explicit core snapshot for aggregation tests.
///
/// Args:
///     id: Stable core identity.
///     address: Optional IPv4 endpoint address.
///     port: Endpoint port used only when `address` is present.
///     status: Connection state.
///     sys: Telemetry snapshot.
///
/// Returns:
///     A named core row with no exchange identity.
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
        exchange: None,
    }
}

/// `model.rs:ServerKey::for_row` must ignore `CoreEndpoint::port`; including it splits two
/// processes on one machine into duplicate visible server rows.
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

/// `model.rs:finish_group` must use the newest sample independently for each machine metric and a
/// `u64` process-memory sum; selecting one whole sample or adding as `u16` shows stale data or
/// overflows a server running several large processes.
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

    let groups = aggregate_servers(&rows);
    let server = &groups[0];

    assert_eq!(server.system_cpu_percent, Some(91));
    assert_eq!(server.free_physical_memory_mb, Some(24_000));
    assert_eq!(server.logical_cpu_count, Some(32));
    assert_eq!(server.process_memory_mb, Some(70_000));
}

/// `model.rs:ServerKey::for_row` must retain the core-specific unknown fallback; merging missing
/// endpoints creates a fictitious server and misleading machine totals.
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

/// `model.rs:visible_flat_rows` dropping the hidden-server filter must fail here; the hidden
/// attention row would remain above the Ready row and create a false mixed-section boundary.
#[test]
fn hidden_server_is_excluded_from_flat_rows() {
    let rows = [
        row(
            35,
            Some([192, 0, 2, 35]),
            3000,
            ConnStatus::Failed("offline".to_string()),
            CoreSysStatus::default(),
        ),
        row(
            36,
            Some([192, 0, 2, 36]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
    ];
    let hidden = HashSet::from([ServerKey::Address(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 35)))]);

    let visible = visible_flat_rows(&rows, &hidden);

    assert_eq!(
        visible.iter().map(|core| core.id).collect::<Vec<_>>(),
        vec![36]
    );
    assert_eq!(normal_core_boundary(&visible), None);
}

/// `model.rs:connectivity` must treat a partially ready server as degraded; classifying any ready
/// child as online hides a failed sibling from the operator.
#[test]
fn partial_readiness_is_degraded() {
    let ready = row(
        41,
        Some([203, 0, 113, 5]),
        3000,
        ConnStatus::Ready,
        CoreSysStatus::default(),
    );
    let failed = row(
        42,
        Some([203, 0, 113, 5]),
        3001,
        ConnStatus::Failed("offline".to_string()),
        CoreSysStatus::default(),
    );

    let groups = aggregate_servers(&[ready, failed]);
    let server = &groups[0];

    assert_eq!(server.ready_count, 1);
    assert_eq!(server.connectivity, ServerConnectivity::Degraded);
}

/// `model.rs:visible_flat_rows` removing its attention sort or `aggregate_servers` removing its
/// group sort must fail here; reconnecting cores or Degraded servers would remain below healthy
/// entries.
#[test]
fn every_non_ready_state_leads_stably_in_both_modes() {
    let rows = [
        row(
            61,
            Some([10, 0, 0, 1]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            62,
            Some([10, 0, 0, 2]),
            3000,
            ConnStatus::Connecting,
            CoreSysStatus::default(),
        ),
        row(
            63,
            Some([10, 0, 0, 3]),
            3000,
            ConnStatus::Stage("auth".to_string()),
            CoreSysStatus::default(),
        ),
        row(
            64,
            Some([10, 0, 0, 4]),
            3000,
            ConnStatus::Failed("offline".to_string()),
            CoreSysStatus::default(),
        ),
        row(
            65,
            Some([10, 0, 0, 5]),
            3000,
            ConnStatus::Disconnected,
            CoreSysStatus::default(),
        ),
        row(
            66,
            Some([10, 0, 0, 6]),
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
    ];

    let flat = visible_flat_rows(&rows, &HashSet::new());
    assert_eq!(
        flat.iter().map(|core| core.id).collect::<Vec<_>>(),
        vec![62, 63, 64, 65, 61, 66]
    );
    assert_eq!(normal_core_boundary(&flat), Some(4));

    let groups = aggregate_servers(&rows);
    assert_eq!(
        groups
            .iter()
            .map(|group| group.cores[0].id)
            .collect::<Vec<_>>(),
        vec![62, 63, 64, 65, 61, 66]
    );
    assert_eq!(
        groups
            .iter()
            .map(|group| group.connectivity)
            .collect::<Vec<_>>(),
        vec![
            ServerConnectivity::Degraded,
            ServerConnectivity::Degraded,
            ServerConnectivity::Offline,
            ServerConnectivity::Offline,
            ServerConnectivity::Online,
            ServerConnectivity::Online,
        ]
    );
    assert_eq!(normal_server_boundary(&groups), Some(4));
}

/// `model.rs:aggregate_servers` must stable-partition children after connectivity aggregation;
/// removing the child sort leaves a failed process below Ready siblings on a Degraded server.
#[test]
fn mixed_server_children_put_attention_before_ready_stably() {
    let address = Some([203, 0, 113, 71]);
    let rows = [
        row(
            71,
            address,
            3000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
        row(
            72,
            address,
            3001,
            ConnStatus::Failed("offline".to_string()),
            CoreSysStatus::default(),
        ),
        row(
            73,
            address,
            3002,
            ConnStatus::Connecting,
            CoreSysStatus::default(),
        ),
        row(
            74,
            address,
            3003,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        ),
    ];

    let groups = aggregate_servers(&rows);
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0]
            .cores
            .iter()
            .map(|core| core.id)
            .collect::<Vec<_>>(),
        vec![72, 73, 71, 74]
    );
    assert_eq!(groups[0].connectivity, ServerConnectivity::Degraded);
    assert_eq!(normal_core_boundary(&groups[0].cores), Some(2));
}

/// `model.rs:mixed_normal_boundary` must reject zero and absent transitions; returning `Some(0)`
/// draws an attention separator over entirely healthy lists, while inventing a terminal boundary
/// draws one below entirely failed lists.
#[test]
fn section_boundaries_exist_only_for_mixed_lists() {
    let ready = row(
        81,
        Some([192, 0, 2, 81]),
        3000,
        ConnStatus::Ready,
        CoreSysStatus::default(),
    );
    let failed = row(
        82,
        Some([192, 0, 2, 82]),
        3000,
        ConnStatus::Failed("offline".to_string()),
        CoreSysStatus::default(),
    );

    assert_eq!(normal_core_boundary(&[]), None);
    assert_eq!(normal_core_boundary(std::slice::from_ref(&ready)), None);
    assert_eq!(normal_core_boundary(std::slice::from_ref(&failed)), None);
    assert_eq!(normal_server_boundary(&[]), None);
    assert_eq!(
        normal_server_boundary(&aggregate_servers(std::slice::from_ref(&ready))),
        None
    );
    assert_eq!(
        normal_server_boundary(&aggregate_servers(std::slice::from_ref(&failed))),
        None
    );
}
