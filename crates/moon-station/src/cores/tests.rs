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
    assert_eq!(light.profile(), Profile::Reports);
    assert!(!light.config.servers[0].feed.orders);

    let reports_bot = format!("{core}[telegram]\nzone = \"Europe/Moscow\"\nlanguage = \"ru\"\n");
    let bot = from_station_file(&reports_bot, Some(&dir)).unwrap();
    assert_eq!(bot.profile(), Profile::Reports, "a bot alone reads reports");
    let telegram = bot.telegram.as_ref().unwrap();
    assert_eq!(telegram.token.as_ref().map(Secret::expose), Some("123:abc"));
    assert_eq!(telegram.zone, chrono_tz::Europe::Moscow);
    assert_eq!(telegram.language, Language::Ru);

    let mini =
        from_station_file(&format!("{core}[telegram]\nmini_app = true\n"), Some(&dir)).unwrap();
    assert_eq!(mini.profile(), Profile::Account);
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
        station.profile(),
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
