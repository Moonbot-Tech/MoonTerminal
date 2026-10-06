//! Unit tests for the connection-verdict wording. Owned by the breakage gate, which authors
//! every deterministic test in this repository.

use super::*;
use moon_core::config::{FeedFlags, Secret};

/// Synthetic fleet includes blank and malformed keys, stored modes and duplicate identities.
fn indexed_fleet() -> Vec<ServerConfig> {
    (0..200)
        .map(|n| {
            let mut s = server(
                n % 173,
                match n % 5 {
                    0 => None,
                    1 => Some(TransportVersion::V0),
                    2 => Some(TransportVersion::V1),
                    _ => Some(TransportVersion::V2),
                },
            );
            if n % 7 == 0 {
                s.key = Secret::new("synthetic-invalid-key");
            }
            s
        })
        .collect()
}

/// Incorrect duplicate subtraction or first-mode selection changes the original advice.
#[test]
fn fleet_mode_index_matches_fleet_mode_suggestion() {
    let mut fleet = indexed_fleet();
    for scenario in 0..5 {
        if scenario == 4 {
            for server in &mut fleet {
                server.transport = Some(TransportVersion::V1);
            }
            fleet[0].transport = Some(TransportVersion::V0);
            fleet[173].transport = Some(TransportVersion::V2);
        }
        let ready = |id| match scenario {
            0 => false,
            1 => true,
            2 => id % 3 == 0,
            _ => id != 0,
        };
        let index = FleetModeIndex::new(&fleet, ready);
        for id in 0..201 {
            assert_eq!(
                index.suggestion(id),
                fleet_mode_suggestion(id, &fleet, ready),
                "scenario={scenario} id={id}"
            );
        }
    }
    let fleet = [
        server(1, None),
        server(1, Some(TransportVersion::V0)),
        server(2, Some(TransportVersion::V1)),
    ];
    assert_eq!(FleetModeIndex::new(&fleet, |_| true).suggestion(1), None);
    let modes = [
        None,
        Some(TransportVersion::V0),
        Some(TransportVersion::V1),
        Some(TransportVersion::V2),
    ];
    for first in modes {
        for duplicate in modes {
            for sibling in modes {
                let fleet = [server(1, first), server(1, duplicate), server(2, sibling)];
                for mask in 0..4 {
                    let ready = |id| mask & (1 << (id - 1)) != 0;
                    let index = FleetModeIndex::new(&fleet, ready);
                    for id in [1, 2, 99] {
                        assert_eq!(
                            index.suggestion(id),
                            fleet_mode_suggestion(id, &fleet, ready)
                        );
                    }
                }
            }
        }
    }
}

/// Reintroducing per-query readiness scans makes fleet rebuilds quadratic.
#[test]
fn fleet_mode_index_calls_is_ready_once_per_server() {
    let fleet = indexed_fleet();
    let calls = std::cell::Cell::new(0);
    let index = FleetModeIndex::new(&fleet, |_| {
        calls.set(calls.get() + 1);
        true
    });
    for server in &fleet {
        let _ = index.suggestion(server.id);
    }
    assert_eq!(calls.get(), fleet.len());
    // No ready siblings forces the reference to inspect every resolvable configured entry.
    let fleet: Vec<_> = (0..200)
        .map(|id| server(id, Some(TransportVersion::V1)))
        .collect();
    let linear_calls = std::cell::Cell::new(0);
    let indexed_calls = std::cell::Cell::new(0);
    let index = FleetModeIndex::new(&fleet, |_| {
        indexed_calls.set(indexed_calls.get() + 1);
        false
    });
    for server in &fleet {
        let original = fleet_mode_suggestion(server.id, &fleet, |_| {
            linear_calls.set(linear_calls.get() + 1);
            false
        });
        assert_eq!(index.suggestion(server.id), original);
    }
    assert_eq!(linear_calls.get(), fleet.len() * (fleet.len() - 1));
    assert_eq!(indexed_calls.get(), fleet.len());
}

/// Measure complete 200-core rebuild advice, including snapshot construction.
#[test]
#[ignore]
fn bench_fleet_mode_200() {
    let fleet: Vec<_> = (0..200)
        .map(|id| {
            server(
                id,
                Some(if id == 0 {
                    TransportVersion::V0
                } else {
                    TransportVersion::V1
                }),
            )
        })
        .collect();
    let iterations = 2000;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let index = FleetModeIndex::new(std::hint::black_box(&fleet), |id| id != 0);
        for server in &fleet {
            std::hint::black_box(index.suggestion(server.id));
        }
    }
    println!(
        "bench_fleet_mode_200 ns/iter={}",
        start.elapsed().as_nanos() / iterations
    );
}

