//! Regression coverage for port selection and evidence-based failure routing.

use super::{FailScreen, Host, Step, firewall_command, parse_port, pick_failure, port_mismatch, rail_index};
use moon_core::{config::{CoreHost, CoreTarget}, feed::FailureClass};

/// A documentation-only target, never dialled by these tests.
fn target(port: u16, localhost_fallback: bool) -> CoreTarget {
    CoreTarget { host: CoreHost::Ip("203.0.113.24".parse().expect("test IP")), port, localhost_fallback }
}

/// The rule name and UDP port must follow both factory and custom selections.
#[test]
fn firewall_rules_use_selected_port() {
    assert_eq!(firewall_command(3000), "netsh advfirewall firewall add rule name=\"MoonBot-UDP-3000\" dir=in action=allow protocol=UDP localport=3000");
    assert_eq!(firewall_command(5010), "netsh advfirewall firewall add rule name=\"MoonBot-UDP-5010\" dir=in action=allow protocol=UDP localport=5010");
}

/// Reject malformed and out-of-range ports while preserving whitespace tolerance.
#[test]
fn ports_are_nonzero_u16_values() {
    for invalid in ["0", "65536", "", "30a"] {
        assert_eq!(parse_port(invalid), None, "{invalid}");
    }
    assert_eq!(parse_port(" 3000 "), Some(3000));
    assert_eq!(parse_port("1"), Some(1));
    assert_eq!(parse_port("65535"), Some(65535));
}

/// Report the actual key port rather than the user's selected port.
#[test]
fn mismatch_returns_key_port() {
    assert_eq!(port_mismatch(3000, &target(5010, false)), Some(5010));
    assert_eq!(port_mismatch(3000, &target(3000, false)), None);
}

/// Packet-level and payload-level evidence both prevent the claim of total silence.
#[test]
fn silence_needs_zero_packets_and_bytes() {
    for (packets_received, bytes, silent) in [(0, 0, true), (1, 0, false), (0, 1, false), (1, 1, false)] {
        let class = FailureClass::NoResponse { packets_sent: 2, packets_received, bytes, elapsed_ms: 60000 };
        assert_eq!(pick_failure(Some(&class), None, Host::WindowsServer), FailScreen::NoResponse { silent });
    }
}

/// Every non-network class must keep its own evidence instead of guessing a closed port.
#[test]
fn all_diagnosis_classes_route_honestly() {
    let fallback = [
        FailureClass::KeyUnparsable { empty: true },
        FailureClass::Endpoint { unresolved: false },
        FailureClass::LocalPort { attempts: 2 },
        FailureClass::CoreUnidentified { message: None },
        FailureClass::Syncing { step: None, done: 0, total: 1, elapsed_ms: 1, stalled: true },
        FailureClass::Aborted,
        FailureClass::Undetermined { raw_stage: "stage".into() },
    ];
    for class in fallback {
        assert_eq!(pick_failure(Some(&class), None, Host::WindowsServer), FailScreen::Fallback, "{class:?}");
    }
    for refused in [false, true] {
        let class = FailureClass::Access { refused, message: None };
        assert_eq!(pick_failure(Some(&class), None, Host::WindowsServer), FailScreen::KeyRefused);
    }
    assert_eq!(pick_failure(None, None, Host::WindowsServer), FailScreen::Fallback);
}

/// Missing-address evidence wins even over access failure, but only for a remote host.
#[test]
fn missing_address_depends_on_host_and_precedes_diagnosis() {
    let key = target(3000, true);
    let access = FailureClass::Access { refused: true, message: None };
    assert_eq!(pick_failure(Some(&access), Some(&key), Host::WindowsServer), FailScreen::NoAddress);
    assert_eq!(pick_failure(None, Some(&key), Host::WindowsServer), FailScreen::NoAddress);
    assert_eq!(pick_failure(None, Some(&key), Host::ThisComputer), FailScreen::Fallback);
    assert_eq!(pick_failure(None, Some(&target(3000, false)), Host::WindowsServer), FailScreen::Fallback);
    let local = CoreTarget {
        host: CoreHost::Ip(std::net::Ipv4Addr::LOCALHOST.into()),
        port: 3000,
        localhost_fallback: true,
    };
    assert_eq!(pick_failure(None, Some(&local), Host::ThisComputer), FailScreen::Fallback);
}

/// Readiness and added rows override stale instruction steps.
#[test]
fn rail_tracks_instructions_attempt_and_ready() {
    for (step, expected) in [(Step::Port, 1), (Step::Moonbot, 2), (Step::Key, 3)] {
        assert_eq!(rail_index(step, false, false), expected);
        assert_eq!(rail_index(step, true, false), 3);
        assert_eq!(rail_index(step, true, true), 4);
        assert_eq!(rail_index(step, false, true), 4);
    }
}

/// Explicit loopback exports are missing remote addresses too, while local cores remain valid.
#[test]
fn explicit_loopback_is_remote_only_missing_address() {
    for host in [CoreHost::Ip("127.0.0.1".parse().unwrap()), CoreHost::Ip("::1".parse().unwrap()), CoreHost::Name("localhost".into())] {
        let key = CoreTarget { host, port: 3000, localhost_fallback: false };
        assert_eq!(pick_failure(None, Some(&key), Host::WindowsServer), FailScreen::NoAddress);
        assert_eq!(pick_failure(None, Some(&key), Host::ThisComputer), FailScreen::Fallback);
    }
}

/// Editing either the key or the chosen port requires a new explicit mismatch decision.
#[test]
fn mismatch_acceptance_is_bound_to_both_inputs() {
    let key = target(5010, false);
    let accepted = Some(("export A".to_owned(), 3000));
    assert_eq!(super::pending_mismatch(accepted.as_ref(), "export A", 3000, Some(&key)), None);
    assert_eq!(super::pending_mismatch(accepted.as_ref(), "export B", 3000, Some(&key)), Some(5010));
    assert_eq!(super::pending_mismatch(accepted.as_ref(), "export A", 4000, Some(&key)), Some(5010));
}
