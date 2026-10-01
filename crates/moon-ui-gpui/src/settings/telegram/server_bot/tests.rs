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

/// The Station tab's version line: behind a newer known release (or this terminal's own) offers
/// that version, at or past the newest says so, and an unread or development service is unknown.
#[test]
fn the_service_version_is_behind_current_or_unknown() {
    use super::{ServiceVersion, service_version};
    use moon_core::update::ReleaseVersion;
    let v = |tag: &str| ReleaseVersion::parse(tag).unwrap();
    assert_eq!(
        service_version(
            Some("v0.51.0 (abc1234)"),
            Some(v("v0.52.0")),
            Some(v("v0.51.0"))
        ),
        ServiceVersion::Behind(v("v0.52.0"))
    );
    // No scan finished yet: the terminal's own release still shows an older service.
    assert_eq!(
        service_version(Some("v0.50.2 (abc1234)"), None, Some(v("v0.51.0"))),
        ServiceVersion::Behind(v("v0.51.0"))
    );
    assert_eq!(
        service_version(
            Some("v0.52.0 (abc1234)"),
            Some(v("v0.52.0")),
            Some(v("v0.51.0"))
        ),
        ServiceVersion::Current(v("v0.52.0"))
    );
    assert_eq!(
        service_version(Some("v0.53.0 (abc1234)"), Some(v("v0.52.0")), None),
        ServiceVersion::Current(v("v0.53.0"))
    );
    assert_eq!(
        service_version(Some("v0.51.0 (abc1234)"), None, Some(v("v0.51.0"))),
        ServiceVersion::Unknown
    );
    assert_eq!(
        service_version(Some("dev (abc1234)"), Some(v("v0.52.0")), None),
        ServiceVersion::Unknown
    );
    assert_eq!(
        service_version(None, Some(v("v0.52.0")), None),
        ServiceVersion::Unknown
    );
}
