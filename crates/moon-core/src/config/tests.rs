//! Runtime configuration-constructor regressions.

use super::{AppConfig, BadgesConfig, ChartThemeSet, OrdersStyleSet, UiThemeMode};

/// A failed startup read must not let a later save overwrite the document with fallback values.
#[test]
fn failed_config_load_preserves_the_original_on_save() {
    const CHILD: &str = "MOON_CONFIG_PRESERVATION_CASE";
    const ROOT: &str = "MOON_CONFIG_PRESERVATION_ROOT";
    let Ok(case) = std::env::var(CHILD) else {
        let mut failures = Vec::new();
        for kind in [
            "layout",
            "hotkeys",
            "theme-flat",
            "theme-paired",
            "orders-flat",
            "orders-paired",
            "storage",
            "detects",
            "arb",
            "badges",
            "figures",
        ] {
            for mode in ["corrupt", "unreadable"] {
                let case = format!("{kind}-{mode}");
                let root = std::env::temp_dir()
                    .join(format!("moon-preserve-{case}-{}", std::process::id()));
                std::fs::create_dir_all(&root).unwrap();
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "config::tests::failed_config_load_preserves_the_original_on_save",
                        "--nocapture",
                    ])
                    .env(CHILD, &case)
                    .env(ROOT, &root)
                    .output()
                    .unwrap();
                std::fs::remove_dir_all(root).unwrap();
                if !output.status.success()
                    || !String::from_utf8_lossy(&output.stdout).contains("1 passed")
                {
                    failures.push(format!(
                        "{case}: {}",
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os(ROOT).unwrap());
    assert!(super::paths::set_data_dir_override(root));
    let unreadable = case.ends_with("-unreadable");
    let kind = case.rsplit_once('-').unwrap().0;
    let body: &[u8] = if kind.ends_with("-paired") {
        b"[dark\n"
    } else {
        b"[broken\n"
    };
    use super::paths;
    match kind {
        "layout" => preserve_config_fixture(
            paths::layout_path(),
            body,
            unreadable,
            || super::WindowLayout::load(super::ProfileAge::Established),
            |cfg| {
                cfg.save();
            },
        ),
        "hotkeys" => preserve_config_fixture(
            paths::hotkeys_path(),
            body,
            unreadable,
            || super::HotkeysConfig::load().expect("existing file is not a migration source"),
            |cfg| {
                let _ = cfg.save();
            },
        ),
        "theme-flat" | "theme-paired" => preserve_config_fixture(
            paths::theme_path(),
            body,
            unreadable,
            ChartThemeSet::load,
            |cfg| {
                let _ = cfg.save();
            },
        ),
        "orders-flat" | "orders-paired" => preserve_config_fixture(
            paths::orders_path(),
            body,
            unreadable,
            OrdersStyleSet::load,
            |cfg| {
                let _ = cfg.save();
            },
        ),
        "storage" => preserve_config_fixture(
            paths::storage_path(),
            body,
            unreadable,
            super::storage::load,
            |cfg| super::storage::save(&cfg),
        ),
        "detects" => preserve_config_fixture(
            paths::detects_view_path(),
            body,
            unreadable,
            super::detect_view::DetectViewFile::load,
            |cfg| {
                cfg.save();
            },
        ),
        "arb" => preserve_config_fixture(
            paths::arb_view_path(),
            body,
            unreadable,
            super::arb_view::ArbViewCfg::load,
            |cfg| cfg.save(),
        ),
        "badges" => preserve_config_fixture(
            paths::tab_badges_path(),
            body,
            unreadable,
            super::TabBadgeSettings::load,
            |cfg| {
                cfg.save();
            },
        ),
        "figures" => preserve_config_fixture(
            paths::figures_path(),
            body,
            unreadable,
            crate::figures::FigureStore::load,
            |mut cfg| {
                use crate::figures::tools::HLine;
                use crate::figures::{DrawStyle, Figure, FigureKind};
                cfg.add(
                    1,
                    "BTCUSDT",
                    Figure::new(
                        FigureKind::HLine(HLine { price: 42.0 }),
                        DrawStyle::default(),
                        0.0,
                    ),
                );
                cfg.save();
            },
        ),
        _ => panic!("unknown fixture {case}"),
    }
}

/// Simulate either malformed bytes or a read failure that disappears before the next save.
fn preserve_config_fixture<T>(
    path: std::path::PathBuf,
    original: &[u8],
    unreadable: bool,
    load: impl FnOnce() -> T,
    save: impl FnOnce(T),
) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    if unreadable {
        std::fs::create_dir(&path).unwrap();
    } else {
        std::fs::write(&path, original).unwrap();
    }
    let cfg = load();
    if unreadable {
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, original).unwrap();
    }
    save(cfg);
    assert!(
        std::fs::read(&path).unwrap() == original,
        "a fallback must not replace the unread original"
    );
    let unrelated = path.with_extension("unrelated");
    super::write_file_atomic(&unrelated, b"independent saved state", "fixture").unwrap();
    assert_eq!(
        std::fs::read(unrelated).unwrap(),
        b"independent saved state"
    );
}

/// Failed atomic writes must retain figure edits and report badge failure for the next save window.
#[test]
fn figures_and_badges_retry_after_transient_write_failure() {
    const ROOT: &str = "MOON_SAVE_RETRY_TEST_ROOT";
    let Some(root) = std::env::var_os(ROOT) else {
        let root = std::env::temp_dir().join(format!("moon-save-retry-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "config::tests::figures_and_badges_retry_after_transient_write_failure",
                "--nocapture",
            ])
            .env(ROOT, &root)
            .output()
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    assert!(super::paths::set_data_dir_override(root.into()));
    use crate::figures::tools::HLine;
    use crate::figures::{DrawStyle, Figure, FigureKind, FigureStore};
    let mut figures = FigureStore::default();
    figures.add(
        1,
        "BTCUSDT",
        Figure::new(
            FigureKind::HLine(HLine { price: 42.0 }),
            DrawStyle::default(),
            0.0,
        ),
    );
    let path = super::paths::figures_path();
    std::fs::create_dir_all(&path).unwrap();
    figures.save();
    assert!(figures.dirty, "a failed write must leave the edit pending");
    std::fs::remove_dir(path).unwrap();
    figures.save();
    assert!(!figures.dirty);
    assert_eq!(
        FigureStore::load().figures(1, "BTCUSDT")[0].kind,
        FigureKind::HLine(HLine { price: 42.0 })
    );

    let mut badges = super::TabBadgeSettings::default();
    badges.set_counters_visible("Orders", false);
    let path = super::paths::tab_badges_path();
    std::fs::create_dir_all(&path).unwrap();
    assert!(
        !badges.save(),
        "the coordinator must be told to retain dirty"
    );
    std::fs::remove_dir(path).unwrap();
    assert!(badges.save());
    assert!(!super::TabBadgeSettings::load().counters_visible("Orders"));
}

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
    let settings = super::schema::SettingsFile {
        charts_split_by_core: false,
        chart_stack_height: 321,
        ..super::schema::SettingsFile::default()
    };

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
    // …except the one thing a bench must never inherit: a Main chart that closes itself after a
    // quiet minute takes the screenshot with it.
    assert_eq!(config.main_idle_close_secs, 0);
}

/// Catches `config/mod.rs:AppConfig::chart_theme` treating Graphite as light.
/// If it selects the light chart set, Graphite's dark interface paints charts with the wrong palette.
#[test]
fn graphite_chart_theme_uses_the_existing_dark_chart_set() {
    let mut config = AppConfig::blank(None);
    config.ui_theme_mode = UiThemeMode::Graphite;
    assert_eq!(config.chart_theme(), &config.theme.dark);
}
