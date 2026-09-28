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