/// Build a server whose stored mode is the effective mode under this test's explicit fixtures.
fn server(id: CoreId, mode: Option<TransportVersion>) -> ServerConfig {
    ServerConfig {
        id,
        uid: id,
        name: format!("core-{id}"),
        active: true,
        feed: FeedFlags::default(),
        key: Secret::new(""),
        endpoint_override: String::new(),
        endpoint_to_station: false,
        group: "fleet".to_string(),
        market: "BTCUSDT".to_string(),
        color: [0, 0, 0],
        synthetic: false,
        chart_bundle: String::new(),
        default_alert_strategy: 0,
        own_trade_config: false,
        strat_slots: None,
        manual_strategy: None,
        trade: None,
        transport: mode,
        workspace_membership: moon_core::config::WorkspaceMembership::default(),
        total_mode: Default::default(),
    }
}

/// `conn_diag.rs:reason` and `fault_short` must keep the empty-key and non-key wording separate;
/// coalescing their arms sends users after the wrong correction, while the key next step must keep
/// the Copy Key route rather than the undetermined-support route.
#[test]
fn key_unparsable_wording_distinguishes_blank_from_pasted_garbage() {
    let _locale = crate::test_locale::force("en");
    let empty = FailureClass::KeyUnparsable { empty: true };
    let garbage = FailureClass::KeyUnparsable { empty: false };

    assert_ne!(reason(&empty), reason(&garbage));
    assert_ne!(fault_short(&empty), fault_short(&garbage));
    let next = next_step(&empty, None);
    assert!(next.contains("Copy Key"));
    assert!(!next.contains("support"));
}

/// Physical inbound datagrams must not reuse either the silence sentence or its firewall advice.
///
/// Breakage: keying the verdict only off accepted Sliced bytes tells an operator to open a port
/// after the socket has already proved that packets reached the terminal.
#[test]
fn unparsed_datagrams_do_not_use_the_silent_wording() {
    let _locale = crate::test_locale::force("en");
    let silent = FailureClass::NoResponse {
        packets_sent: 9,
        packets_received: 0,
        bytes: 0,
        elapsed_ms: 12_000,
    };
    let unparsed = FailureClass::NoResponse {
        packets_sent: 9,
        packets_received: 73,
        bytes: 0,
        elapsed_ms: 12_000,
    };

    assert_ne!(reason(&silent), reason(&unparsed));
    assert_ne!(next_step(&silent, None), next_step(&unparsed, None));
    assert_ne!(fault_short(&silent), fault_short(&unparsed));
    assert!(reason(&unparsed).contains("73"));
}

/// `conn_diag.rs:fleet_mode_suggestion` must compare a core with other configured cores only.
///
/// Breakage: including the failing core in its own sibling evidence would make a one-core fleet fabricate a transport recommendation from the very failure it is meant to explain.
#[test]
fn fleet_mode_suggestion_excludes_the_failing_core_and_uses_ready_siblings() {
    let alone = [server(1, Some(TransportVersion::V0))];
    assert_eq!(fleet_mode_suggestion(1, &alone, |_| true), None);
    let fleet = [
        server(1, Some(TransportVersion::V0)),
        server(2, Some(TransportVersion::V1)),
    ];
    assert_eq!(
        fleet_mode_suggestion(1, &fleet, |id| id == 2),
        Some(TransportVersion::V1)
    );
}

/// `conn_diag.rs:fleet_mode_suggestion` must not infer a mode when the failing core has none.
///
/// Breakage: using only sibling modes when this core's mode is unreadable would recommend a setting without knowing whether it differs from the failing configuration.
#[test]
fn fleet_mode_suggestion_requires_the_failing_cores_effective_mode() {
    let fleet = [server(1, None), server(2, Some(TransportVersion::V1))];
    assert_eq!(fleet_mode_suggestion(1, &fleet, |id| id == 2), None);
}

/// `conn_diag.rs:next_step` must append mode advice only to no-response verdicts.
///
/// Breakage: widening its `(FailureClass::NoResponse, Some(_))` match to every class tells users to change transport mode after a handshake that already proved transport worked.
#[test]
fn only_no_response_verdicts_receive_a_mode_suggestion() {
    let _locale = crate::test_locale::force("en");
    let silent = FailureClass::NoResponse {
        packets_sent: 3,
        packets_received: 0,
        bytes: 0,
        elapsed_ms: 1_000,
    };
    let unparsed = FailureClass::NoResponse {
        packets_sent: 3,
        packets_received: 2,
        bytes: 0,
        elapsed_ms: 1_000,
    };
    for no_response in [&silent, &unparsed] {
        assert_ne!(
            next_step(no_response, Some(TransportVersion::V1)),
            next_step(no_response, None),
            "both packet-count no-response variants must surface the proven V1 alternative"
        );
    }
    let other_classes = [
        FailureClass::KeyUnparsable { empty: true },
        FailureClass::LocalPort { attempts: 1 },
        FailureClass::Access {
            refused: true,
            message: None,
        },
        FailureClass::CoreUnidentified { message: None },
        FailureClass::Syncing {
            step: None,
            done: 1,
            total: 8,
            elapsed_ms: 1_000,
            stalled: false,
        },
        FailureClass::Aborted,
        FailureClass::Undetermined {
            raw_stage: "unknown".to_string(),
        },
    ];
    for class in &other_classes {
        assert_eq!(
            next_step(class, Some(TransportVersion::V1)),
            next_step(class, None)
        );
    }
}
