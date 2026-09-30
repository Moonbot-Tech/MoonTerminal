use super::parse_target;

#[test]
fn an_address_takes_port_22_unless_it_names_one() {
    let t = parse_target("65.20.108.232").unwrap();
    assert_eq!((t.host.as_str(), t.port), ("65.20.108.232", 22));
    let t = parse_target(" 65.20.108.232:2222 ").unwrap();
    assert_eq!((t.host.as_str(), t.port), ("65.20.108.232", 2222));
    let t = parse_target("[2001:db8::1]:2200").unwrap();
    assert_eq!((t.host.as_str(), t.port), ("2001:db8::1", 2200));
    let t = parse_target("2001:db8::1").unwrap();
    assert_eq!((t.host.as_str(), t.port), ("2001:db8::1", 22));
}

#[test]
fn an_empty_or_bad_address_is_refused() {
    assert!(parse_target("  ").is_none());
    assert!(parse_target("host:notaport").is_none());
    assert!(parse_target(":22").is_none());
}
