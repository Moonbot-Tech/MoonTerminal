//! Synthetic TESTKEY exports with test key bytes, never live credentials.
//! VALID and SAME_ENDPOINT differ in master bytes (0x11 / 0x33), both naming 198.51.100.42:4321.
//! Other fixtures change only the port to 4322 or the host to 198.51.100.43. LEGACY carries no
//! network block at all, so it has no address.

const VALID: &str = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==";
const OTHER_PORT: &str = "sX85BQAAAACAlFj0nrgLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPN1GryNcv1KZClUCEhH6mRG/Np81EodJlA==";
const OTHER_HOST: &str = "sX85BQAAAAD4HMd1j7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryBcv1KZClUCEhH6mRG/Np81EodJlA==";
const SAME_ENDPOINT: &str = "sX85BQAAAADS70YFULzT0VN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AAstnAFE0y9hHeJZPoV5f4dLrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==";
const LEGACY: &str = "sX85BQAAAAD28wZIjkZrEFN0GO03YmO97mVEnuHg11uof3oqb1aJUMaYPTx2BjfkRv9KKmfKnw8RUUyCkrE508/47Z1ns3t2NH0iKCnYuILRnc8YJKtqiHTKFozEOcpM";

use super::{endpoint_cells, key_placeholder};
use moon_core::config::{Secret, ServerConfig};

/// Build an unsaved, disconnected draft row using configuration defaults.
fn server(name: &str, key: &str) -> ServerConfig {
    let mut server: ServerConfig = serde_json::from_str(r#"{"id":0}"#).unwrap();
    server.name = name.into();
    server.key = Secret::new(key);
    server
}

/// Dropping key derivation or substituting a live endpoint leaves freshly pasted rows empty.
#[test]
fn draft_key_shows_endpoint_and_unreadable_keys_stay_empty() {
    let cells = endpoint_cells(&[
        server("new", VALID),
        server("bad", "garbage"),
        server("blank", ""),
    ]);
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<Vec<_>>(),
        ["198.51.100.42:4321", "", ""]
    );
    assert!(cells.iter().all(|cell| cell.duplicate_name.is_none()));
}

/// Comparing only IP, only port, key bytes, or only later rows marks the wrong cores.
#[test]
fn marks_exactly_all_rows_sharing_address_and_port() {
    let mut servers = vec![
        server("first", VALID),
        server("port", OTHER_PORT),
        server("host", OTHER_HOST),
        server("second", SAME_ENDPOINT),
        server("third", VALID),
        server("bad", "garbage"),
        server("blank", ""),
    ];
    servers[3].active = false;
    servers[3].group = "hidden".into();
    let cells = endpoint_cells(&servers);
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell.duplicate_name.as_deref())
            .collect::<Vec<_>>(),
        [
            Some("second"),
            None,
            None,
            Some("first"),
            Some("first"),
            None,
            None
        ]
    );
    servers[0].key = Secret::new("");
    servers.remove(4);
    let cells = endpoint_cells(&servers);
    assert!(cells.iter().all(|cell| cell.duplicate_name.is_none()));
}

/// Duplicates follow the address the row DIALS: an override that moves a row off a shared key
/// endpoint clears the mark, and one that moves it onto another row's endpoint sets it. Comparing
/// key endpoints, as before #616, would warn about the first and miss the second.
#[test]
fn duplicates_compare_the_overridden_endpoint() {
    let mut servers = vec![
        server("first", VALID),
        server("second", SAME_ENDPOINT),
        server("host", OTHER_HOST),
    ];
    servers[1].endpoint_override = "192.168.1.5".into();
    servers[2].endpoint_override = "192.168.1.5:4321".into();
    let cells = endpoint_cells(&servers);
    assert_eq!(cells[0].duplicate_name, None);
    assert_eq!(cells[1].text, "192.168.1.5:4321");
    assert_eq!(cells[1].duplicate_name.as_deref(), Some("host"));
    assert_eq!(cells[2].duplicate_name.as_deref(), Some("second"));
}

/// An override that is not an address is flagged and dials nothing, so it neither shows the key's
/// address as if it were in effect nor joins a duplicate pair.
#[test]
fn an_invalid_override_is_flagged_and_has_no_endpoint() {
    let mut servers = vec![server("first", VALID), server("second", VALID)];
    servers[1].endpoint_override = "192.168.1.300".into();
    let cells = endpoint_cells(&servers);
    assert!(cells[1].invalid);
    assert_eq!(cells[1].text, "");
    assert_eq!(cells[0].duplicate_name, None);
}

/// A key without an address dials localhost (#616): the row must know it, so it can warn instead
/// of showing `127.0.0.1` as if the key said so. A typed host ends the warning.
#[test]
fn a_key_without_an_address_is_flagged_until_a_host_is_typed() {
    let mut servers = vec![server("legacy", LEGACY), server("valid", VALID)];
    let cells = endpoint_cells(&servers);
    assert!(cells[0].localhost_fallback);
    assert_eq!(cells[0].text, "127.0.0.1:3000");
    assert!(!cells[1].localhost_fallback);
    servers[0].endpoint_override = "192.168.1.5".into();
    assert!(!endpoint_cells(&servers)[0].localhost_fallback);
}

/// The placeholder is the key's own endpoint, IPv4 or not, and nothing for a key that has none.
#[test]
fn the_placeholder_is_the_keys_endpoint() {
    assert_eq!(
        key_placeholder(VALID).as_deref(),
        Some("198.51.100.42:4321")
    );
    assert_eq!(key_placeholder("garbage"), None);
    assert_eq!(key_placeholder(""), None);
}
