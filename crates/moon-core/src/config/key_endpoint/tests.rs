//! Synthetic TESTKEY exports with test key bytes, never live credentials.
//! Frozen MoonProto V1 and legacy containers carry master 0x11 / MAC 0x22 bytes; V1 names
//! documentation address 198.51.100.42:4321. Expected values do not call the fixture encoder.

const VALID: &str = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==";
const LEGACY: &str = "sX85BQAAAAD28wZIjkZrEFN0GO03YmO97mVEnuHg11uof3oqb1aJUMaYPTx2BjfkRv9KKmfKnw8RUUyCkrE508/47Z1ns3t2NH0iKCnYuILRnc8YJKtqiHTKFozEOcpM";

use super::{endpoint_from_key, endpoint_from_network};
use moonproto::{ImportedIpVersion, ImportedNetworkConfig, TransportMode};

/// Replacing parsed network metadata with defaults would display and connect to the wrong core.
#[test]
fn pasted_key_preserves_address_and_port() {
    let endpoint = endpoint_from_key(VALID).expect("synthetic key");
    assert_eq!(endpoint.address.to_string(), "198.51.100.42");
    assert_eq!(endpoint.port, 4321);
}

/// Returning a fallback for invalid keys would invent endpoints for incomplete paste input.
#[test]
fn unreadable_keys_have_no_endpoint() {
    for key in ["", " ", "not-a-key", "aGVsbG8="] {
        assert_eq!(endpoint_from_key(key), None);
    }
}

/// Removing legacy or zero-value defaults would change the established live connection target.
#[test]
fn legacy_and_zero_network_keep_live_defaults() {
    let legacy = endpoint_from_key(LEGACY).expect("synthetic legacy key");
    assert_eq!(legacy.address.to_string(), "127.0.0.1");
    assert_eq!(legacy.port, 3000);
    let network = ImportedNetworkConfig {
        ip_version: ImportedIpVersion::V4,
        address: None,
        port: 0,
        transport_mode: TransportMode::V2,
    };
    assert_eq!(endpoint_from_network(Some(&network)), legacy);
}
