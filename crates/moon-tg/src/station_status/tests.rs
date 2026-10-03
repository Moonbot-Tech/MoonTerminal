use moon_core::station_api::{CpuWindow, DataFile, Disk, Memory};

use super::*;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Removing a section or flattening its rows loses structured readings in Settings.
#[test]
fn typed_facts_keep_sections_values_and_missing_reading_notes() {
    let _locale = crate::test_locale::force("en");
    let facts = StatusFacts::of(&status(Some(host(vec![
        file("small.db", MIB),
        file("large.db", 2 * GIB),
    ]))));
    assert_eq!(facts.title, "Station 0.1.0 — up 3 h 12 min");
    assert_eq!(
        facts
            .sections
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        ["Service", "Tape", "Server", "Largest files"]
    );
    assert_eq!(
        facts.sections[0].rows,
        [("Cores".into(), "26 of 27 ready".into())]
    );
    assert_eq!(
        facts.sections[1].rows,
        [
            ("Around a trade".into(), "3 min".into()),
            ("Long trade from".into(), "10 min".into()),
        ]
    );
    assert_eq!(facts.sections[2].rows.len(), 7);
    assert_eq!(
        facts.sections[3].rows,
        [
            ("large.db".into(), "2.00 GB".into()),
            ("small.db".into(), "1.0 MB".into()),
        ]
    );
    assert!(facts.notes.is_empty());
    let missing = StatusFacts::of(&status(None));
    assert_eq!(missing.sections.len(), 2);
    assert_eq!(missing.notes.len(), 1);
    assert!(missing.notes[0].ends_with("update it."));
}

fn status(host: Option<Host>) -> Status {
    Status {
        station_version: "0.1.0".into(),
        cores_ready: 26,
        cores_total: 27,
        bot: None,
        tape: Some(TapeWindow {
            margin_s: 180,
            long_position_min: 10,
        }),
        host: host.map(Box::new),
        last_update: None,
        auto_update: None,
    }
}

fn host(files: Vec<DataFile>) -> Host {
    Host {
        uptime_s: 3 * 3_600 + 12 * 60 + 5,
        cpu: vec![
            CpuWindow {
                minutes: 60,
                station_avg_permille: 123,
                station_peak_permille: 870,
                machine_avg_permille: 150,
                machine_peak_permille: 1000,
            },
            CpuWindow {
                minutes: 192,
                station_avg_permille: 5,
                station_peak_permille: 870,
                machine_avg_permille: 90,
                machine_peak_permille: 1000,
            },
        ],
        memory: Some(Memory {
            rss_bytes: 420 * MIB,
            rss_peak_bytes: 520 * MIB,
            available_bytes: 300 * MIB,
            total_bytes: 955 * MIB,
        }),
        disk: Some(Disk {
            free_bytes: 15 * GIB + GIB / 2,
            total_bytes: 23 * GIB,
        }),
        files,
    }
}

fn file(name: &str, bytes: u64) -> DataFile {
    DataFile {
        name: name.into(),
        bytes,
    }
}

/// The terminal's lines are grouped under headings, one short `label: value` each, sizes in
/// their own unit (`size_text`), files largest first — and never any markup.
#[test]
fn the_status_reads_in_short_grouped_lines() {
    let _locale = crate::test_locale::force("en");
    let host = host(vec![
        file("telegram.json", 10),
        file("reports.sqlite", 609 * MIB),
    ]);
    let text = station_status_text(&status(Some(host)));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "Station 0.1.0 — up 3 h 12 min",
            "Service",
            "  Cores: 26 of 27 ready",
            "Tape",
            "  Around a trade: 3 min",
            "  Long trade from: 10 min",
            "Server",
            "  Station CPU, hour: 12.3 % (peak 87.0 %)",
            "  Server CPU, hour: 15.0 % (peak 100.0 %)",
            "  Station CPU, 192 min: 0.5 % (peak 87.0 %)",
            "  Server CPU, 192 min: 9.0 % (peak 100.0 %)",
            "  Station memory: 420.0 MB (peak 520.0 MB)",
            "  Server memory: 300.0 MB free of 955.0 MB",
            "  Disk: 15.50 GB free of 23.00 GB",
            "Largest files",
            "  reports.sqlite: 609.0 MB",
            "  telegram.json: 0 KB",
        ]
    );
}

