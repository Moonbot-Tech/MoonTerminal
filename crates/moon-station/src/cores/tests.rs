use super::*;

/// A dropped stored override would make the feed and listing disagree with the terminal after reload.
#[test]
fn stored_core_override_reaches_the_feed_and_listing_without_dns() {
    let key = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA=="; // gitleaks:allow
    let dir = creds("override", &[(3, key)]);
    for (text, expected) in [
        ("Core.Example.Invalid:5020", "core.example.invalid:5020"),
        ("203.0.113.8:5020", "203.0.113.8:5020"),
        ("", "198.51.100.42:4321"),
    ] {
        let config = format!("[[core]]\nuid = 3\nname = 'Fixture'\nendpoint_override = '{text}'\n");
        let station = from_station_file(&config, Some(&dir)).unwrap();
        assert_eq!(station.listed[0].address.as_deref(), Some(expected));
        assert_eq!(station.listed[0].endpoint_override.as_deref(), Some(text));
        let server = &station.config.servers[0];
        let parsed = moon_core::config::parse_endpoint_override(&server.endpoint_override).unwrap();
        assert_eq!(
            moon_core::config::target_from_key(server.key.expose(), parsed.as_ref())
                .unwrap()
                .text(),
            expected
        );
    }
    let invalid = from_station_file(
        "[[core]]\nuid = 3\nname = 'Fixture'\nendpoint_override = 'host:0'\n",
        Some(&dir),
    )
    .unwrap();
    assert_eq!(invalid.listed[0].address, None);
    assert_eq!(invalid.config.servers[0].endpoint_override, "host:0");
}

/// Ignoring the report maximum on upgrade could allocate uid 10 into uid 12's retired history.
#[test]
fn upgraded_uid_floor_includes_retired_report_rows_and_persisted_counter() {
    let config = "[[core]]\nuid = 9\nname = 'Survivor'\nactive = false\n";
    let station = from_station_file(config, None).unwrap();
    assert_eq!(
        high_water_from_reports(station.core_uid_high_water, Ok(Some(12))),
        12
    );
    let station = from_station_file(&format!("core_uid_high_water = 17\n{config}"), None).unwrap();
    assert_eq!(
        high_water_from_reports(station.core_uid_high_water, Ok(Some(12))),
        17
    );
    assert_eq!(high_water_from_reports(9, Ok(None)), 9);
    assert_eq!(
        high_water_from_reports(9, Err(moon_core::db::ReadFail::NotReady)),
        9
    );
    assert_eq!(
        high_water_from_reports(9, Err(moon_core::db::ReadFail::PeriodOutOfRange)),
        u64::MAX
    );
}

/// C1: legacy station files and credentials retain uids and addresses, including inactive entries.
#[test]
fn listing_includes_available_and_unavailable_credentials_without_keys() {
    // Frozen TESTKEY V1 export with synthetic master/MAC bytes and endpoint 198.51.100.42:4321.
    let key = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA=="; // gitleaks:allow
    let dir = creds("listing", &[(3, key), (9, key)]);
    let station = from_station_file(
        "[[core]]\nuid = 3\nname = 'Core A'\n[[core]]\nuid = 5\nname = 'Missing'\n[[core]]\nuid = 9\nname = 'Inactive'\nactive = false\n",
        Some(&dir),
    )
    .unwrap();
    assert_eq!(
        station
            .listed
            .iter()
            .map(|core| core.uid)
            .collect::<Vec<_>>(),
        [3, 5, 9]
    );
    assert_eq!(station.listed[0].name, "Core A");
    assert_eq!(
        station.listed[0].address.as_deref(),
        Some("198.51.100.42:4321")
    );
    assert!(station.listed[0].key_fp.is_some());
    assert_eq!(station.listed[1].name, "Missing");
    assert_eq!(station.listed[1].address, None);
    assert_eq!(station.listed[1].key_fp, None);
    assert_eq!(station.listed[2].name, "Inactive");
    assert_eq!(
        station.listed[2].address.as_deref(),
        Some("198.51.100.42:4321")
    );
    assert_eq!(
        station
            .config
            .servers
            .iter()
            .map(|server| server.uid)
            .collect::<Vec<_>>(),
        [3, 9]
    );
    assert!(!station.config.servers[1].active);
    assert!(
        !serde_json::to_string(&station.listed)
            .unwrap()
            .contains(key)
    );
}

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
    .expect("parses")
    .config;
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
    let Err(err) = from_station_file(
        "[[core]]\nuid = 3\nname = \"A\"\nkey = \"plain\"\n",
        Some(&dir),
    ) else {
        panic!("a station.toml with a key was accepted");
    };
    assert!(format!("{err:#}").contains("key"), "{err:#}");
}

