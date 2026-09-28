use super::*;

/// A credentials directory holding `(uid, key)` pairs, as systemd would lay it out.
fn creds(name: &str, keys: &[(u64, &str)]) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("moon-station-creds-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (uid, key) in keys {
        std::fs::write(dir.join(format!("core-{uid}")), key).unwrap();
    }
    dir
}

#[test]
fn station_file_cores_become_servers_keyed_by_their_uid() {
    let dir = creds("uid", &[(3, "k1\n")]);
    let cfg = from_station_file(
        r#"
        [[core]]
        uid = 3
        name = "BinF1"

        [[core]]
        uid = 9
        name = "Off"
        active = false
        transport = "v1"
        "#,
        Some(&dir),
    )
    .expect("parses");
    assert_eq!(cfg.servers.len(), 2);
    let first = &cfg.servers[0];
    assert_eq!((first.id, first.uid, first.name.as_str()), (3, 3, "BinF1"));
    assert_eq!(
        first.key.expose(),
        "k1",
        "the credential's trailing newline is not the key"
    );
    assert!(
        first.active && first.feed.reports,
        "a new server replicates reports"
    );
    assert!(
        !first.feed.log && !first.feed.orders && !first.feed.strategies && !first.feed.detects,
        "the station reads reports alone: no core log reaches its disk"
    );
    assert!(!cfg.servers[1].active);
    assert!(
        cfg.servers[1].key.is_empty(),
        "an inactive core needs no credential"
    );
    assert_eq!(first.transport, None);
    assert_eq!(cfg.servers[1].transport, Some(TransportVersion::V1));
}

#[test]
fn a_station_file_without_cores_is_refused() {
    assert!(from_station_file("", None).is_err());
}

#[test]
fn a_shared_or_zero_uid_is_refused() {
    let dir = creds("shared", &[(3, "k1")]);
    let twice = r#"
        [[core]]
        uid = 3
        name = "A"
        [[core]]
        uid = 3
        name = "B"
    "#;
    assert!(from_station_file(twice, Some(&dir)).is_err());
    assert!(from_station_file("[[core]]\nuid = 0\nname = \"A\"\n", Some(&dir)).is_err());
}

/// A key in the plain file is the one thing this layout forbids: refused, never read.
#[test]
fn a_key_in_the_station_file_is_refused() {
    let dir = creds("inline", &[(3, "k1")]);
    let err = from_station_file(
        "[[core]]\nuid = 3\nname = \"A\"\nkey = \"plain\"\n",
        Some(&dir),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("key"), "{err:#}");
}

#[test]
fn an_active_core_without_its_credential_is_refused() {
    let dir = creds("missing", &[]);
    let text = "[[core]]\nuid = 3\nname = \"A\"\n";
    assert!(from_station_file(text, Some(&dir)).is_err());
    assert!(
        from_station_file(text, None).is_err(),
        "no credentials directory at all"
    );
}
