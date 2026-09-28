use super::*;

#[test]
fn station_file_cores_become_servers_keyed_by_their_uid() {
    let cfg = from_station_file(
        r#"
        [[core]]
        uid = 3
        name = "BinF1"
        key = "k1"

        [[core]]
        uid = 9
        name = "Off"
        key = "k2"
        active = false
        transport = "v1"
        "#,
    )
    .expect("parses");
    assert_eq!(cfg.servers.len(), 2);
    let first = &cfg.servers[0];
    assert_eq!((first.id, first.uid, first.name.as_str()), (3, 3, "BinF1"));
    assert_eq!(first.key.expose(), "k1");
    assert!(
        first.active && first.feed.reports,
        "a new server replicates reports"
    );
    assert!(!cfg.servers[1].active);
    assert_eq!(first.transport, None);
    assert_eq!(cfg.servers[1].transport, Some(TransportVersion::V1));
}

#[test]
fn a_station_file_without_cores_is_refused() {
    assert!(from_station_file("").is_err());
}

#[test]
fn a_shared_or_zero_uid_is_refused() {
    let twice = r#"
        [[core]]
        uid = 3
        name = "A"
        key = "k1"
        [[core]]
        uid = 3
        name = "B"
        key = "k2"
    "#;
    assert!(from_station_file(twice).is_err());
    assert!(from_station_file("[[core]]\nuid = 0\nname = \"A\"\nkey = \"k\"\n").is_err());
}
