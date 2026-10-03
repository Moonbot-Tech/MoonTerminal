//! Settings defaults, validation, pruning, and the notifications file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

static TEST_SEQ: AtomicU32 = AtomicU32::new(0);

/// Isolated temp root. Removed when the test drops it, including on panic.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "moonterminal-notify-{}-{tag}-{}",
            std::process::id(),
            TEST_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_all_off(settings: &NotifySettings) {
    assert!(!settings.trades.on);
    assert_eq!(settings.trades.cores, CoreScope::All);
    assert_eq!(settings.trades.min_volume_usd, None);
    assert_eq!(settings.trades.profit_at_least_usd, None);
    assert_eq!(settings.trades.loss_at_least_usd, None);
    assert!(!settings.down.on);
    assert_eq!(settings.down.after_minutes, 5);
    assert!(!settings.daily.on);
    assert_eq!((settings.daily.hour, settings.daily.minute), (21, 0));
}

/// Switching the struct defaults on would announce into every paired chat before anyone opts in.
#[test]
fn default_settings_leave_every_notification_off() {
    let settings = NotifySettings::default();
    assert_all_off(&settings);
    assert!(settings.validate().is_ok());
    let file = NotifyFile::default();
    assert!(file.chats.is_empty());
    assert!(file.outbox.is_empty());
    assert_eq!(file.next_id, 0);
    let ledger_json = serde_json::to_string(&NotifyLedger::default()).unwrap();
    let ledger: NotifyLedger = serde_json::from_str(&ledger_json).unwrap();
    assert_eq!(ledger.daily_last, None);
    assert!(ledger.seen.is_empty());
}

