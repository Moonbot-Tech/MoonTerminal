//! Synthetic TESTKEY exports with test key bytes, never live credentials.
//! Frozen MoonProto V1 and legacy containers carry master 0x11 / MAC 0x22 bytes; V1 names
//! documentation address 198.51.100.42:4321. Expected values do not call the fixture encoder.

const VALID: &str = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==";
const LEGACY: &str = "sX85BQAAAAD28wZIjkZrEFN0GO03YmO97mVEnuHg11uof3oqb1aJUMaYPTx2BjfkRv9KKmfKnw8RUUyCkrE508/47Z1ns3t2NH0iKCnYuILRnc8YJKtqiHTKFozEOcpM";

use super::{CoreTarget, target_from_key, target_from_network};
use crate::config::endpoint_override::{CoreHost, parse_endpoint_override};
use moonproto::{ImportedIpVersion, ImportedNetworkConfig, TransportMode};

/// Parse an override that must be valid.
fn over(text: &str) -> crate::config::endpoint_override::EndpointOverride {
    parse_endpoint_override(text).unwrap().unwrap()
}

/// Replacing parsed network metadata with defaults would display and connect to the wrong core.
#[test]
fn pasted_key_preserves_address_and_port() {
    let target = target_from_key(VALID, None).expect("synthetic key");
    assert_eq!(target.text(), "198.51.100.42:4321");
    assert!(!target.localhost_fallback);
}

/// Returning a fallback for invalid keys would invent endpoints for incomplete paste input, and an
/// override must not stand in for a key it cannot authenticate with.
#[test]
fn unreadable_keys_have_no_target() {
    let lan = over("192.168.1.5:5020");
    for key in ["", " ", "not-a-key", "aGVsbG8="] {
        assert_eq!(target_from_key(key, None), None);
        assert_eq!(target_from_key(key, Some(&lan)), None);
    }
}

/// Removing legacy or zero-value defaults would change the established live connection target,
/// and losing the flag would hide the "no address in the key" warning (#616).
#[test]
fn legacy_and_zero_network_keep_live_defaults_and_say_so() {
    let legacy = target_from_key(LEGACY, None).expect("synthetic legacy key");
    assert_eq!(legacy.text(), "127.0.0.1:3000");
    assert!(legacy.localhost_fallback);
    let network = ImportedNetworkConfig {
        ip_version: ImportedIpVersion::V4,
        address: None,
        port: 0,
        transport_mode: TransportMode::V2,
    };
    assert_eq!(target_from_network(Some(&network), None), legacy);
}

/// Each part of the override replaces only its own half of the key's endpoint.
#[test]
fn override_replaces_only_the_parts_it_names() {
    let both = target_from_key(VALID, Some(&over("192.168.1.5:5020"))).unwrap();
    assert_eq!(both.text(), "192.168.1.5:5020");
    let host = target_from_key(VALID, Some(&over("192.168.1.5"))).unwrap();
    assert_eq!(host.text(), "192.168.1.5:4321");
    let port = target_from_key(VALID, Some(&over(":5020"))).unwrap();
    assert_eq!(port.text(), "198.51.100.42:5020");
    assert!(![both, host, port].iter().any(|t| t.localhost_fallback));
}

/// A key without an address stops being a localhost fallback once a host is typed, but a port-only
/// override leaves the missing address missing.
#[test]
fn override_host_clears_the_fallback_but_a_port_does_not() {
    let host = target_from_key(LEGACY, Some(&over("core.example.net"))).unwrap();
    assert_eq!(host.text(), "core.example.net:3000");
    assert!(!host.localhost_fallback);
    let port = target_from_key(LEGACY, Some(&over(":5020"))).unwrap();
    assert_eq!(port.text(), "127.0.0.1:5020");
    assert!(port.localhost_fallback);
}

/// A literal resolves without the resolver and reaches MoonProto as the literal, IPv6 unbracketed:
/// MoonProto adds its own `:port`.
#[test]
fn literal_hosts_resolve_to_themselves() {
    let v6 = CoreTarget {
        host: CoreHost::Ip("2001:db8::1".parse().unwrap()),
        port: 5020,
        localhost_fallback: false,
    };
    let resolved = v6.resolve().unwrap();
    assert_eq!(resolved.endpoint.address.to_string(), "2001:db8::1");
    assert_eq!(resolved.endpoint.port, 5020);
    assert_eq!(resolved.client_host, "2001:db8::1");
    assert_eq!(v6.text(), "[2001:db8::1]:5020");
}

/// `localhost` is the one name every resolver answers offline. If it comes back IPv4-only the name
/// itself goes to MoonProto (re-resolved on every rebind); if it also has an IPv6 answer the IPv4
/// literal does, so MoonProto never binds the wrong socket family.
#[test]
fn a_name_resolves_to_an_ipv4_address_first() {
    let target = CoreTarget {
        host: CoreHost::Name("localhost".into()),
        port: 5020,
        localhost_fallback: false,
    };
    let resolved = target.resolve().expect("localhost resolves");
    assert!(resolved.endpoint.address.is_loopback());
    assert_eq!(resolved.endpoint.port, 5020);
    if resolved.endpoint.address.is_ipv4() {
        assert!(
            resolved.client_host == "localhost"
                || resolved.client_host == resolved.endpoint.address.to_string()
        );
    }
}
