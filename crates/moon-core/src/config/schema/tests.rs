//! Compatibility tests for persisted history sizing, theme defaults and retired keys.

use moonproto::state::MarketHistorySizing;

use super::*;

/// Catches `SettingsFile` growing `deny_unknown_fields`, or a retired key regaining a field:
/// every profile written while `ui_density` and `ui_font_delta` existed must still load, and
/// the values it carries for them must not reach the interface.
#[test]
fn retired_density_and_font_keys_are_ignored_on_load() {
    for legacy in [
        "ui_font_delta = 6.0",
        "ui_density = \"compact\"",
        "ui_density = \"large\"\nui_font_delta = nan",
    ] {
        let parsed: SettingsFile = toml::from_str(legacy)
            .unwrap_or_else(|e| panic!("a file carrying `{legacy}` must still load: {e}"));
        assert_eq!(
            parsed.ui_scale,
            default_ui_scale(),
            "`{legacy}` must not touch the scale"
        );
    }
}

/// Catches serializing either retired key again: a file that carried them loses them on its
/// next save, so the interface never reads a density it no longer has.
#[test]
fn retired_keys_are_dropped_on_save() {
    let settings: SettingsFile =
        toml::from_str("ui_font_delta = 6.0\nui_density = \"large\"\nui_scale = 1.25").unwrap();
    let saved = toml::to_string(&settings).unwrap();
    assert!(!saved.contains("ui_font_delta"));
    assert!(!saved.contains("ui_density"));
    let reloaded: SettingsFile = toml::from_str(&saved).unwrap();
    assert_eq!(reloaded.ui_scale, 1.25);
}

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

/// Catches dropping `serde(other)` from `config/schema.rs:UiThemeMode::Dark`. A theme value only
/// a newer build knows must load as Dark with every other setting intact, not fail the whole
/// file — the loader quarantines a failed file to `.bak` and the downgrade loses all settings.
#[test]
fn unknown_theme_mode_loads_as_dark_without_failing_the_settings_file() {
    let reread: SettingsFile = toml::from_str("ui_theme_mode = \"neon-future\"\nui_scale = 1.25\n")
        .expect("an unknown theme value must not fail the settings file");
    assert_eq!(reread.ui_theme_mode, UiThemeMode::Dark);
    assert_eq!(reread.ui_scale, 1.25);
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

/// Catches changing `config/schema.rs:UiThemeMode::is_light` to include Graphite, or leaving the
/// light experimental mode out. Either would route a mode through the other side's color branches
/// in badges, orders, lines, and charts.
#[test]
fn graphite_is_dark_while_light_remains_the_only_light_mode() {
    assert!(UiThemeMode::Light.is_light());
    assert!(UiThemeMode::LightExperimental.is_light());
    assert!(!UiThemeMode::Graphite.is_light());
    assert!(!UiThemeMode::Dark.is_light());
    assert!(!UiThemeMode::DarkExperimental.is_light());
}

/// Catches renaming the experimental `UiThemeMode` variants without their explicit serde names.
/// Under `rename_all = "lowercase"` they would persist as `"darkexperimental"`, and a settings file
/// written by this build would then stop loading the moment the spelling changed.
#[test]
fn experimental_theme_modes_round_trip_under_their_hyphenated_settings_values() {
    for (mode, value) in [
        (UiThemeMode::DarkExperimental, "dark-experimental"),
        (UiThemeMode::LightExperimental, "light-experimental"),
    ] {
        let stored = SettingsFile {
            ui_theme_mode: mode,
            ..SettingsFile::default()
        };
        let persisted = toml::to_string(&stored).expect("settings file must serialize");
        let line = format!("ui_theme_mode = \"{value}\"");
        assert!(
            persisted.lines().any(|l| l == line),
            "settings.toml must persist {mode:?} as {value}"
        );
        let reread: SettingsFile = toml::from_str(&line).expect("stored mode must load");
        assert_eq!(reread.ui_theme_mode, mode);
    }
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