/// Restoring the fatal `core_key(...)?` makes a missing key crash-loop the whole station.
#[test]
fn an_active_core_without_its_credential_is_skipped() {
    let dir = creds("missing", &[]);
    let text = "[[core]]\nuid = 3\nname = \"A\"\n";
    for directory in [Some(dir.as_path()), None] {
        let station = from_station_file(text, directory).expect("the station stays up");
        assert!(station.config.servers.is_empty());
        assert_eq!(
            station.skipped_cores,
            ["core 3 (\"A\"): credential unavailable, skipped"]
        );
    }
}

/// Returning an error or stopping the loop at a bad credential prevents healthy later cores
/// from loading; the skipped identity must remain available to status without any key material.
#[test]
fn unavailable_credentials_do_not_hide_healthy_or_inactive_cores() {
    let dir = creds("partial", &[(3, "synthetic-healthy-key"), (5, " \n")]);
    std::fs::create_dir(dir.join("core-6")).unwrap();
    let text = r#"
        [[core]]
        uid = 4
        name = "Missing"
        [[core]]
        uid = 5
        name = "Empty"
        [[core]]
        uid = 6
        name = "Unreadable"
        [[core]]
        uid = 3
        name = "Healthy"
        transport = "v1"
        [[core]]
        uid = 9
        name = "Off"
        active = false
        [tape]
        margin_s = 300
    "#;
    let station = from_station_file(text, Some(&dir)).unwrap();
    assert_eq!(station.config.servers.len(), 2);
    assert_eq!(station.config.servers[0].uid, 3);
    assert_eq!(
        station.config.servers[0].key.expose(),
        "synthetic-healthy-key"
    );
    assert_eq!(
        station.config.servers[0].transport,
        Some(TransportVersion::V1)
    );
    assert!(station.config.servers[0].active && station.config.servers[0].feed.reports);
    assert_eq!(station.config.servers[1].uid, 9);
    assert!(!station.config.servers[1].active);
    assert_eq!(station.tape.margin_s, Some(300));
    assert_eq!(
        station.skipped_cores,
        [
            "core 4 (\"Missing\"): credential unavailable, skipped",
            "core 5 (\"Empty\"): credential unavailable, skipped",
            "core 6 (\"Unreadable\"): credential unavailable, skipped",
        ]
    );
}

/// The terminal's window rides in `[tape]`; without the section the station keeps its own.
#[test]
fn the_tape_window_comes_from_the_station_file() {
    let dir = creds("tape", &[(3, "k1")]);
    let core = "[[core]]\nuid = 3\nname = \"A\"\n";
    let with_tape = format!("{core}[tape]\nmargin_s = 180\nlong_position_min = 10\n");
    let station = from_station_file(&with_tape, Some(&dir)).unwrap();
    assert_eq!(station.tape.margin_s, Some(180));
    assert_eq!(station.tape.long_position_min, Some(10));

    let station = from_station_file(core, Some(&dir)).unwrap();
    assert!(station.tape.margin_s.is_none() && station.tape.long_position_min.is_none());
    let unknown = format!("{core}[tape]\nmargin = 180\n");
    assert!(from_station_file(&unknown, Some(&dir)).is_err());
}

