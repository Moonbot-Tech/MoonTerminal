//! Synthetic TESTKEY exports with test key bytes, never live credentials.
//! VALID and SAME_ENDPOINT differ in master bytes (0x11 / 0x33), both naming 198.51.100.42:4321.
//! Other fixtures change only the port to 4322 or the host to 198.51.100.43.

const VALID: &str = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==";
const OTHER_PORT: &str = "sX85BQAAAACAlFj0nrgLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPN1GryNcv1KZClUCEhH6mRG/Np81EodJlA==";
const OTHER_HOST: &str = "sX85BQAAAAD4HMd1j7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryBcv1KZClUCEhH6mRG/Np81EodJlA==";
const SAME_ENDPOINT: &str = "sX85BQAAAADS70YFULzT0VN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AAstnAFE0y9hHeJZPoV5f4dLrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==";

use super::{endpoint_cells, endpoint_text};
use moon_core::config::{Secret, ServerConfig};
use moon_core::feed::CoreEndpoint;

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

/// Omitting brackets would make IPv6 address and port boundaries ambiguous to the user.
#[test]
fn ipv6_keeps_the_port_unambiguous() {
    assert_eq!(
        endpoint_text(Some(CoreEndpoint {
            address: "2001:db8::1".parse().unwrap(),
            port: 4321,
        })),
        "[2001:db8::1]:4321"
    );
}
