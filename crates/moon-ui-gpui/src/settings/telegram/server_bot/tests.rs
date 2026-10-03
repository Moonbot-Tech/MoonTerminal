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

/// Enabling station details in Telegram leaks status/journal; removing shared feedback hides
/// job results. This binary crate's tab ownership contract is checked against render wiring.
#[test]
fn station_and_telegram_tabs_both_show_the_shared_job_result() {
    let telegram = include_str!("../../telegram.rs");
    let bot_sections = telegram
        .find(".children(self.server_bot_sections(cx))")
        .unwrap();
    assert!(telegram[bot_sections..].contains(".child(self.server_bot_progress(false, cx))"));
    let station = include_str!("../server_bot.rs");
    let section = station
        .split("fn server_bot_section(")
        .nth(1)
        .unwrap()
        .split("fn server_bot_field(")
        .next()
        .unwrap();
    assert!(section.contains("s.child(self.server_bot_progress(true, cx))"));
    let progress = station.split("fn server_bot_progress(").nth(1).unwrap();
    let (feedback, details) = progress.split_once(".when(station_details, |s|").unwrap();
    assert!(feedback.contains(".when(st.busy(),"));
    assert!(feedback.contains(".when_some(outcome,"));
    assert!(!feedback.contains("StationProgress"));
    assert!(!feedback.contains("st.status"));
    assert!(!feedback.contains("st.lines"));
    assert!(details.contains("s.child(progress::StationProgress"));
    assert!(details.contains("status: st.status.clone()"));
    assert!(details.contains("lines: st.lines.clone()"));
    assert!(details.contains("scroll: self.telegram.server.lines_scroll.clone()"));
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

/// The station's draft asks only for what the station knows, never for its zone, and reads as
/// unedited when it holds what was read — whatever zone the station reports.
#[test]
fn the_station_draft_asks_for_what_the_station_knows() {
    use moon_core::config::telegram_menu::{MenuItem, MenuLevel};
    use moon_core::station_api::Access;
    let read = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        bot: Some(Default::default()),
        zone: Some("Europe/Moscow".into()),
        ..Access::default()
    };
    let mut draft = super::draft_of(&read);
    assert!(!super::access_edited(Some(&draft), Some(&read)));
    let asked = super::draft_access(&draft, &read);
    assert_eq!(asked.zone, None, "an edit never carries the zone back");
    assert!(
        draft
            .bot
            .menu
            .set_shown(MenuLevel::Keyboard, MenuItem::Report, true)
    );
    assert!(super::access_edited(Some(&draft), Some(&read)));
    // A station that predates the bot's settings: they are neither asked nor an edit.
    let old = Access {
        bot: None,
        zone: None,
        ..read.clone()
    };
    let draft = super::draft_of(&old);
    assert!(!super::access_edited(Some(&draft), Some(&old)));
    assert_eq!(super::draft_access(&draft, &old).bot, None);
}
