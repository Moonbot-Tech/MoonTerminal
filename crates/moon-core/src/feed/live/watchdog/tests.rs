use super::*;

/// A credential the user did not retype must stay `None` on the wire, or the core would replace
/// the saved key with whatever the terminal sent.
///
/// Mutation: map `None` to an empty string. The core then rejects every Apply that touches an
/// existing row, or worse, clears its key.
#[test]
fn untouched_credentials_stay_absent_on_the_wire() {
    let edit = WatchdogSettingsEdit {
        server_ip: " 203.0.113.9 ".to_string(),
        entries: vec![
            WatchdogEntryEdit {
                id: "core-1".to_string(),
                ..WatchdogEntryEdit::default()
            },
            WatchdogEntryEdit {
                api_secret: Some("new-secret".to_string()),
                ..WatchdogEntryEdit::default()
            },
        ],
        ..WatchdogSettingsEdit::default()
    };
    let wire = serde_json::to_value(settings_to_proto(edit)).expect("serializable");
    assert_eq!(wire["server_ip"], "203.0.113.9");
    assert!(wire["entries"][0].get("api_key").is_none());
    assert!(wire["entries"][0].get("api_secret").is_none());
    assert_eq!(wire["entries"][0]["id"], "core-1");
    assert_eq!(wire["entries"][1]["api_secret"], "new-secret");
    assert_eq!(wire["entries"][1]["id"], "");
}

/// The request id arrives as a decimal string; the full `u64` range must survive the trip.
///
/// Mutation: parse as `i64`. A request id above `i64::MAX` — half of all random ids — would then
/// never settle its banner.
#[test]
fn request_ids_parse_across_the_whole_u64_range() {
    let raw = serde_json::json!({
        "domain": "watchdog",
        "action": "apply",
        "request_id": u64::MAX.to_string(),
        "applied": true,
        "error": "",
        "state": { "revision": 1 }
    });
    let result: ServiceSettingsResult = serde_json::from_value(raw).expect("valid result");
    let projected = service_result_from_proto(&result);
    assert_eq!(projected.request_id, Some(u64::MAX));
    assert_eq!(projected.domain, ServiceDomain::Watchdog);
    assert!(projected.applied);
}
