//! Regression tests for Core Status server aggregation.

use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::{ConnStatus, CoreEndpoint, CoreTimeOffsetStatus};
use moon_core::session::{CoreStartupState, CoreStartupStatus, CoreSysStatus};

use super::{
    CoreStatusRow, GroupVersion, ServerConnectivity, ServerKey, aggregate_servers, group_version,
};

/// Build one core snapshot for aggregation tests.
fn row(
    id: u64,
    address: Option<[u8; 4]>,
    port: u16,
    status: ConnStatus,
    sys: CoreSysStatus,
) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        id,
        name: format!("Core {id}"),
        status,
        sys,
        endpoint: address.map(|octets| CoreEndpoint {
            address: IpAddr::V4(Ipv4Addr::from(octets)),
            port,
        }),
        ping_warn: false,
        exch_warn: false,
        ping_sev: crate::backend::core_warn::LatencySeverity::Normal,
        exch_sev: crate::backend::core_warn::LatencySeverity::Normal,
        api_key: crate::panels::core_status::model::ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        startup: CoreStartupStatus::default(),
        time_offset: CoreTimeOffsetStatus::default(),
        server_version: None,
        version_behind: None,
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

    let groups = aggregate_servers(&rows, None);

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

/// `model.rs:group_version` must treat a silent sibling as disagreement; changing that branch to
/// `Uniform` would make a collapsed server row claim a build that one of its cores never reported.
#[test]
fn group_version_requires_every_core_to_report_the_same_build() {
    let mut v734 = row(1, None, 0, ConnStatus::Ready, CoreSysStatus::default());
    v734.server_version = Some(734);
    let mut same_v734 = row(2, None, 0, ConnStatus::Ready, CoreSysStatus::default());
    same_v734.server_version = Some(734);
    let mut v735 = row(3, None, 0, ConnStatus::Ready, CoreSysStatus::default());
    v735.server_version = Some(735);
    let silent = row(4, None, 0, ConnStatus::Connecting, CoreSysStatus::default());

    assert_eq!(
        group_version(&[v734.clone(), same_v734]),
        GroupVersion::Uniform(734)
    );
    assert_eq!(group_version(&[v734.clone(), v735]), GroupVersion::Mixed);
    assert_eq!(group_version(&[v734, silent.clone()]), GroupVersion::Mixed);
    assert_eq!(group_version(&[silent]), GroupVersion::Absent);
    assert_eq!(group_version(&[]), GroupVersion::Absent);
}

/// `model.rs:finish_group` must mark a collapsed group as behind only after every core agrees on
/// its build. Mutation: replace the Uniform-only match with a `find_map` over child
/// `version_behind`; a Mixed group would claim that its ellipsis is an older build.
#[test]
fn only_uniform_groups_are_marked_behind_the_fleet() {
    let address = Some([192, 0, 2, 55]);
    let mut older = row(
        1,
        address,
        3000,
        ConnStatus::Ready,
        CoreSysStatus::default(),
    );
    older.server_version = Some(734);
    older.version_behind = Some(735);
    let mut newest = row(
        2,
        address,
        3001,
        ConnStatus::Ready,
        CoreSysStatus::default(),
    );
    newest.server_version = Some(735);

    let groups = aggregate_servers(&[older, newest], Some(735));

    assert_eq!(groups[0].version, GroupVersion::Mixed);
    assert_eq!(groups[0].version_behind, None);
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

    let server = &aggregate_servers(&rows, None)[0];

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

    let groups = aggregate_servers(&rows, None);

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

    let addresses = aggregate_servers(&rows, None)
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

    let groups = aggregate_servers(&rows, None);

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

    let server = &aggregate_servers(&rows, None)[0];

    assert_eq!(server.ready_count, 1);
    assert_eq!(server.connectivity, ServerConnectivity::Degraded);
    // The connectivity WARNING (conn_warn) now comes from the backend engine, tested beside it.
}

/// One core row carrying only an API-key state.
fn key_row(id: u64, address: [u8; 4], key: super::ApiKeyState) -> CoreStatusRow {
    CoreStatusRow {
        api_key: key,
        ..row(
            id,
            Some(address),
            7000,
            ConnStatus::Ready,
            CoreSysStatus::default(),
        )
    }
}

