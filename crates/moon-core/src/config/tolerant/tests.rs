//! A value a newer build wrote must cost only its own field, never the whole settings file.

use super::super::groups::{GroupExitSettings, TakeProfitMode};
use super::super::schema::{SettingsFile, UiThemeMode};
use super::super::servers::TransportVersion;
use super::super::toml_io::{ConfigLoad, load_or_default_status};
use std::path::PathBuf;

/// Write `text` to a fresh temp file and load it the way `store::read_settings` does, recording
/// whether the quarantine hook fired.
fn load(tag: &str, text: &str) -> (SettingsFile, ConfigLoad, bool) {
    let dir = std::env::temp_dir().join(format!(
        "moon-tolerant-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path: PathBuf = dir.join("settings.toml");
    std::fs::write(&path, text).expect("write fixture");
    let mut quarantined = false;
    let (sf, status) = load_or_default_status(&path, "settings.toml", |_| {
        quarantined = true;
        true
    });
    let _ = std::fs::remove_dir_all(&dir);
    (sf, status, quarantined)
}

/// Synthetic file shared by the cases: every non-enum value differs from its default, so a
/// reset of the whole file is visible on each of them.
const BODY: &str = r#"
version = 18
chart_stack_height = 777
log_retention_days = 3
ui_scale = 1.25
next_uid = 42

[[servers]]
uid = 7
name = "core-a"
group = "grp-x"
market = "futures"
"#;

/// Assert that every non-enum value in [`BODY`] survived the load.
fn assert_body_kept(sf: &SettingsFile) {
    assert_eq!(sf.version, 18);
    assert_eq!(sf.chart_stack_height, 777);
    assert_eq!(sf.log_retention_days, 3);
    assert_eq!(sf.ui_scale, 1.25);
    assert_eq!(sf.next_uid, 42);
    assert_eq!(sf.servers.len(), 1);
    assert_eq!(sf.servers[0].name, "core-a");
    assert_eq!(sf.servers[0].group, "grp-x");
}

#[test]
fn an_unknown_theme_resets_only_the_theme() {
    let text = format!("ui_theme_mode = \"from-a-newer-build\"\n{BODY}");
    let (sf, status, quarantined) = load("theme", &text);
    assert_eq!(status, ConfigLoad::Present);
    assert!(
        !quarantined,
        "an unknown theme must not move the file to .bak"
    );
    assert_eq!(sf.ui_theme_mode, UiThemeMode::default());
    assert_body_kept(&sf);
}

#[test]
fn an_unknown_per_core_transport_resets_only_that_field() {
    let text = BODY.replace(
        "market = \"futures\"",
        "market = \"futures\"\ntransport = \"v9\"",
    );
    let (sf, status, quarantined) = load("transport", &text);
    assert_eq!(status, ConfigLoad::Present);
    assert!(
        !quarantined,
        "an unknown transport must not move the file to .bak"
    );
    assert_eq!(sf.servers[0].transport, None::<TransportVersion>);
    assert_body_kept(&sf);
}

#[test]
fn an_unknown_take_profit_mode_resets_only_that_field() {
    let text = format!(
        "{BODY}\n[[groups]]\nname = \"grp-x\"\nicon = 5\n[groups.trade.exit]\n\
         take_profit_mode = \"hyper\"\n"
    );
    let (sf, status, quarantined) = load("tp", &text);
    assert_eq!(status, ConfigLoad::Present);
    assert!(
        !quarantined,
        "an unknown TP mode must not move the file to .bak"
    );
    assert_eq!(sf.groups.len(), 1);
    assert_eq!(sf.groups[0].icon, 5);
    assert_eq!(
        sf.groups[0].trade.exit.take_profit_mode,
        TakeProfitMode::Scalp,
        "no stored TP: the fallback is the exit block's own default mode"
    );
    assert_body_kept(&sf);
}

/// Load one group whose exit block holds `take_profit_pct` under a mode this build cannot name.
fn exit_with_unknown_mode(tag: &str, take_profit_pct: &str) -> GroupExitSettings {
    let text = format!(
        "{BODY}
[[groups]]
name = \"grp-x\"
[groups.trade.exit]
         take_profit_mode = \"hyper\"
take_profit_pct = {take_profit_pct}
         fixed_sell_pcts = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0]
fixed_sell_slot = 2
         stop_loss_pct = -3.5
stop_loss_enabled = true
"
    );
    let (sf, status, quarantined) = load(tag, &text);
    assert_eq!(status, ConfigLoad::Present);
    assert!(!quarantined);
    sf.groups[0].trade.exit
}

/// Regression target: falling back to a fixed mode (e.g. `TakeProfitMode::default()`, Normal)
/// makes a Scalp-range TP fail `canonicalize`, which resets TP, fixed sells and stop loss.
#[test]
fn an_unknown_take_profit_mode_keeps_the_rest_of_the_exit_block() {
    for (tag, pct, mode) in [
        ("tp-scalp", "0.5", TakeProfitMode::Scalp),
        ("tp-normal", "25.0", TakeProfitMode::Normal),
        ("tp-extended", "300.0", TakeProfitMode::Extended),
    ] {
        let mut exit = exit_with_unknown_mode(tag, pct);
        assert_eq!(exit.take_profit_mode, mode, "pct {pct}");
        assert!(
            exit.canonicalize(),
            "pct {pct}: the fallback mode must accept the stored TP, or the exit block resets"
        );
        assert_eq!(exit.take_profit_pct, pct.parse::<f64>().unwrap());
        // Extended quantizes fixed sells through f32 by design, so compare within that precision.
        assert!((exit.fixed_sell_pcts[4] - 3.0).abs() < 1e-6);
        assert_eq!(exit.fixed_sell_slot, Some(2));
        assert_eq!(exit.stop_loss_pct, -3.5);
        assert!(exit.stop_loss_enabled);
    }
}

#[test]
fn an_unknown_action_click_drops_only_that_entry() {
    use super::super::hotkeys::{HotkeysConfig, MouseGestureBinding};
    let text = "[action_clicks]
cancel_buy = \"middle\"
sell_all = \"from-a-newer-build\"
";
    let cfg: HotkeysConfig = toml::from_str(text).expect("an unknown value must not fail the file");
    assert_eq!(
        cfg.action_clicks.get("cancel_buy"),
        Some(&MouseGestureBinding::Middle),
        "a readable entry must survive its unreadable neighbour"
    );
    assert!(!cfg.action_clicks.contains_key("sell_all"));
}

#[test]
fn a_known_value_still_loads_as_written() {
    let text = format!("ui_theme_mode = \"graphite\"\n{BODY}");
    let (sf, _, _) = load("known", &text);
    assert_eq!(sf.ui_theme_mode, UiThemeMode::Graphite);
}

#[test]
fn malformed_toml_is_still_quarantined() {
    let (_, status, quarantined) = load("malformed", "ui_theme_mode = \"dark\n[[servers");
    assert!(quarantined, "malformed TOML must keep the .bak quarantine");
    assert_eq!(status, ConfigLoad::Corrupt);
}

#[test]
fn an_unknown_hotkey_gesture_falls_back_to_that_fields_own_default() {
    use super::super::hotkeys::{HotkeysConfig, MouseGestureBinding};
    let text = "cancel_buy = \"ctrl-k\"\nfig_delete_click = \"from-a-newer-build\"\n\
                buy_move_kind = \"from-a-newer-build\"\n";
    let cfg: HotkeysConfig = toml::from_str(text).expect("unknown gestures must not fail the file");
    assert_eq!(cfg.cancel_buy, "ctrl-k", "a readable binding must survive");
    assert_eq!(
        cfg.fig_delete_click,
        MouseGestureBinding::Middle,
        "the fallback must be the field's serde default, not the type's Default"
    );
    assert_eq!(cfg.buy_move_kind, Default::default());
}
