use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moon-station-tg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(PAIRING_FILE)
}

/// A pairing survives a restart as it was saved: chats, owner and the viewers' grants.
#[test]
fn a_saved_pairing_reads_back_whole() {
    let path = scratch("roundtrip");
    assert!(
        load_pairing(&path).unwrap().authorized_chat_ids.is_empty(),
        "no file yet is no pairing, not an error"
    );
    let pairing = Pairing {
        authorized_chat_ids: vec![7, 9],
        owner_chat_id: Some(7),
        chat_access: vec![TelegramChatAccess {
            chat_id: 9,
            name: "viewer".into(),
            core_uids: vec![3],
        }],
    };
    write_pairing(&path, &pairing).unwrap();
    let back = load_pairing(&path).unwrap();
    assert_eq!(back.authorized_chat_ids, vec![7, 9]);
    assert_eq!(back.owner_chat_id, Some(7));
    assert_eq!(back.chat_access, pairing.chat_access);
    assert!(
        !path.with_extension("json.new").exists(),
        "the staging file is renamed, not left behind"
    );
}

/// A damaged file is an error, never an empty pairing: an empty one would be overwritten by the
/// next pairing and every grant in the file lost.
#[test]
fn a_damaged_pairing_is_refused() {
    let path = scratch("damaged");
    std::fs::write(&path, "{ not json").unwrap();
    assert!(load_pairing(&path).is_err());
}

/// A field this binary does not know — written by a newer one before a rollback — is skipped, and
/// the pairing it knows still loads: the bot must not stay off after a rollback.
#[test]
fn a_newer_field_does_not_stop_the_bot() {
    let path = scratch("newer");
    std::fs::write(
        &path,
        r#"{"authorized_chat_ids": [7], "owner_chat_id": 7, "since_ms": 1}"#,
    )
    .unwrap();
    let pairing = load_pairing(&path).unwrap();
    assert_eq!(pairing.authorized_chat_ids, vec![7]);
    assert_eq!(pairing.owner_chat_id, Some(7));
}
