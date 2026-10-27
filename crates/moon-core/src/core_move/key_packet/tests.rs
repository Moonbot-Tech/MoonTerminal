use std::net::{IpAddr, Ipv4Addr};

use moonproto::{TransportMode, parse_key_info};

use super::*;

const VPS: Ipv4Addr = Ipv4Addr::new(45, 32, 41, 189);

#[test]
fn a_generated_packet_reads_back_through_moonproto() {
    let packet = ConnectionPacket::generate(VPS, 5150, TransportVersion::V2).unwrap();
    let text = packet.export().unwrap();
    let info = parse_key_info(&text).expect("moonproto must accept the packet");
    let network = info.network.expect("a V1 packet carries its endpoint");
    assert_eq!(network.address, Some(IpAddr::V4(VPS)));
    assert_eq!(network.port, 5150);
    assert_eq!(network.transport_mode, TransportMode::V2);
}

#[test]
fn moving_a_key_keeps_the_key_and_changes_only_the_endpoint() {
    let original =
        ConnectionPacket::generate(Ipv4Addr::new(10, 0, 0, 1), 3000, TransportVersion::V1)
            .unwrap()
            .export()
            .unwrap();
    let moved = ConnectionPacket::parse(&original)
        .unwrap()
        .with_endpoint(VPS, 5151)
        .export()
        .unwrap();

    let before = parse_key_info(&original).unwrap();
    let after = parse_key_info(&moved).unwrap();
    assert_eq!(after.keys.master_key, before.keys.master_key);
    assert_eq!(after.keys.mac_key, before.keys.mac_key);
    // The name MoonBot shows next to a key comes from the container: unchanged means byte-exact.
    assert_eq!(after.display_name, before.display_name);
    let network = after.network.unwrap();
    assert_eq!(network.address, Some(IpAddr::V4(VPS)));
    assert_eq!(network.port, 5151);
    assert_eq!(network.transport_mode, TransportMode::V1);
    assert!(
        ConnectionPacket::parse(&moved)
            .unwrap()
            .same_key(&ConnectionPacket::parse(&original).unwrap())
    );
}

#[test]
fn a_container_whose_own_hash_fails_is_refused() {
    let mut packet = ConnectionPacket::generate(VPS, 3000, TransportVersion::V0).unwrap();
    packet.container[40] ^= 0xFF;
    let text = packet.export().unwrap();
    // The outer checksum is fresh, so moonproto still reads it; a core would refuse the key.
    assert!(parse_key_info(&text).is_some());
    assert!(ConnectionPacket::parse(&text).is_none());
}

#[test]
fn a_legacy_packet_parses_without_an_endpoint() {
    let source = ConnectionPacket::generate(VPS, 3000, TransportVersion::V0).unwrap();
    let mut plain = Vec::with_capacity(LEGACY_PLAIN);
    plain.extend_from_slice(&7_i64.to_le_bytes());
    plain.extend_from_slice(&source.container);
    let checksum = checksum_w(&plain);
    let stamp = 123_456_i64;
    encode_buffer(&mut plain, &password(OLD_PWD_HEAD, stamp));
    let mut raw = stamp.to_le_bytes().to_vec();
    raw.extend_from_slice(&checksum.to_le_bytes());
    raw.extend_from_slice(&plain);
    let text = base64::engine::general_purpose::STANDARD.encode(raw);

    assert!(
        parse_key_info(&text).is_some(),
        "moonproto reads the legacy form"
    );
    let parsed = ConnectionPacket::parse(&text).unwrap();
    assert_eq!(parsed.port, 0);
    assert_eq!(parsed.ipv4, None);
    assert!(parsed.same_key(&source));
}

#[test]
fn garbage_is_not_a_packet() {
    assert!(ConnectionPacket::parse("").is_none());
    assert!(ConnectionPacket::parse("not base64 at all!").is_none());
    assert!(ConnectionPacket::parse(&"A".repeat(200)).is_none());
}