/// `[telegram]` with the Mini App switches the station to the account profile and gives every
/// core the account feed; without the section it stays light.
#[test]
fn the_mini_app_turns_the_account_feed_on() {
    let dir = creds("tg", &[(3, "k1")]);
    std::fs::write(dir.join("telegram-token"), "123:abc\n").unwrap();
    let core = "[[core]]\nuid = 3\nname = \"A\"\n";

    let light = from_station_file(core, Some(&dir)).unwrap();
    assert_eq!(light.profile(false), Profile::Reports);
    assert!(!light.config.servers[0].feed.orders);

    let reports_bot = format!("{core}[telegram]\nzone = \"Europe/Moscow\"\nlanguage = \"ru\"\n");
    let bot = from_station_file(&reports_bot, Some(&dir)).unwrap();
    assert_eq!(
        bot.profile(false),
        Profile::Reports,
        "a bot alone reads reports"
    );
    assert_eq!(
        bot.profile(true),
        Profile::Account,
        "the bot's Control section reads the account, as the Mini App does"
    );
    assert_eq!(
        light.profile(true),
        Profile::Reports,
        "no bot, nobody to command from"
    );
    let telegram = bot.telegram.as_ref().unwrap();
    assert_eq!(telegram.token.as_ref().map(Secret::expose), Some("123:abc"));
    assert_eq!(telegram.zone, chrono_tz::Europe::Moscow);
    assert_eq!(telegram.language, Language::Ru);

    let mini =
        from_station_file(&format!("{core}[telegram]\nmini_app = true\n"), Some(&dir)).unwrap();
    assert_eq!(mini.profile(false), Profile::Account);
    let feed = mini.config.servers[0].feed;
    assert!(feed.orders && feed.balance && feed.strategies && feed.reports);
    assert!(
        !feed.log && !feed.detects && !feed.alerts && !feed.arb,
        "the Mini App shows none of these"
    );
    let telegram = mini.telegram.as_ref().unwrap();
    assert_eq!(
        (telegram.zone, telegram.language),
        (chrono_tz::Tz::UTC, Language::En)
    );
}

/// An unknown zone, language or key is refused rather than started half-configured; a missing
/// token only keeps the bot off — the cores still run.
#[test]
fn a_broken_telegram_section_is_refused() {
    let core = "[[core]]\nuid = 3\nname = \"A\"\n";
    let no_token = creds("tg-none", &[(3, "k1")]);
    let station = from_station_file(
        &format!("{core}[telegram]\nmini_app = true\n"),
        Some(&no_token),
    )
    .expect("the cores run without the bot's token");
    assert!(station.telegram.as_ref().unwrap().token.is_none());
    assert_eq!(station.config.servers.len(), 1);
    assert_eq!(
        station.profile(true),
        Profile::Reports,
        "no bot, no Mini App: the account nobody reads is not fetched"
    );
    assert!(!station.config.servers[0].feed.orders);

    let dir = creds("tg-bad", &[(3, "k1")]);
    std::fs::write(dir.join("telegram-token"), "123:abc").unwrap();
    for section in [
        "[telegram]\nzone = \"Mars/Olympus\"\n",
        "[telegram]\nlanguage = \"de\"\n",
        "[telegram]\ntoken = \"123:abc\"\n",
    ] {
        assert!(
            from_station_file(&format!("{core}{section}"), Some(&dir)).is_err(),
            "{section}"
        );
    }
}

/// The switch is on unless the file turns it off: a parse that read a missing `[update]` as off
/// would stop every existing station from updating itself.
#[test]
fn update_auto_is_on_unless_the_file_turns_it_off() {
    let dir = creds("update-auto", &[(3, "k1")]);
    let core = "[[core]]\nuid = 3\nname = \"BinF1\"\n";
    let parse = |extra: &str| {
        from_station_file(&format!("{core}{extra}"), Some(&dir))
            .expect("parses")
            .auto_update
    };
    assert!(parse(""), "no [update] section: on");
    assert!(parse("[update]\n"), "an empty [update]: on");
    assert!(parse("[update]\nauto = true\n"));
    assert!(!parse("[update]\nauto = false\n"));
    assert!(
        from_station_file(&format!("{core}[update]\nauto = 1\n"), Some(&dir)).is_err(),
        "a switch that is not a boolean is refused, not guessed"
    );
}