/// A hand-written `{}` must deserialize as those same off defaults, including a present empty chat.
#[test]
fn empty_json_object_loads_all_off() {
    let settings: NotifySettings = serde_json::from_str("{}").expect("empty settings");
    assert_eq!(settings, NotifySettings::default());
    assert_all_off(&settings);

    let root = TempRoot::new("empty");
    let path = root.path("notifications.json");
    std::fs::write(&path, "{}").unwrap();
    let loaded = NotifyFile::load(&path).expect("empty file");
    assert_eq!(loaded, NotifyFile::default());

    let with_chat: NotifyFile = serde_json::from_str(r#"{"chats":{"1":{}}}"#).expect("empty chat");
    let chat = with_chat.chats.get(&1).expect("chat 1");
    assert_eq!(chat.settings, NotifySettings::default());
    assert_eq!(chat.ledger, NotifyLedger::default());
    assert_eq!(chat.revision, 0);
    assert!(chat.settings.validate().is_ok());
}

/// An object that sets only `on` must keep the 5 minute and 21:00 defaults, not zero.
#[test]
fn partial_rules_keep_clock_defaults() {
    let settings: NotifySettings =
        serde_json::from_str(r#"{"trades":{"on":true},"down":{"on":true},"daily":{"on":true}}"#)
            .expect("partial settings");
    assert!(settings.trades.on);
    assert_eq!(settings.trades.cores, CoreScope::All);
    assert!(settings.down.on);
    assert_eq!(settings.down.after_minutes, 5);
    assert!(settings.daily.on);
    assert_eq!((settings.daily.hour, settings.daily.minute), (21, 0));
    assert!(settings.validate().is_ok());
}

/// `{"all":true}` is the rejected shape. The file uses a `kind` tag.
#[test]
fn core_scope_uses_a_kind_tag() {
    let all = serde_json::to_value(CoreScope::All).unwrap();
    assert_eq!(all, serde_json::json!({"kind": "all"}));
    let only = serde_json::to_value(CoreScope::Only(vec![7, 8])).unwrap();
    assert_eq!(only, serde_json::json!({"kind": "only", "ids": [7, 8]}));
    assert!(serde_json::from_str::<CoreScope>(r#"{"all":true}"#).is_err());
    let empty: CoreScope = serde_json::from_str(r#"{"kind":"only","ids":[]}"#).unwrap();
    assert_eq!(empty, CoreScope::Only(vec![]));
}

/// First launch has no file. Load must not create one, or a crash would look like an opt-out write.
#[test]
fn missing_file_loads_the_default_without_creating_it() {
    let root = TempRoot::new("missing");
    let path = root.path("notifications.json");
    let loaded = NotifyFile::load(&path).expect("missing file");
    assert_eq!(loaded, NotifyFile::default());
    assert!(!path.exists());
}

/// A truncated file is an error and stays on disk. Replacing it with defaults would wipe chats.
#[test]
fn corrupt_file_is_an_error_and_is_left_in_place() {
    let root = TempRoot::new("corrupt");
    let path = root.path("notifications.json");
    std::fs::write(&path, "{").unwrap();
    assert!(NotifyFile::load(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{");

    let directory = root.path("not-a-file");
    std::fs::create_dir(&directory).unwrap();
    assert!(NotifyFile::load(&directory).is_err());
}

/// A bad calendar date is corrupt. Skipping it would move the daily summary to "never sent".
#[test]
fn invalid_daily_date_is_corrupt() {
    let root = TempRoot::new("date");
    let path = root.path("notifications.json");
    std::fs::write(
        &path,
        r#"{"chats":{"1":{"ledger":{"daily_last":"2026-13-40"}}}}"#,
    )
    .unwrap();
    assert!(NotifyFile::load(&path).is_err());
}

fn sample_file() -> NotifyFile {
    let mut seen = BTreeMap::new();
    seen.insert(
        7,
        BTreeMap::from([(99, 1_700_000_100), (-5, 1_700_000_050)]),
    );
    let mut down_announced = BTreeSet::new();
    down_announced.insert(7);
    let chat = ChatNotify {
        settings: NotifySettings {
            trades: TradeRule {
                on: true,
                cores: CoreScope::Only(vec![7]),
                min_volume_usd: Some(100.0),
                profit_at_least_usd: Some(1.5),
                loss_at_least_usd: None,
                usd_followup: true,
            },
            down: DownRule {
                on: true,
                after_minutes: 12,
            },
            daily: DailyRule {
                on: false,
                hour: 9,
                minute: 30,
            },
            reports: AutoReports {
                hourly: true,
                today: false,
                month: true,
            },
        },
        ledger: NotifyLedger {
            trades_enabled_utc: Some(1_700_000_000),
            seen,
            down_announced,
            daily_last: NaiveDate::from_ymd_opt(2026, 3, 1),
            reports: AutoLedger {
                hourly: AutoSlot {
                    slot_utc: Some(1_700_003_600),
                    message: Some(42),
                },
                ..AutoLedger::default()
            },
            ..NotifyLedger::default()
        },
        revision: 3,
    };
    let mut chats = BTreeMap::new();
    chats.insert(-10042, chat);
    NotifyFile {
        chats,
        outbox: vec![Pending {
            id: 1,
            chat: -10042,
            html: "<b>closed</b>".to_string(),
            created_utc: 1_700_000_200,
            cores: None,
            auto: None,
            ..Pending::default()
        }],
        next_id: 2,
    }
}

/// Integer map keys must come back as decimal strings, or a restart drops the announce-once set.
#[test]
fn save_and_load_round_trip_keeps_ledger_outbox_and_string_map_keys() {
    let root = TempRoot::new("round");
    let path = root.path("nested").join("notifications.json");
    let original = sample_file();
    original.save(&path).expect("save");

    let raw = std::fs::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let chats = value["chats"].as_object().expect("chats object");
    assert!(chats.contains_key("-10042"));
    let seen = value["chats"]["-10042"]["ledger"]["seen"]
        .as_object()
        .expect("seen object");
    assert!(seen.contains_key("7"));
    let rows = seen["7"].as_object().expect("record object");
    assert!(rows.contains_key("99"));
    assert!(rows.contains_key("-5"));
    assert_eq!(rows["99"], serde_json::json!(1_700_000_100));
    assert_eq!(
        value["chats"]["-10042"]["ledger"]["daily_last"],
        serde_json::json!("2026-03-01")
    );
    assert_eq!(
        value["outbox"][0]["html"],
        serde_json::json!("<b>closed</b>")
    );
    assert_eq!(value["next_id"], serde_json::json!(2));

    let loaded = NotifyFile::load(&path).expect("load");
    assert_eq!(loaded, original);

    let hand = r#"{
        "chats": {
            "-10042": {
                "settings": {
                    "trades": {
                        "on": true,
                        "cores": {"kind": "only", "ids": [7]},
                        "min_volume_usd": 100.0,
                        "profit_at_least_usd": 1.5,
                        "usd_followup": true
                    },
                    "down": {"on": true, "after_minutes": 12},
                    "daily": {"on": false, "hour": 9, "minute": 30},
                    "reports": {"hourly": true, "month": true}
                },
                "ledger": {
                    "trades_enabled_utc": 1700000000,
                    "seen": {"7": {"-5": 1700000050, "99": 1700000100}},
                    "down_announced": [7],
                    "daily_last": "2026-03-01",
                    "reports": {"hourly": {"slot_utc": 1700003600, "message": 42}}
                },
                "revision": 3
            }
        },
        "outbox": [{"id": 1, "chat": -10042, "html": "<b>closed</b>", "created_utc": 1700000200}],
        "next_id": 2
    }"#;
    let from_disk: NotifyFile = serde_json::from_str(hand).expect("hand-written file");
    assert_eq!(from_disk, original);
    assert_eq!(from_disk.outbox[0].cores, None);
}

/// A missing `cores` field is unknown. A JSON array is an explicit disclosure, including `[]`.
#[test]
fn missing_cores_field_is_none_and_an_array_is_some() {
    let missing: Pending = serde_json::from_str(r#"{"id":1,"chat":2,"html":"h","created_utc":3}"#)
        .expect("legacy row");
    assert_eq!(missing.cores, None);

    let empty: Pending =
        serde_json::from_str(r#"{"id":1,"chat":2,"html":"h","created_utc":3,"cores":[]}"#)
            .expect("empty disclosure");
    assert_eq!(empty.cores, Some(Vec::new()));

    let listed: Pending =
        serde_json::from_str(r#"{"id":1,"chat":2,"html":"h","created_utc":3,"cores":[7]}"#)
            .expect("listed cores");
    assert_eq!(listed.cores, Some(vec![7]));

    let root = TempRoot::new("cores-opt");
    let path = root.path("notifications.json");
    let mut file = NotifyFile::default();
    file.outbox.push(Pending {
        id: 1,
        chat: 2,
        html: "quiet".into(),
        created_utc: 3,
        cores: Some(Vec::new()),
        auto: None,
        ..Pending::default()
    });
    file.next_id = 2;
    file.save(&path).expect("save empty disclosure");
    let raw = std::fs::read_to_string(&path).expect("read empty disclosure");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(value["outbox"][0]["cores"], serde_json::json!([]));
    let loaded = NotifyFile::load(&path).expect("load empty disclosure");
    assert_eq!(loaded.outbox[0].cores, Some(Vec::new()));

    file.outbox[0].cores = None;
    file.save(&path).expect("save unknown row");
    let raw = std::fs::read_to_string(&path).expect("read unknown row");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert!(value["outbox"][0].get("cores").is_none());
    let loaded = NotifyFile::load(&path).expect("load unknown row");
    assert_eq!(loaded.outbox[0].cores, None);
}

fn threshold(field: &str, value: f64) -> NotifyError {
    let mut settings = NotifySettings::default();
    match field {
        "min_volume_usd" => settings.trades.min_volume_usd = Some(value),
        "profit_at_least_usd" => settings.trades.profit_at_least_usd = Some(value),
        "loss_at_least_usd" => settings.trades.loss_at_least_usd = Some(value),
        other => panic!("unknown field {other}"),
    }
    settings.validate().expect_err(field)
}

/// Dropping `is_finite` would persist +inf, and a negative floor would invert the filter.
#[test]
fn validate_rejects_non_finite_and_negative_thresholds() {
    for field in ["min_volume_usd", "profit_at_least_usd", "loss_at_least_usd"] {
        match threshold(field, f64::NAN) {
            NotifyError::Threshold { field: got, value } => {
                assert_eq!(got, field);
                assert!(value.is_nan());
            }
            other => panic!("expected NaN threshold, got {other:?}"),
        }
        match threshold(field, f64::INFINITY) {
            NotifyError::Threshold { field: got, value } => {
                assert_eq!(got, field);
                assert!(value.is_infinite() && value.is_sign_positive());
            }
            other => panic!("expected infinite threshold, got {other:?}"),
        }
        match threshold(field, -1.0) {
            NotifyError::Threshold { field: got, value } => {
                assert_eq!(got, field);
                assert_eq!(value, -1.0);
            }
            other => panic!("expected negative threshold, got {other:?}"),
        }
        let mut ok = NotifySettings::default();
        match field {
            "min_volume_usd" => ok.trades.min_volume_usd = Some(0.0),
            "profit_at_least_usd" => ok.trades.profit_at_least_usd = Some(0.0),
            "loss_at_least_usd" => ok.trades.loss_at_least_usd = Some(0.0),
            _ => unreachable!(),
        }
        assert!(ok.validate().is_ok(), "{field} zero is a real floor");
    }
}

/// `0` would notify on the first missed tick, and `1441` is longer than a day.
#[test]
fn validate_rejects_after_minutes_outside_one_day() {
    for (minutes, ok) in [(0, false), (1, true), (1440, true), (1441, false)] {
        let settings = NotifySettings {
            down: DownRule {
                on: true,
                after_minutes: minutes,
            },
            ..NotifySettings::default()
        };
        assert_eq!(settings.validate().is_ok(), ok, "after_minutes {minutes}");
        if !ok {
            assert_eq!(
                settings.validate(),
                Err(NotifyError::AfterMinutes { value: minutes })
            );
        }
    }
}

/// Hour 24 and minute 60 are not a civil time. The adjacent in-range values stay valid.
#[test]
fn validate_rejects_hour_24_minute_60_and_an_empty_core_list() {
    let hour = NotifySettings {
        daily: DailyRule {
            on: true,
            hour: 24,
            minute: 0,
        },
        ..NotifySettings::default()
    };
    assert_eq!(hour.validate(), Err(NotifyError::Hour { value: 24 }));
    let minute = NotifySettings {
        daily: DailyRule {
            on: true,
            hour: 23,
            minute: 60,
        },
        ..NotifySettings::default()
    };
    assert_eq!(minute.validate(), Err(NotifyError::Minute { value: 60 }));
    let edges = NotifySettings {
        daily: DailyRule {
            on: true,
            hour: 23,
            minute: 59,
        },
        ..NotifySettings::default()
    };
    assert!(edges.validate().is_ok());
    let midnight = NotifySettings {
        daily: DailyRule {
            on: true,
            hour: 0,
            minute: 0,
        },
        ..NotifySettings::default()
    };
    assert!(midnight.validate().is_ok());

    let empty = NotifySettings {
        trades: TradeRule {
            on: true,
            cores: CoreScope::Only(vec![]),
            ..TradeRule::default()
        },
        ..NotifySettings::default()
    };
    assert_eq!(empty.validate(), Err(NotifyError::EmptyCores));
    let listed = NotifySettings {
        trades: TradeRule {
            cores: CoreScope::Only(vec![4]),
            ..TradeRule::default()
        },
        ..NotifySettings::default()
    };
    assert!(listed.validate().is_ok());
}

/// Keeping a close older than the window would grow the file forever and re-announce nothing new.
/// Dropping the equal bound would announce that trade again on the next tick.
#[test]
fn prune_seen_keeps_newer_and_drops_older_and_empty_cores() {
    let mut ledger = NotifyLedger::default();
    ledger
        .seen
        .insert(1, BTreeMap::from([(10, 100), (11, 200), (14, 150)]));
    ledger.seen.insert(2, BTreeMap::from([(12, 50)]));
    ledger.seen.insert(3, BTreeMap::new());
    ledger.down_announced.insert(1);
    ledger.prune_seen(150);

    let mut expected = BTreeMap::new();
    expected.insert(1, BTreeMap::from([(11, 200), (14, 150)]));
    assert_eq!(ledger.seen, expected);
    assert!(ledger.down_announced.contains(&1));
}

/// A file from before automatic reports loads with them off and no slot recorded.
#[test]
fn a_file_without_auto_reports_loads_them_off() {
    let json = r#"{
        "chats": {"7": {"settings": {"daily": {"on": true}}, "ledger": {}, "revision": 2}},
        "outbox": [{"id": 1, "chat": 7, "html": "x", "created_utc": 3}],
        "next_id": 2
    }"#;
    let file: NotifyFile = serde_json::from_str(json).expect("old file");
    let chat = &file.chats[&7];
    assert!(chat.settings.daily.on);
    assert!(!chat.settings.reports.any());
    assert_eq!(chat.ledger.reports, AutoLedger::default());
    assert_eq!(file.outbox[0].auto, None);
}

/// A queued automatic report of a kind this build does not know keeps the file readable.
#[test]
fn an_unknown_auto_report_kind_does_not_fail_the_file() {
    let json = r#"{
        "outbox": [{"id": 1, "chat": 7, "html": "x", "created_utc": 3,
                    "auto": {"kind": "weekly", "keyboard": {"inline_keyboard": []}}},
                   {"id": 2, "chat": 7, "html": "y", "created_utc": 4,
                    "auto": {"kind": "hourly", "keyboard": {"inline_keyboard": []}}}],
        "next_id": 3
    }"#;
    let file: NotifyFile = serde_json::from_str(json).expect("newer file");
    assert_eq!(file.outbox[0].auto, None);
    assert_eq!(
        file.outbox[1].auto.as_ref().map(|auto| auto.kind),
        Some(AutoReport::Hourly)
    );
}
