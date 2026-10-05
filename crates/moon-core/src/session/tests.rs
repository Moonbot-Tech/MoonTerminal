//! What a change to a server's configuration must, and must not, cost it its connection.

use super::conn_sig;
use crate::config::{ServerConfig, TransportVersion};

/// Build a server carrying every serde default, with the given transport mode.
fn server(transport: Option<TransportVersion>) -> ServerConfig {
    ServerConfig {
        id: 1,
        uid: 1,
        transport,
        ..toml::from_str("id = 0").expect("ServerConfig must deserialize from defaults")
    }
}

/// The transport mode is chosen when `ClientConfig` is built, so a live feed thread cannot adopt a
/// new one. `SessionManager::reconcile` respawns a core only when this signature moves — leave the
/// mode out of it and the Settings dropdown changes nothing until the next restart, which is
/// exactly the "I changed it and nothing happened" the control exists to avoid.
#[test]
fn changing_the_transport_mode_requires_a_reconnect() {
    let base = conn_sig(&server(None));
    let v1 = conn_sig(&server(Some(TransportVersion::V1)));
    let v2 = conn_sig(&server(Some(TransportVersion::V2)));

    assert_ne!(base, v1, "pinning a mode must restart the feed thread");
    assert_ne!(v1, v2, "switching between modes must restart it too");
}

/// The endpoint override is dialed when `ClientConfig` is built, exactly like the transport mode:
/// left out of the signature, a typed LAN address would change nothing until the next restart.
#[test]
fn changing_the_endpoint_override_requires_a_reconnect() {
    let mut lan = server(None);
    lan.endpoint_override = "192.168.1.5".to_string();
    assert_ne!(conn_sig(&server(None)), conn_sig(&lan));
}

/// The signature follows what the override DIALS, not its spelling: retyping a host name in
/// another case or with stray spaces must not drop a live connection, while a different port must.
#[test]
fn respelling_the_same_endpoint_override_keeps_the_connection() {
    let with = |text: &str| {
        let mut s = server(None);
        s.endpoint_override = text.to_string();
        conn_sig(&s)
    };
    assert_eq!(
        with("Core.Example.net:5017"),
        with("  core.example.net:5017 ")
    );
    assert_eq!(with(""), with("   "));
    assert_ne!(with("core.example.net:5017"), with("core.example.net:5018"));
}

/// The other half: a signature that moved for a presentation field would reconnect every core on
/// an unrelated edit. Colour is the cheapest witness that the hash still covers only connection
/// inputs.
#[test]
fn a_presentation_field_does_not_reconnect() {
    let mut recoloured = server(Some(TransportVersion::V1));
    recoloured.color = [1, 2, 3];
    recoloured.name = "renamed".to_string();

    assert_eq!(
        conn_sig(&server(Some(TransportVersion::V1))),
        conn_sig(&recoloured),
        "name and colour are updated in place, never by reconnecting"
    );
}
