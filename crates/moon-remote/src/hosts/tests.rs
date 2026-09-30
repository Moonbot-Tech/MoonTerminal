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
