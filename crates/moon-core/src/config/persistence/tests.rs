use super::super::{AppConfig, ServerConfig};

/// Normalizing only the worker's copy would leave live IDs unassigned and reuse them on later saves.
#[test]
fn snapshot_assigns_live_ids_and_owns_the_pre_edit_config() {
    let row: ServerConfig = serde_json::from_value(serde_json::json!({
        "id": 0, "name": "first", "group": "desk",
    }))
    .unwrap();
    let mut live = AppConfig::headless(vec![row]);
    let first = live.persistence_snapshot().unwrap();
    let uid = live.servers[0].uid;
    assert_ne!(uid, 0);
    assert_eq!(first.0.servers[0].uid, uid);
    assert_eq!(first.0.next_uid.get(), live.next_uid.get());
    live.servers[0].name = "renamed".into();
    let mut added = live.servers[0].clone();
    added.uid = 0;
    added.name = "second".into();
    live.servers.push(added);
    let second = live.persistence_snapshot().unwrap();
    assert_eq!(first.0.servers.len(), 1);
    assert_eq!(first.0.servers[0].name, "first");
    assert_eq!(second.0.servers[0].name, "renamed");
    assert_eq!(second.0.servers[0].uid, uid);
    assert!(second.0.servers[1].uid > uid);
    assert_eq!(second.0.servers[1].uid, live.servers[1].uid);
}

/// Capturing an unread fallback must not grant it permission to replace the original later.
#[test]
fn unread_config_cannot_become_a_background_snapshot() {
    let mut live = AppConfig::headless(Vec::new());
    live.settings_unreadable = true;
    assert!(live.persistence_snapshot().is_err());
}
