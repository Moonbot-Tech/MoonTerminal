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
