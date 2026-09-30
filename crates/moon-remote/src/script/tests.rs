use super::*;

#[test]
fn a_quoted_word_survives_its_own_quotes() {
    assert_eq!(sh_quote("plain"), "'plain'");
    assert_eq!(sh_quote("it's"), r"'it'\''s'");
}

/// The scripts run under `sh` on Linux: a carriage return from a Windows checkout would turn
/// every line into a syntax error there.
#[test]
fn the_embedded_scripts_have_unix_line_ends() {
    for (name, text) in [("bootstrap", BOOTSTRAP), ("helper", HELPER), ("unit", UNIT)] {
        assert!(!text.contains('\r'), "{name} carries a CR");
    }
    assert!(
        HELPER.starts_with("#!/bin/sh\n"),
        "bootstrap checks for this line"
    );
}

#[test]
fn arguments_are_quoted_and_the_script_is_one_word() {
    let line = bootstrap("admin", &["moon"]);
    assert!(line.starts_with("sh -c '"));
    assert!(line.ends_with(" moon-bootstrap admin 'moon'"));
    assert_eq!(
        helper("put-cred", &["3"]),
        format!("sudo -n {HELPER_PATH} put-cred '3'")
    );
}

#[test]
fn values_are_read_by_key() {
    let out = "arch=x86_64\nos=ubuntu 24.04\nsystemd=255\n";
    assert_eq!(value(out, "os"), Some("ubuntu 24.04"));
    assert_eq!(value(out, "systemd"), Some("255"));
    assert_eq!(value(out, "tpm2"), None);
}

/// Treating a revoked write as an ordinary English stderr string loses localized recovery
/// advice for a Save that raced another terminal's station removal.
#[test]
fn removal_revocation_is_a_typed_failure() {
    let out = Output {
        status: Some(1),
        stdout: b"station=removed\n".to_vec(),
        stderr: b"station was removed; run setup again".to_vec(),
    };
    let error = checked(out).err().expect("revocation must fail");
    assert!(matches!(
        error.downcast_ref::<crate::station::access::RemovalError>(),
        Some(crate::station::access::RemovalError::NotConfigured)
    ));
}

/// Clearing revocation on an automatic helper refresh would let a concurrent Save recreate
/// secrets after removal. Only the explicit setup's service step may clear it.
#[test]
fn only_explicit_service_setup_clears_removal_revocation() {
    let service = BOOTSTRAP
        .split("step_service() {")
        .nth(1)
        .unwrap()
        .split("step_harden() {")
        .next()
        .unwrap();
    assert!(service.contains("rm -f \"$REMOVED\""));
    let helper = BOOTSTRAP
        .split("step_helper() {")
        .nth(1)
        .unwrap()
        .split("step_service() {")
        .next()
        .unwrap();
    assert!(!helper.contains("rm -f \"$REMOVED\""));
}