/// The terminal shows the plain lines verbatim: no tag, even around a name that looks like one.
#[test]
fn the_terminal_text_carries_no_html() {
    for locale in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let mut status = status(Some(host(vec![file("<b>x</b>.db", 1)])));
        status.last_update = Some("2026-09-30T14:02Z future=<i>".into());
        let text = station_status_text(&status);
        for tag in ["<p>", "<table", "<tr", "<td", "&lt;", "&amp;"] {
            assert!(!text.contains(tag), "{tag} in {text}");
        }
        assert!(
            text.contains("<b>x</b>.db"),
            "names are shown as they are: {text}"
        );
    }
}

/// A station older than the figures says so instead of showing nothing.
#[test]
fn an_older_station_is_told_to_update() {
    let _locale = crate::test_locale::force("en");
    let text = station_status_text(&status(None));
    assert!(text.starts_with("Station 0.1.0\n"));
    assert!(text.ends_with("update it."));
}

/// The chat gets a rich message: every dynamic value is escaped, and a huge data root is cut to
/// the few largest files so the message stays inside Telegram's limits.
#[test]
fn the_chat_status_is_escaped_and_bounded() {
    let _locale = crate::test_locale::force("en");
    let mut files: Vec<_> = (0..5_000u64)
        .map(|n| file(&format!("<script>&{}{}", "x".repeat(300), n), n))
        .collect();
    files.push(file("big & <b>bold</b>.sqlite", 9 * GIB));
    let mut status = status(Some(host(files)));
    status.station_version = "0.1.0 <dev>".into();
    status.last_update = Some("2026-09-30T14:02Z update=failed: <boom> & more".into());
    let Response::Rich { html, keyboard, .. } = station_status_reply(
        &status,
        &ReleaseCheck::Current,
        crate::station_owner_navigation(&moon_core::config::TelegramConfig::default()),
    ) else {
        panic!("the status is a rich message");
    };
    assert!(crate::report::rich_message_fits(&html));
    assert!(html.chars().count() < 4_096, "{}", html.chars().count());
    assert!(html.contains("big &amp; &lt;b&gt;bold&lt;/b&gt;.sqlite"));
    assert!(html.contains("0.1.0 &lt;dev&gt;"));
    assert!(html.contains("&lt;boom&gt; &amp; more"));
    assert!(!html.contains("<script>") && !html.contains("<boom>") && !html.contains("<dev>"));
    assert!(html.contains("and 4996 more"), "{html}");
    assert_eq!(html.matches("&lt;script&gt;").count(), 4);
    let ReplyMarkup::Inline(markup) = keyboard else {
        panic!("rich messages carry an inline keyboard");
    };
    assert_eq!(
        markup.inline_keyboard.len(),
        1,
        "only the way back to Settings"
    );
    assert_eq!(
        markup.inline_keyboard[0][0].callback_data.as_deref(),
        Some("m:s")
    );
}

/// The chat's "Update" button comes only with a newer release, carrying the callback the bot
/// parses; the station's navigation always comes with the answer, and a failed look says why.
#[test]
fn the_update_button_comes_only_with_a_newer_release() {
    let _locale = crate::test_locale::force("en");
    let Response::Rich {
        html,
        keyboard,
        navigation,
    } = station_status_reply(
        &status(None),
        &ReleaseCheck::Newer("v0.52.0".into()),
        crate::station_owner_navigation(&moon_core::config::TelegramConfig::default()),
    )
    else {
        panic!("the status is a rich message");
    };
    assert!(html.ends_with("<p>A new version of the station is out: v0.52.0.</p>"));
    assert_eq!(
        navigation.1,
        crate::station_owner_navigation(&moon_core::config::TelegramConfig::default())
    );
    let mut updated = status(None);
    updated.last_update = Some("2026-09-30T14:02Z health=ok".into());
    assert!(
        station_status_text(&updated)
            .contains("\n  Last update: 2026-09-30T14:02Z Updated; the service is healthy.\n")
    );
    let ReplyMarkup::Inline(markup) = keyboard else {
        panic!("a newer release brings the inline Update button");
    };
    assert_eq!(markup.inline_keyboard[0][0].text, "Update to v0.52.0");
    assert_eq!(
        markup.inline_keyboard[0][0].callback_data.as_deref(),
        Some(STATION_UPDATE_CALLBACK)
    );

    for (check, tail) in [
        (ReleaseCheck::Current, "update it.</p>"),
        (
            ReleaseCheck::Failed(ReleaseFailure::Unavailable(
                "GitHub releases returned HTTP 403".into(),
            )),
            "Details: GitHub releases returned HTTP 403</p>",
        ),
        (
            ReleaseCheck::Unversioned,
            "updated from the terminal only.</p>",
        ),
    ] {
        let Response::Rich { html, keyboard, .. } = station_status_reply(
            &status(None),
            &check,
            crate::station_owner_navigation(&moon_core::config::TelegramConfig::default()),
        ) else {
            panic!("the status is a rich message");
        };
        assert!(html.ends_with(tail), "{html}");
        assert!(
            matches!(keyboard, ReplyMarkup::Inline(ref markup)
                if markup.inline_keyboard.len() == 1
                    && markup.inline_keyboard[0][0].callback_data.as_deref() == Some("m:s")),
            "{check:?} brings no Update button, only the way back to Settings"
        );
    }
}