/// A server stands for the key expiring SOONEST among its cores, so the row an operator scans
/// reports the one that actually needs attention rather than whichever core sorted first.
#[test]
fn a_server_reports_its_soonest_key() {
    let groups = aggregate_servers(
        &[
            key_row(1, [10, 0, 0, 1], super::ApiKeyState::Days(40)),
            key_row(2, [10, 0, 0, 1], super::ApiKeyState::Days(3)),
            key_row(3, [10, 0, 0, 1], super::ApiKeyState::Perpetual),
        ],
        None,
    );

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].api_key, super::ApiKeyState::Days(3));
}

/// "Effectively unlimited" may speak for a server only when EVERY core on it says so. One unlimited
/// key beside a sibling nobody could check would otherwise report the whole machine as safe.
#[test]
fn one_unlimited_key_does_not_speak_for_unchecked_siblings() {
    let mixed = aggregate_servers(
        &[
            key_row(1, [10, 0, 0, 2], super::ApiKeyState::Perpetual),
            key_row(2, [10, 0, 0, 2], super::ApiKeyState::Unknown),
        ],
        None,
    );
    assert_eq!(mixed[0].api_key, super::ApiKeyState::Unknown);

    let all_unlimited = aggregate_servers(
        &[
            key_row(1, [10, 0, 0, 3], super::ApiKeyState::Perpetual),
            key_row(2, [10, 0, 0, 3], super::ApiKeyState::Perpetual),
        ],
        None,
    );
    assert_eq!(all_unlimited[0].api_key, super::ApiKeyState::Perpetual);
}

/// An expired key outranks every live one when a server picks what to show — it is the most urgent
/// state the column has, and `days()` places it below zero for exactly that reason.
#[test]
fn an_expired_key_wins_the_server_row() {
    let groups = aggregate_servers(
        &[
            key_row(1, [10, 0, 0, 4], super::ApiKeyState::Days(0)),
            key_row(2, [10, 0, 0, 4], super::ApiKeyState::Days(-3)),
        ],
        None,
    );

    assert_eq!(groups[0].api_key, super::ApiKeyState::Days(-3));
}

/// A SUCCESSFUL check that carries no expiration date means the core looked and the key is
/// unlimited — the answer most of a Binance fleet gives. Reading it as "nothing known" instead
/// paints a dash over every healthy unlimited key, which is precisely the regression this pins:
/// the protocol separates "unlimited" from "could not check" by the response's success flag, and a
/// failed check never becomes an `ApiKeyExpiry` at all — it arrives here as `None`.
///
/// The fixture is what the converter builds for that answer: `unlimited`, with no date and no
/// count — the zero the wire carries beside it is deliberately not kept as a number.
#[test]
fn a_successful_answer_without_a_date_is_unlimited_and_no_answer_is_unknown() {
    let no_expiry = moon_core::session::ApiKeyExpiry {
        unlimited: true,
        known: false,
        days_left: None,
        at_unix: None,
        checked_ms: 0,
    };

    assert_eq!(
        super::ApiKeyState::of(Some(no_expiry), 0),
        super::ApiKeyState::Perpetual,
        "the core checked and found no expiry — that is an unlimited key, not an unknown one"
    );
    assert_eq!(
        super::ApiKeyState::of(None, 0),
        super::ApiKeyState::Unknown,
        "a failed or absent check is the only thing that reads as unknown"
    );
}

/// The two thresholds of this feature live in different crates, and only their ORDER keeps the
/// panel honest: the warning horizon must stay below the point where a count turns into ∞, or the
/// engine would light a triangle on a cell that says "cannot expire". Nothing else asserts it.
#[test]
fn the_warning_horizon_stays_below_the_unlimited_cut() {
    assert!(
        i32::from(moon_core::config::layout::API_WARN_MAX_DAYS) < super::API_PERPETUAL_DAYS,
        "a horizon reaching into the unlimited range would warn on an infinity cell"
    );
}

