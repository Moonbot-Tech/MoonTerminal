//! Regression coverage for app-local custom colour history and its legacy migration.

use std::path::PathBuf;

use super::*;
use crate::config::badges::BadgesConfig;

/// Build an isolated directory outside the application's real configuration root.
fn fixture_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "moonterminal-custom-colors-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create isolated custom-color fixture directory");
    root
}

/// `custom_colors.rs:CustomColors::load_from` must save its first legacy import; removing
/// `history.save_to(path)?;` leaves no completion record, so later badge edits replace a user's
/// remembered picker palette instead of preserving the app-local history.
#[test]
fn legacy_import_is_durable_and_new_history_stays_authoritative() {
    let root = fixture_root();
    let history_path = root.join("cfg/custom_colors.json");
    let legacy_path = root.join("cfg/badges.json");
    std::fs::create_dir_all(history_path.parent().expect("history has a parent"))
        .expect("create fixture config directory");

    let unique: Vec<[u8; 3]> = (0..22).map(|n| [n, 100 + n, 200 - n]).collect();
    let mut legacy_colors = unique.clone();
    legacy_colors.insert(5, unique[2]);
    let legacy_bytes = serde_json::to_vec_pretty(&BadgesConfig {
        custom_colors: legacy_colors,
        ..BadgesConfig::default()
    })
    .expect("serialize legacy badges fixture");
    std::fs::write(&legacy_path, &legacy_bytes).expect("write legacy badges fixture");
    let expected = unique[..CUSTOM_COLORS_MAX].to_vec();

    let imported = CustomColors::load_from(&history_path, &legacy_path)
        .expect("import legacy custom colours on first load");
    assert_eq!(
        imported.custom_colors, expected,
        "legacy import must preserve first-occurrence order and the twenty-swatch cap"
    );
    assert!(
        history_path.is_file(),
        "first legacy import must persist custom_colors.json before returning"
    );
    assert_eq!(
        std::fs::read(&legacy_path).expect("read unchanged legacy badges fixture"),
        legacy_bytes,
        "legacy badges.json must remain byte-for-byte unchanged for downgrade"
    );

    std::fs::write(
        &legacy_path,
        serde_json::to_vec_pretty(&BadgesConfig {
            custom_colors: vec![[250, 1, 2]],
            ..BadgesConfig::default()
        })
        .expect("serialize later legacy edit"),
    )
    .expect("write later legacy edit");
    assert_eq!(
        CustomColors::load_from(&history_path, &legacy_path)
            .expect("reload persisted app-local history")
            .custom_colors,
        expected,
        "a later legacy edit must not replace a migrated picker history"
    );

    CustomColors::default()
        .save_to(&history_path)
        .expect("save an intentionally empty new history");
    assert!(
        CustomColors::load_from(&history_path, &legacy_path)
            .expect("reload intentionally empty app-local history")
            .custom_colors
            .is_empty(),
        "an existing empty custom_colors.json must win over legacy badges"
    );
    std::fs::remove_dir_all(root).expect("remove custom-color fixture directory");
}

/// Regression target: removing the front-entry guard in `CustomColors::remember_custom_color`
/// duplicates the currently selected colour, making the Settings reuse palette grow on a no-op.
#[test]
fn remember_custom_color_prepends_new_values_and_leaves_the_current_front_unchanged() {
    let initial = vec![[12, 34, 56], [78, 90, 12]];
    let new_color = [210, 45, 67];
    let mut config = CustomColors {
        custom_colors: initial.clone(),
    };

    assert!(config.remember_custom_color(new_color));
    assert_eq!(
        config.custom_colors,
        vec![new_color, initial[0], initial[1]],
        "a newly typed colour must be the first reusable swatch"
    );

    let unchanged = config.custom_colors.clone();
    assert!(!config.remember_custom_color(unchanged[0]));
    assert_eq!(
        config.custom_colors, unchanged,
        "reselecting the current swatch must not duplicate or reorder the palette"
    );
}

/// Regression target: removing the existing-colour removal in `CustomColors::remember_custom_color`
/// leaves duplicate swatches after a previously used colour is selected again.
#[test]
fn remember_custom_color_moves_an_older_entry_to_the_front_without_a_duplicate() {
    let mut config = CustomColors {
        custom_colors: vec![[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]],
    };

    assert!(config.remember_custom_color([7, 8, 9]));
    assert_eq!(
        config.custom_colors,
        vec![[7, 8, 9], [1, 2, 3], [4, 5, 6], [10, 11, 12]],
        "selecting an older swatch must promote that one value instead of cloning it"
    );
}

/// Regression target: removing `truncate(CUSTOM_COLORS_MAX)` in
/// `CustomColors::remember_custom_color` lets the persisted reuse palette exceed the picker cap.
#[test]
fn remember_custom_color_evicts_only_the_oldest_values_at_the_picker_limit() {
    let colors: Vec<[u8; 3]> = (0..=CUSTOM_COLORS_MAX as u8)
        .map(|n| [n, 255 - n, n.wrapping_mul(7)])
        .collect();
    let mut config = CustomColors::default();

    for color in colors.iter().copied() {
        assert!(config.remember_custom_color(color));
    }

    let expected: Vec<[u8; 3]> = colors[1..].iter().rev().copied().collect();
    assert_eq!(
        config.custom_colors, expected,
        "the newest twenty swatches must remain in recency order after the oldest is evicted"
    );
    assert_eq!(config.custom_colors.len(), CUSTOM_COLORS_MAX);
    assert!(config.custom_colors.contains(&colors[10]));
    assert!(!config.custom_colors.contains(&colors[0]));
}
