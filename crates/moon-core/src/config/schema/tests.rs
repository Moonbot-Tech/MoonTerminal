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
