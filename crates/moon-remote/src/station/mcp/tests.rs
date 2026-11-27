//! Endpoint and configuration regressions for automatic public HTTPS.

use super::*;

/// A user-provided destination must never become a URL path, local certificate, or shell argument.
#[test]
fn public_hosts_reject_local_destinations_and_injection() {
    for host in [
        "localhost",
        "127.0.0.1",
        "10.2.3.4",
        "100.64.1.2",
        "192.0.2.1",
        "::1",
        "fd00::1",
        "2001:db8::1",
        "server.local",
        "a.b/path",
        "x.y;id",
        "a.b\nnext",
        "-a.b",
        "a..b",
    ] {
        assert!(public_origin(host).is_err(), "{host}");
    }
    assert_eq!(public_origin("8.8.8.8").unwrap().1, "https://8.8.8.8");
    assert_eq!(
        public_origin("[2606:4700:4700::1111]").unwrap().1,
        "https://[2606:4700:4700::1111]"
    );
    assert_eq!(
        public_origin("Station.Example.org").unwrap().1,
        "https://station.example.org"
    );
}

/// Setup preserves core identity and future sections, and cannot silently revoke a previous audience.
#[test]
fn setup_preserves_configuration_and_refuses_endpoint_changes() {
    let base =
        "future = [1, 2]\n[[core]]\nuid = 45\nname = 'primary'\n[telegram]\nmini_app = true\n";
    let changed = with_mcp(Some(base), "https://8.8.8.8").unwrap().unwrap();
    let parsed: toml::Table = toml::from_str(&changed).unwrap();
    let mut retained = parsed.clone();
    retained.remove("mcp");
    assert_eq!(retained, toml::from_str::<toml::Table>(base).unwrap());
    assert!(
        with_mcp(Some(&changed), "https://8.8.8.8")
            .unwrap()
            .is_none()
    );
    assert!(with_mcp(Some(&changed), "https://1.1.1.1").is_err());
    assert!(with_mcp(None, "https://8.8.8.8").is_err());
}

/// The tunnel sits beside a public origin without disturbing it, and leaving takes only what it
/// added: Claude's address and the rest of the file survive both ways.
#[test]
fn the_tunnel_joins_and_leaves_mcp_without_touching_the_rest() {
    let id = "tunnel_0123456789abcdef0123456789abcdef";
    let base = "[[core]]\nuid = 3\nname = 'BinF1'\n";
    let tunnel_only = with_tunnel(Some(base), id).unwrap().unwrap();
    let parsed: toml::Table = toml::from_str(&tunnel_only).unwrap();
    assert_eq!(parsed["mcp"]["tunnel_id"].as_str(), Some(id));
    assert!(parsed["mcp"].get("public_url").is_none());
    assert_eq!(parsed["core"][0]["uid"].as_integer(), Some(3));
    assert_eq!(
        with_tunnel(Some(&tunnel_only), id).unwrap(),
        None,
        "already there"
    );
    // Claude's setup on a tunnel-only station adds its origin beside the tunnel.
    let both = with_mcp(Some(&tunnel_only), "https://8.8.8.8")
        .unwrap()
        .unwrap();
    let parsed: toml::Table = toml::from_str(&both).unwrap();
    assert_eq!(parsed["mcp"]["tunnel_id"].as_str(), Some(id));
    assert_eq!(
        parsed["mcp"]["public_url"].as_str(),
        Some("https://8.8.8.8")
    );
    // Leaving keeps the public origin; leaving a tunnel-only section removes the section.
    let public_left: toml::Table =
        toml::from_str(&without_tunnel(Some(&both)).unwrap().unwrap()).unwrap();
    assert_eq!(
        public_left["mcp"]["public_url"].as_str(),
        Some("https://8.8.8.8")
    );
    assert!(public_left["mcp"].get("tunnel_id").is_none());
    let none_left: toml::Table =
        toml::from_str(&without_tunnel(Some(&tunnel_only)).unwrap().unwrap()).unwrap();
    assert!(none_left.get("mcp").is_none());
    assert_eq!(none_left["core"][0]["uid"].as_integer(), Some(3));
    assert_eq!(
        without_tunnel(Some(base)).unwrap(),
        None,
        "nothing to remove"
    );
}
