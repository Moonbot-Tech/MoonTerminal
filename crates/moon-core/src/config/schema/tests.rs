//! Compatibility tests for persisted MoonProto retained-history sizing and theme defaults.

use moonproto::state::MarketHistorySizing;

use super::*;

/// The plausible production mutation is `config/schema.rs:clamp_chart_memory_percent`: restoring
/// `value.clamp(100, 800)` rejects MoonProto's 75% depth setting, so a saved 75 reloads as 100.
#[test]
fn chart_history_percentage_tracks_moonproto_contract() {
    let min = MarketHistorySizing::MIN_BUDGET_PERCENT;
    let max = MarketHistorySizing::MAX_BUDGET_PERCENT;

    assert_eq!(
        default_chart_memory_percent(),
        MarketHistorySizing::DEFAULT_BUDGET_PERCENT
    );
    assert_eq!(clamp_chart_memory_percent(min), min);
    assert_eq!(clamp_chart_memory_percent(min.saturating_sub(1)), min);
    assert_eq!(clamp_chart_memory_percent(max), max);
    assert_eq!(clamp_chart_memory_percent(max.saturating_add(1)), max);
}

/// `config/schema.rs:resolve_ui_theme_mode` must keep its sole Light branch at
/// `(FirstRun, Absent)`; moving it to an unreadable or corrupt read re-themes an established
/// user during a transient settings-file failure.
#[test]
fn first_run_theme_is_light_only_for_an_absent_settings_file() {
    let stored = UiThemeMode::Dark;
    let cases = [
        (ProfileAge::FirstRun, ConfigLoad::Absent, UiThemeMode::Light),
        (ProfileAge::FirstRun, ConfigLoad::Present, stored),
        (ProfileAge::FirstRun, ConfigLoad::Corrupt, stored),
        (ProfileAge::FirstRun, ConfigLoad::Unreadable, stored),
        (ProfileAge::Established, ConfigLoad::Absent, stored),
        (ProfileAge::Established, ConfigLoad::Present, stored),
        (ProfileAge::Established, ConfigLoad::Corrupt, stored),
        (ProfileAge::Established, ConfigLoad::Unreadable, stored),
    ];

    for (age, load, expected) in cases {
        assert_eq!(
            resolve_ui_theme_mode(stored, load, age),
            expected,
            "{age:?} with {load:?} must preserve the first-run theme boundary"
        );
    }
}

/// A core-issued strategy id uses the whole `u64` range while a TOML integer is an `i64`, so one
/// pinned strategy above that ceiling used to abort `toml::to_string` for the WHOLE file: every
/// setting in the application stopped saving, and the only visible sign was
/// "out-of-range value for u64 type" beside the Save button. Both id fields go through
/// `config::wire_id`; this asserts the file they live in survives the value.
#[test]
fn a_core_issued_strategy_id_cannot_break_the_whole_settings_file() {
    let id = u64::MAX;
    let stored = format!(
        "[[servers]]\n\
         uid = 1\n\
         name = \"Core\"\n\
         default_alert_strategy = \"{id}\"\n\
         [servers.manual_strategy]\n\
         strategy = \"Manual1\"\n\
         id = \"{id}\"\n"
    );

    let file: SettingsFile = toml::from_str(&stored).expect("string form must load");
    let text =
        toml::to_string_pretty(&file).expect("an id above i64::MAX must not abort the write");
    let reread: SettingsFile = toml::from_str(&text).expect("our own output must load");

    for parsed in [&file, &reread] {
        assert_eq!(parsed.servers[0].default_alert_strategy, id);
        assert_eq!(
            parsed.servers[0]
                .manual_strategy
                .as_ref()
                .expect("manual strategy section")
                .id,
            id
        );
    }
}

/// Catches dropping `config/schema.rs:UiThemeMode`'s `serde(rename_all = "lowercase")`.
/// Without it, Graphite persists as `"Graphite"`, which the same build cannot read from settings.
#[test]
fn graphite_theme_mode_round_trips_as_the_lowercase_settings_value() {
    let stored = SettingsFile {
        ui_theme_mode: UiThemeMode::Graphite,
        ..SettingsFile::default()
    };
    let persisted = toml::to_string(&stored).expect("settings file must serialize");
    assert!(
        persisted
            .lines()
            .any(|line| line == "ui_theme_mode = \"graphite\""),
        "settings.toml must persist the lowercase Graphite spelling"
    );
    let reread: SettingsFile =
        toml::from_str("ui_theme_mode = \"graphite\"").expect("stored graphite must load");
    assert_eq!(reread.ui_theme_mode, UiThemeMode::Graphite);
}

/// Catches changing `config/schema.rs:UiThemeMode::is_light` to include Graphite.
/// That would route Graphite through every light color branch in badges, orders, lines, and charts.
#[test]
fn graphite_is_dark_while_light_remains_the_only_light_mode() {
    assert!(UiThemeMode::Light.is_light());
    assert!(!UiThemeMode::Graphite.is_light());
    assert!(!UiThemeMode::Dark.is_light());
}

/// `config::store::read_servers` must continue accepting a pre-cut Telegram table whose removed
/// `default_report_days` and `pushes` keys are unknown to `TelegramConfig`; otherwise saving the
/// decoded configuration would strand the user's encrypted core credentials behind a parse error.
#[test]
fn legacy_telegram_keys_decode_and_are_dropped_without_losing_surviving_values() {
    let legacy = r#"
        [[servers]]
        uid = 41
        name = "alpha"
        key = "core-secret"

        [telegram]
        token = "bot-secret"
        authorized_chat_ids = [1001, -1002]
        mini_app_enabled = true
        default_report_days = 7

        [telegram.pushes]
        reports = true
    "#;

    let decoded: ServersFile = toml::from_str(legacy)
        .expect("a servers.enc payload from the previous Telegram build must decode");

    assert_eq!(decoded.servers.len(), 1);
    assert_eq!(decoded.servers[0].uid, 41);
    assert_eq!(decoded.servers[0].name, "alpha");
    assert_eq!(decoded.servers[0].key.expose(), "core-secret");
    assert_eq!(decoded.telegram.token.expose(), "bot-secret");
    assert_eq!(decoded.telegram.authorized_chat_ids, vec![1001, -1002]);
    assert!(decoded.telegram.mini_app_enabled);

    let rewritten = toml::to_string(&decoded)
        .expect("the accepted legacy payload must remain serializable after the migration");
    assert!(
        !rewritten.contains("default_report_days") && !rewritten.contains("[telegram.pushes]"),
        "the next save intentionally removes only the retired Telegram keys: {rewritten}"
    );
    assert!(rewritten.contains("uid = 41"));
    assert!(rewritten.contains("name = \"alpha\""));
    assert!(rewritten.contains("key = \"core-secret\""));
    assert!(rewritten.contains("token = \"bot-secret\""));
    assert!(rewritten.contains("authorized_chat_ids = [1001, -1002]"));
    assert!(rewritten.contains("mini_app_enabled = true"));
}