/// A lifetime of a year or more is not a number an operator acts on, and a round 1000 is what two
/// Bybit cores answer live — too round to be a real date. Both read as unlimited, while anything
/// inside a year keeps its exact day count.
#[test]
fn a_year_or_more_reads_as_unlimited() {
    let dated = |days: i64| {
        Some(moon_core::session::ApiKeyExpiry {
            unlimited: false,
            known: true,
            days_left: Some(days as i32),
            at_unix: Some(days * 86_400),
            checked_ms: 0,
        })
    };

    assert_eq!(
        super::ApiKeyState::of(dated(1000), 0),
        super::ApiKeyState::Perpetual,
        "the round 1000 two Bybit cores answer live"
    );
    assert_eq!(
        super::ApiKeyState::of(dated(365), 0),
        super::ApiKeyState::Perpetual,
        "exactly a year is already unlimited"
    );
    assert_eq!(
        super::ApiKeyState::of(dated(364), 0),
        super::ApiKeyState::Days(364),
        "one day inside the year keeps its count"
    );
}

/// An answer with a real day count but NO usable date must reach the column as that count. The
/// parser produces this shape (it zeroes the date whenever the core's timestamp is unusable while
/// still returning the count), and both wrong readings of it are silent: as "unlimited" it hides a
/// dying key behind an infinity, as "unknown" it hides the same key behind a dash.
#[test]
fn a_count_without_a_date_reaches_the_column() {
    let dateless = moon_core::session::ApiKeyExpiry {
        unlimited: false,
        known: false,
        days_left: Some(42),
        at_unix: None,
        checked_ms: 0,
    };

    assert_eq!(
        super::ApiKeyState::of(Some(dateless), 0),
        super::ApiKeyState::Days(42)
    );
    // And it ages from the receipt stamp like any other count.
    assert_eq!(
        super::ApiKeyState::of(Some(dateless), 40 * 86_400_000),
        super::ApiKeyState::Days(2)
    );
}

/// Build one core row for `group_startup` tests, varying only connection status and the startup
/// snapshot.
fn startup_row(status: ConnStatus, startup: CoreStartupStatus) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        id: 1,
        name: "Core 1".to_string(),
        status,
        sys: CoreSysStatus::default(),
        endpoint: None,
        ping_warn: false,
        exch_warn: false,
        ping_sev: crate::backend::core_warn::LatencySeverity::Normal,
        exch_sev: crate::backend::core_warn::LatencySeverity::Normal,
        api_key: crate::panels::core_status::model::ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        startup,
        time_offset: CoreTimeOffsetStatus::default(),
        server_version: None,
        version_behind: None,
    }
}

/// `group_startup`: one still-starting core among otherwise-`Ready` ones reports THAT core's
/// progress — the unfinished core must not be averaged or overwritten away by its finished
/// siblings.
#[test]
fn one_still_starting_core_reports_its_own_progress() {
    let rows = [
        startup_row(
            ConnStatus::Ready,
            CoreStartupStatus {
                state: CoreStartupState::Ready,
                elapsed_ms: 9_000,
                ..Default::default()
            },
        ),
        startup_row(
            ConnStatus::Stage("connecting…".to_string()),
            CoreStartupStatus {
                state: CoreStartupState::Connecting,
                completed_mask: 0b0000_0011,
                elapsed_ms: 3_000,
                ..Default::default()
            },
        ),
    ];

    let cell = super::group_startup(&rows);
    assert_eq!(
        cell,
        Some(super::StartupCell::Progress {
            done: 2,
            total: moon_core::session::INIT_STEPS_TOTAL,
            elapsed_ms: 3_000,
        })
    );
}

/// `group_startup`: once every core has settled, the group reports the LONGEST elapsed time any of
/// them took, not the first or the shortest.
#[test]
fn an_all_ready_group_reports_the_maximum_elapsed() {
    let rows = [
        startup_row(
            ConnStatus::Ready,
            CoreStartupStatus {
                state: CoreStartupState::Ready,
                elapsed_ms: 4_000,
                ..Default::default()
            },
        ),
        startup_row(
            ConnStatus::Ready,
            CoreStartupStatus {
                state: CoreStartupState::Ready,
                elapsed_ms: 11_000,
                ..Default::default()
            },
        ),
    ];

    let cell = super::group_startup(&rows);
    assert_eq!(cell, Some(super::StartupCell::Done { elapsed_ms: 11_000 }));
}
