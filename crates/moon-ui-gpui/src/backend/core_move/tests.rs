use std::net::Ipv4Addr;

use moon_core::config::{ServerConfig, TransportVersion};
use moon_core::core_move::key_packet::ConnectionPacket;
use moon_remote::core_move::plan::old_core_endpoint;

use super::old_core_target;
use super::worker::old_core_from;

/// A core row `uid` 7 holding `key`, with `address` typed into its Address field.
fn row(key: &str, address: &str) -> ServerConfig {
    let mut server: ServerConfig =
        serde_json::from_value(serde_json::json!({"id": 7, "uid": 7})).unwrap();
    server.key = moon_core::config::Secret::new(key);
    server.endpoint_override = address.into();
    server
}

/// A key exported as MoonBot does when its Moon Proto page names no address.
fn key_without_address() -> String {
    ConnectionPacket::generate(Ipv4Addr::UNSPECIFIED, 5020, TransportVersion::V0)
        .unwrap()
        .export()
        .unwrap()
}

/// The user's report: a core reached through its Address field, its key naming no address. The
/// move read the key alone and stopped with "the core's key names no address"; it must ask the
/// old core where the terminal reaches it.
#[test]
fn a_key_without_an_address_moves_from_the_rows_address() {
    let key = key_without_address();
    let packet = ConnectionPacket::parse(&key).unwrap();
    let before = old_core_endpoint(&packet, None).unwrap_err();
    assert!(
        before.to_string().contains("names no address"),
        "{before:#}"
    );

    let old = old_core_target(&[row(&key, "203.0.113.5:4510")], 7).unwrap();
    let from = old_core_from(&old).unwrap();
    assert_eq!(
        old_core_endpoint(&packet, Some(&from)).unwrap(),
        ("203.0.113.5".to_owned(), 4510)
    );
}

/// The Address replaces the key's address, as it does for the live feed: a core that moved off
/// the address its key was copied at is asked where it runs now.
#[test]
fn the_rows_address_wins_over_the_keys() {
    let key =
        ConnectionPacket::generate(Ipv4Addr::new(198, 51, 100, 9), 5020, TransportVersion::V0)
            .unwrap()
            .export()
            .unwrap();
    let from = old_core_from(&old_core_target(&[row(&key, "203.0.113.5")], 7).unwrap()).unwrap();
    assert_eq!(from, "203.0.113.5:5020");
    let from = old_core_from(&old_core_target(&[row(&key, "")], 7).unwrap()).unwrap();
    assert_eq!(from, "198.51.100.9:5020");
}

/// An Address that does not parse is named as such — the feed refuses it too — and not reported
/// as a missing one the user is told to fill in.
#[test]
fn a_broken_address_is_named_not_reported_missing() {
    let error = old_core_target(&[row(&key_without_address(), "203.0.113.5:99999")], 7)
        .unwrap_err()
        .to_string();
    assert!(error.contains("is not an address"), "{error}");
}

/// With no address anywhere the move refuses at once, naming the field to fill — not the
/// localhost the feed falls back to, where a server's core never answers.
#[test]
fn no_address_anywhere_names_the_field_to_fill() {
    let error = old_core_target(&[row(&key_without_address(), "")], 7)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Fill in the Address"), "{error}");
}

/// Same guard as `hotkeys::tests`: moon-core names the move's log target rooted at the BINARY and
/// cannot check it from its own side. A module move or a `[[bin]]` rename would mute every move
/// line under the `warn` baseline again while every gate stayed green.
#[test]
fn the_core_move_log_target_still_matches_this_module() {
    let target = moon_core::diagnostics::CORE_MOVE_TARGET;
    assert!(
        module_path!().starts_with(target),
        "the base filter raises {target:?}, but this module logs as {:?}",
        module_path!()
    );
}
