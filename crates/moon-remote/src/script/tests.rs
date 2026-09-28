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
