use super::*;

/// `mb --status` of an L6 core logged in to Hyperliquid (mbl-deb13, 2026-11-02).
const L6_READY: &str = r#"{"ok":true,"pid":37745,"state":"ready","exchange":"hyperliquid","exchange_name":"Hyper","has_api_keys":true,"rsa":false,"login_error":"","moonproto_enabled":true,"moonproto_listening":true,"moonproto_error":"","moonproto_port":5170}"#;

fn status_of(json: &str) -> CoreStatus {
    serde_json::from_value(parse_reply(true, json, "").unwrap()).unwrap()
}

#[test]
fn an_l6_status_reads_as_connectable_without_an_emulator_field() {
    let status = status_of(L6_READY);
    assert!(status.connectable());
    assert_eq!(status.exchange, "hyperliquid");
    assert_eq!(status.moonproto_port, 5170);
    assert_eq!(status.emulator, None);
}

#[test]
fn a_newer_core_reports_its_emulator_mode() {
    let json = L6_READY.replace("\"pid\"", "\"emulator\":true,\"pid\"");
    assert_eq!(status_of(&json).emulator, Some(true));
}

#[test]
fn ready_without_a_listener_is_not_connectable() {
    let json = L6_READY.replace(
        "\"moonproto_listening\":true",
        "\"moonproto_listening\":false",
    );
    let status = status_of(&json);
    assert!(!status.connectable());
    assert!(!status.needs_user());
}

#[test]
fn a_login_error_needs_the_user_and_an_unknown_state_is_kept() {
    let refused = L6_READY.replace("\"ready\"", "\"login_error\"");
    assert!(status_of(&refused).needs_user());
    let newer = status_of(&L6_READY.replace("\"ready\"", "\"migrating\""));
    assert_eq!(newer.state, CoreState::Other("migrating".into()));
    assert!(!newer.needs_user() && !newer.connectable());
}

#[test]
fn the_exchange_list_keeps_the_required_passphrase() {
    let reply = parse_reply(
        true,
        r#"{"ok":true,"exchanges":[{"id":"okx","name":"OKX","required_passphrase":"Okx!00000","rsa_supported":false},{"id":"binance","name":"Binance","required_passphrase":"","rsa_supported":true}]}"#,
        "",
    )
    .unwrap();
    let list = parse_exchanges(reply).unwrap();
    assert_eq!(list[0].required_passphrase, "Okx!00000");
    assert!(list[1].rsa_supported);
}

#[test]
fn a_refusal_carries_the_cores_own_words() {
    let err = parse_reply(
        false,
        r#"{"ok":false,"error":"The core command channel is unavailable."}"#,
        "",
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("command channel is unavailable"));
}

#[test]
fn an_exit_failure_is_a_refusal_even_with_ok_true() {
    assert!(parse_reply(false, L6_READY, "").is_err());
}

#[test]
fn an_unreadable_reply_never_quotes_stdout() {
    // A truncated `--setup-moonproto` reply carries the connection key.
    let err = parse_reply(true, "{\"ok\":true,\"key\":\"SECRETPACKET", "").unwrap_err();
    assert!(!format!("{err:#}").contains("SECRETPACKET"));
}

#[test]
fn the_reply_is_the_last_json_line() {
    let reply = parse_reply(true, "Installing…\n{\"ok\":true,\"key\":\"K\"}\n", "").unwrap();
    assert_eq!(reply["key"], "K");
}

#[test]
fn keys_go_as_the_json_the_core_reads() {
    let pair = ApiKeys::Pair {
        api_key: Zeroizing::new(" k1 ".into()),
        api_secret: Zeroizing::new("s\"2\n".into()),
    };
    let json: serde_json::Value = serde_json::from_slice(&pair.json()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"api_key": "k1", "api_secret": "s\"2"})
    );
    let rsa = ApiKeys::Rsa {
        bundle: Zeroizing::new("B".into()),
    };
    let json: serde_json::Value = serde_json::from_slice(&rsa.json()).unwrap();
    assert_eq!(json, serde_json::json!({"rsa_bundle": "B"}));
}
