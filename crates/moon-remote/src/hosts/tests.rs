use super::*;

#[test]
fn a_pin_is_kept_and_a_different_key_is_refused() {
    let mut hosts = Hosts::default();
    hosts.pin("h:22", "SHA256:a").unwrap();
    hosts.pin("h:22", "SHA256:a").unwrap();
    assert!(hosts.pin("h:22", "SHA256:b").is_err());
    assert_eq!(hosts.get("h:22").unwrap().fingerprint, "SHA256:a");
}

#[test]
fn the_admin_is_recorded_only_for_a_pinned_host() {
    let mut hosts = Hosts::default();
    assert!(hosts.set_admin("h:22", "moon").is_err());
    hosts.pin("h:22", "SHA256:a").unwrap();
    hosts.set_admin("h:22", "moon").unwrap();

    let text = toml::to_string_pretty(&hosts).unwrap();
    let back: Hosts = toml::from_str(&text).unwrap();
    assert_eq!(back.get("h:22").unwrap().admin.as_deref(), Some("moon"));
}

/// Replacing this migration with forget+pin loses the administrator and locks out a hardened
/// server. The serialized record must still support the same key login at the new address.
#[test]
fn changing_address_keeps_the_verified_administrator() {
    let mut hosts = Hosts::default();
    hosts.pin("old:22", "SHA256:old").unwrap();
    hosts.set_admin("old:22", "moon-admin").unwrap();
    let source = hosts.get("old:22").unwrap().clone();
    hosts
        .change_address(&source, "new:2222", "SHA256:confirmed")
        .unwrap();
    let back: Hosts = toml::from_str(&toml::to_string_pretty(&hosts).unwrap()).unwrap();
    assert!(back.get("old:22").is_none());
    let moved = back.first_set_up().unwrap();
    assert_eq!(moved.addr, "new:2222");
    assert_eq!(moved.admin.as_deref(), Some("moon-admin"));
    assert_eq!(moved.fingerprint, "SHA256:confirmed");
}

/// Removing the source/collision checks lets stale consent overwrite another host's pin or
/// administrator. Every refusal must preserve the entire durable host list.
#[test]
fn stale_or_colliding_address_changes_do_not_mutate_hosts() {
    let mut hosts = Hosts::default();
    hosts.pin("old:22", "SHA256:old").unwrap();
    hosts.set_admin("old:22", "moon-admin").unwrap();
    hosts.pin("other:22", "SHA256:other").unwrap();
    let source = hosts.get("old:22").unwrap().clone();
    let before = toml::to_string(&hosts).unwrap();
    for (addr, pin) in [
        ("old:22", "SHA256:new"),
        ("other:22", "SHA256:new"),
        ("new:22", ""),
    ] {
        assert!(hosts.change_address(&source, addr, pin).is_err());
        assert_eq!(toml::to_string(&hosts).unwrap(), before);
    }
    hosts.set_admin("old:22", "different-admin").unwrap();
    let before = toml::to_string(&hosts).unwrap();
    assert!(
        hosts
            .change_address(&source, "new:22", "SHA256:new")
            .is_err()
    );
    assert_eq!(toml::to_string(&hosts).unwrap(), before);
}

/// Matching the whole address or ignoring admin would select the wrong login for a core connection.
#[test]
fn admin_lookup_matches_ip_across_ports_and_requires_an_admin() {
    let mut hosts = Hosts::default();
    hosts.pin("203.0.113.10:22", "SHA256:a").unwrap();
    hosts.pin("203.0.113.10:2222", "SHA256:b").unwrap();
    hosts.set_admin("203.0.113.10:2222", "moon-admin").unwrap();
    hosts.pin("203.0.113.20:22", "SHA256:c").unwrap();

    assert_eq!(
        hosts.app_login_by_ip("203.0.113.10").unwrap().addr,
        "203.0.113.10:2222"
    );
    assert!(hosts.app_login_by_ip("203.0.113.20").is_none());
}

/// A core server closed after a password login is reached by its proven login, yet it must never
/// read as the station's server: `first_set_up` picking it would send station work to a core box.
/// A file written before the field existed loads with no such login.
#[test]
fn a_core_servers_key_login_is_found_but_never_taken_for_the_station() {
    let old: Hosts =
        toml::from_str("[[host]]\naddr = \"198.51.100.4:22\"\nfingerprint = \"SHA256:k\"\n")
            .unwrap();
    assert_eq!(old.get("198.51.100.4:22").unwrap().key_login, None);

    let mut hosts = old;
    hosts.set_key_login("198.51.100.4:22", "root").unwrap();
    let back: Hosts = toml::from_str(&toml::to_string_pretty(&hosts).unwrap()).unwrap();
    let found = back.app_login_by_ip("198.51.100.4").unwrap();
    assert_eq!(found.login_user(), Some("root"));
    assert!(back.first_set_up().is_none());
    assert!(
        back.check_address(found, "198.51.100.9:22").is_err(),
        "the station's address change must not adopt a core server"
    );
}
