//! Regression proof for reading pre-migration chart graphics from the durable backup.

use std::path::PathBuf;

use super::*;

/// A temporary fixture directory removed when its test finishes.
struct TempRoot(PathBuf);

impl TempRoot {
    /// Create a unique directory outside the application's real configuration root.
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "moonterminal-theme-legacy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create isolated legacy-theme fixture directory");
        Self(root)
    }

    /// Return a path inside the isolated fixture directory.
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempRoot {
    /// Remove only this test's unique temporary fixture directory.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `theme_legacy.rs::read_legacy_from` must select its backup when the rewritten live theme no
/// longer carries the six moved keys. Deleting the `if backup.has_any()` branch overwrites a
/// retried migration with defaults after a failed `layout.toml` marker write, losing each tab's
/// previously migrated appearance.
#[test]
fn backup_values_survive_a_retried_migration_after_the_live_theme_is_stripped() {
    let root = TempRoot::new();
    let primary = root.join("theme.toml");
    let backup = root.join("theme.toml.pre-chart-graphics.bak");
    std::fs::write(&primary, "[dark]\nname = \"rewritten\"\n")
        .expect("write a live theme without legacy graphics");
    std::fs::write(
        &backup,
        "[dark]\nmarker_scale = 1.4\ntrade_volume_alpha = 0.42\ncandle_volume_style = 2\ncandle_volume_height = 0.31\ncandle_volume_alpha = 0.63\ncandle_volume_scale = [12, 34, 56]\n",
    )
    .expect("write a pre-migration backup with the user's graphics");

    let expected = LegacyChartGraphics {
        source: LegacySource::Read,
        marker_scale: Some(1.4),
        trade_volume_alpha: Some(0.42),
        candle_volume_style: Some(2),
        candle_volume_height: Some(0.31),
        candle_volume_alpha: Some(0.63),
        candle_volume_scale: Some([12, 34, 56]),
    };
    let first = read_legacy_from(&primary, &backup, UiThemeMode::Dark);
    let retry = read_legacy_from(&primary, &backup, UiThemeMode::Dark);

    assert_eq!(
        first, expected,
        "the backup, not defaults, must supply every moved value after the live file is rewritten"
    );
    assert_eq!(
        retry, expected,
        "every retry over the stripped live file must resolve the same durable backup values"
    );
}
