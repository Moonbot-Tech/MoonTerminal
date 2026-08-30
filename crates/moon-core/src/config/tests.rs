//! Runtime configuration-constructor regressions.

use super::{AppConfig, BadgesConfig, ChartThemeSet, OrdersStyleSet};

/// Regression target: removing `ensure_server_group_configs` from
/// `AppConfig::build_plaintext_config` leaves the environment-backed core without durable
/// group-local Size, TP, or SL state.
#[test]
fn plaintext_config_materializes_its_server_group() {
    let config = AppConfig::build_plaintext_config(
        None,
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
        BadgesConfig::default(),
        vec![(1, "synthetic".to_string())],
        "desk".to_string(),
        "BTCUSDT".to_string(),
        "synthetic".to_string(),
        true,
        super::schema::SettingsFile::default(),
        false,
    );

    assert_eq!(
        config.groups,
        vec![super::GroupConfig::new("desk")],
        "the plaintext runtime must own concrete local controls for its server group"
    );
}

/// The bench's cores must BE the cores its data belongs to, so the list arrives as uid/name pairs
/// read out of the reports database rather than as one fixed core.
#[test]
fn a_plaintext_config_carries_every_core_it_was_given() {
    let config = AppConfig::build_plaintext_config(
        None,
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
        BadgesConfig::default(),
        vec![(1, "BB1".to_string()), (3, "BinF1".to_string())],
        "desk".to_string(),
        "BTCUSDT".to_string(),
        "synthetic".to_string(),
        true,
        super::schema::SettingsFile::default(),
        false,
    );

    let cores: Vec<(u64, &str)> = config
        .servers
        .iter()
        .map(|s| (s.uid, s.name.as_str()))
        .collect();
    assert_eq!(cores, vec![(1, "BB1"), (3, "BinF1")]);
    // Breakage: since schema v11 the runtime CoreId EQUALS the uid, and everything binds to it.
    // A positional id brought the second core up as core 2 while its trades said core 3.
    for server in &config.servers {
        assert_eq!(server.id, server.uid, "CoreId must equal the uid");
    }
    // Breakage: a counter fixed at 2 hands the next core a uid that is already in use here.
    let mut counter = config.next_uid.clone();
    assert_eq!(
        counter.issue(),
        4,
        "the counter must start above the highest uid given"
    );
}

/// Breakage: a malformed list silently configuring fewer cores than asked for shows up as empty
/// panels, which reads as a data problem rather than as a typo in the environment.
#[test]
fn a_malformed_core_list_is_refused() {
    assert!(AppConfig::parse_plaintext_cores("1:BB1,3:BinF1").is_ok());
    assert!(AppConfig::parse_plaintext_cores("  1 : BB1 , 3 : BinF1 ").is_ok());
    for bad in ["", "BB1", "1:", "0:BB1", "x:BB1", "1:BB1,1:Other"] {
        assert!(
            AppConfig::parse_plaintext_cores(bad).is_err(),
            "{bad:?} must be refused"
        );
    }
}

/// Breakage: the test config used to hardcode its own defaults, so nothing arranged in Settings
/// survived a bench launch - two cores opened six chart tabs however the developer had set it, and
/// a pinned layout was impossible.
#[test]
fn a_plaintext_config_keeps_the_settings_it_was_given() {
    let mut settings = super::schema::SettingsFile::default();
    settings.charts_split_by_core = false;
    settings.chart_stack_height = 321;
    settings.ui_font_delta = 2.5;

    let config = AppConfig::build_plaintext_config(
        None,
        ChartThemeSet::default(),
        OrdersStyleSet::default(),
        BadgesConfig::default(),
        vec![(1, "BB1".to_string())],
        "desk".to_string(),
        "BTCUSDT".to_string(),
        "synthetic".to_string(),
        true,
        settings,
        false,
    );

    assert!(!config.charts_split_by_core);
    assert_eq!(config.chart_stack_height, 321);
    assert_eq!(config.ui_font_delta, 2.5);
    // …except the one thing a bench must never inherit: a Main chart that closes itself after a
    // quiet minute takes the screenshot with it.
    assert_eq!(config.main_idle_close_secs, 0);
}