#[test]
fn uptime_takes_the_largest_two_units() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(duration(59), "0 min");
    assert_eq!(duration(3_600), "1 h 0 min");
    assert_eq!(duration(2 * 86_400 + 5 * 3_600 + 59 * 60), "2 d 5 h");
}

/// Restoring raw helper verdicts would put health=ok in Russian and Spanish bot headlines.
#[test]
fn old_helper_verdicts_and_refusals_are_localized() {
    for (locale, health, tab) in [
        ("ru", "служба работает", "вкладке «Станция»"),
        ("en", "service is healthy", "Station tab"),
        ("es", "servicio funciona", "pestaña Estación"),
    ] {
        let _locale = crate::test_locale::force(locale);
        let verdict = update_verdict("2026-09-30T14:02Z update: health=ok");
        assert!(verdict.contains(health), "{verdict}");
        assert!(verdict.starts_with("2026-09-30T14:02Z "));
        assert!(!verdict.contains("health=ok"));
        assert_eq!(
            update_verdict("2026-09-30T14:02Z update from release: health=ok"),
            verdict
        );
        assert!(UpdateRefusal::UpdaterMissing.text().contains(tab));
        let failure = ReleaseFailure::Unavailable("HTTP 403".into()).text();
        assert!(!failure.lines().next().unwrap().contains("HTTP 403"));
        assert!(failure.lines().nth(1).unwrap().contains("HTTP 403"));
        let unknown = update_verdict("future=unrecognized");
        assert!(!unknown.lines().next().unwrap().contains("future="));
        assert!(
            unknown
                .lines()
                .nth(1)
                .unwrap()
                .contains("future=unrecognized")
        );
    }
}

/// The helper publishes its final error line after successful automatic rollback, not health=ok.
#[test]
fn automatic_rollback_verdict_describes_the_restored_service() {
    for (locale, expected) in [
        ("ru", "прежняя версия восстановлена"),
        ("en", "previous version restored"),
        ("es", "versión anterior restaurada"),
    ] {
        let _locale = crate::test_locale::force(locale);
        let shown = update_verdict(
            "2026-09-30T14:02Z update from release: error: the new binary did not stay up; the previous one is back",
        );
        assert!(shown.contains(expected), "{shown}");
        assert!(!shown.contains("error:"));
        assert!(shown.starts_with("2026-09-30T14:02Z "));
    }
}

/// The Service section says whether the station updates itself; a station older than the switch
/// shows no row rather than a guess.
#[test]
fn the_service_section_shows_the_auto_update_switch() {
    let _locale = crate::test_locale::force("en");
    let row = |on: Option<bool>| {
        let mut s = status(None);
        s.auto_update = on;
        StatusFacts::of(&s).sections[0]
            .rows
            .iter()
            .find(|(label, _)| label == "Auto-update")
            .map(|(_, value)| value.clone())
    };
    assert_eq!(row(Some(true)).as_deref(), Some("on"));
    assert_eq!(row(Some(false)).as_deref(), Some("off"));
    assert_eq!(row(None), None);
}
