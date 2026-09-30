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

/// Removing the shared progress child from either tab hides validation errors and job results
/// where its buttons are pressed. This binary crate's rendering wiring is checked as source.
#[test]
fn station_and_telegram_tabs_both_show_the_shared_job_result() {
    let telegram = include_str!("../../telegram.rs");
    let bot_sections = telegram
        .find(".children(self.server_bot_sections(cx))")
        .unwrap();
    let local_toggle = telegram[bot_sections..]
        .find(".child(self.server_bot_local_toggle(cx))")
        .unwrap();
    assert!(
        telegram[bot_sections..bot_sections + local_toggle]
            .contains(".child(self.server_bot_progress(cx))")
    );
    let station = include_str!("../server_bot.rs");
    let section = station
        .split("fn server_bot_section(")
        .nth(1)
        .unwrap()
        .split("fn server_bot_field(")
        .next()
        .unwrap();
    assert!(section.contains("s.child(self.server_bot_progress(cx))"));
}
