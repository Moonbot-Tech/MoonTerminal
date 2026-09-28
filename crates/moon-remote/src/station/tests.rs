use super::*;

/// The file that goes to the server names each core and never carries its key.
#[test]
fn the_station_file_carries_no_key() {
    let cores = [
        CoreKey {
            uid: 3,
            name: "BinF \"1\"".to_owned(),
            transport: Some(TransportVersion::V1),
            key: Secret::new("SECRET-KEY-TEXT"),
        },
        CoreKey {
            uid: 9,
            name: "HL".to_owned(),
            transport: None,
            key: Secret::new("OTHER-KEY"),
        },
    ];
    let text = station_toml(&cores).unwrap();
    assert!(!text.contains("SECRET-KEY-TEXT") && !text.contains("OTHER-KEY"));
    assert!(!text.lines().any(|l| l.trim_start().starts_with("key")));

    let back: toml::Value = toml::from_str(&text).unwrap();
    let list = back["core"].as_array().unwrap();
    assert_eq!(list[0]["uid"].as_integer(), Some(3));
    assert_eq!(list[0]["name"].as_str(), Some("BinF \"1\""));
    assert_eq!(list[0]["transport"].as_str(), Some("v1"));
    assert!(list[1].get("transport").is_none());
}
