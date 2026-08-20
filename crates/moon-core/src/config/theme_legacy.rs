//! One-shot reader for the trade-mark and bottom-volume values that used to live in `theme.toml`.
//!
//! Those six values moved onto [`super::ChartGraphicsCfg`], where they belong to a chart TAB rather
//! than to a colour scheme. Carrying a user's existing numbers across is the whole point of this
//! module, and it has to read the file as TEXT rather than through [`super::theme::ChartThemeSet`]:
//! once the fields left `ChartTheme`, `#[serde(default)]` drops the keys on deserialize, and
//! `ChartThemeSet::load`'s legacy-flat branch RE-SAVES the file immediately. By the time a
//! `ChartThemeSet` exists in memory the values are already gone.
//!
//! Nothing here writes to `theme.toml`. It only takes a `.bak` copy, because the leftover keys are
//! NOT a durable recovery copy: `AppConfig::save_impl` calls `ChartThemeSet::save` on every settings
//! write, so the user's first trip through Settings erases them.

use std::path::PathBuf;

use super::paths;
use super::schema::UiThemeMode;

/// Suffix of the durable pre-migration copy of `theme.toml`.
const BACKUP_SUFFIX: &str = "pre-chart-graphics.bak";

/// Why a read produced the values it did.
///
/// The migration has to tell "there was nothing to migrate" from "I could not look", because only
/// the first is a legitimate reason to mark the one-shot pass complete. A locked or half-written
/// `theme.toml` that reported "absent" would commit stock values forever.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LegacySource {
    /// No `theme.toml` and no backup: a fresh install with nothing to carry over.
    #[default]
    Absent,
    /// A file was read and parsed. It may still have carried none of the six keys.
    Read,
    /// A file EXISTS but could not be read or parsed. Nothing may be concluded from it.
    Unreadable,
}

/// The six legacy values, each present only when the old file actually carried it.
///
/// `None` means "the file never said", which is what lets the migration leave a field at its new
/// default instead of stamping a fabricated number over it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LegacyChartGraphics {
    /// Whether the values below are trustworthy, and why.
    pub source: LegacySource,
    /// Former `marker_scale` key in the chart-theme TOML.
    pub marker_scale: Option<f32>,
    /// Former `trade_volume_alpha` key in the chart-theme TOML.
    pub trade_volume_alpha: Option<f32>,
    /// Former `candle_volume_style` key in the chart-theme TOML.
    pub candle_volume_style: Option<u8>,
    /// Former `candle_volume_height` key in the chart-theme TOML.
    pub candle_volume_height: Option<f32>,
    /// Former `candle_volume_alpha` key in the chart-theme TOML.
    pub candle_volume_alpha: Option<f32>,
    /// Former `candle_volume_scale` key in the chart-theme TOML.
    pub candle_volume_scale: Option<[u8; 3]>,
}

impl LegacyChartGraphics {
    /// Whether the file carried at least one of the six values.
    ///
    /// A file that parsed but named none of them is indistinguishable from one written after the
    /// move, which is exactly when the backup becomes the better source.
    pub fn has_any(&self) -> bool {
        self.marker_scale.is_some()
            || self.trade_volume_alpha.is_some()
            || self.candle_volume_style.is_some()
            || self.candle_volume_height.is_some()
            || self.candle_volume_alpha.is_some()
            || self.candle_volume_scale.is_some()
    }
}

/// Path of the durable pre-migration copy beside `theme.toml`.
pub fn backup_path() -> PathBuf {
    let path = paths::theme_path();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "theme.toml".to_owned());
    path.with_file_name(format!("{name}.{BACKUP_SUFFIX}"))
}

/// Copy `theme.toml` aside once, before anything can rewrite it.
///
/// Idempotent by existence check rather than by a marker: a second run must never overwrite the
/// FIRST copy, which is the only one taken while the legacy keys were still present. A missing
/// source file is a fresh install and needs no copy.
///
/// Returns:
///     True when a copy now exists on disk, whether this call made it or an earlier one did.
pub fn backup_legacy_theme_file() -> bool {
    let src = paths::theme_path();
    let dst = backup_path();
    if dst.exists() {
        return true;
    }
    if !src.exists() {
        return false;
    }
    match std::fs::copy(&src, &dst) {
        Ok(_) => true,
        Err(e) => {
            log::warn!(
                "не удалось сохранить копию {} перед переносом настроек графики: {e}",
                src.display()
            );
            false
        }
    }
}

/// Read one number that TOML may have stored as either a float or an integer.
///
/// Load-bearing: a hand-typed `marker_scale = 1` is an INTEGER to the TOML parser, and reading only
/// `as_float` would silently report "the file never said" for a value the user did set.
fn as_f32(v: &toml::Value) -> Option<f32> {
    v.as_float()
        .map(|f| f as f32)
        .or_else(|| v.as_integer().map(|i| i as f32))
}

/// Read one style id, saturating a value too large for a `u8` rather than discarding it.
fn as_u8(v: &toml::Value) -> Option<u8> {
    v.as_integer().map(|i| i.clamp(0, i64::from(u8::MAX)) as u8)
}

/// Read one sRGB triple.
fn as_rgb(v: &toml::Value) -> Option<[u8; 3]> {
    let arr = v.as_array()?;
    if arr.len() != 3 {
        return None;
    }
    let mut out = [0u8; 3];
    for (slot, item) in out.iter_mut().zip(arr) {
        *slot = as_u8(item)?;
    }
    Some(out)
}

/// The UI theme mode recorded in `settings.toml`, read as TEXT.
///
/// The migration runs BEFORE `AppConfig::load`, so the parsed config does not exist yet and this is
/// the only way to learn which of the two theme tables the user is actually looking at. Anything
/// unreadable answers Dark, which is both the enum's own default and the table a legacy flat file
/// becomes.
fn active_theme_mode() -> UiThemeMode {
    let Ok(text) = std::fs::read_to_string(paths::settings_path()) else {
        return UiThemeMode::Dark;
    };
    let Ok(doc) = text.parse::<toml::Value>() else {
        return UiThemeMode::Dark;
    };
    match doc.get("ui_theme_mode").and_then(|v| v.as_str()) {
        Some("light") => UiThemeMode::Light,
        _ => UiThemeMode::Dark,
    }
}

/// Read the six legacy values out of `theme.toml`.
///
/// Which of the two theme tables wins is decided by the user's ACTIVE mode, not by a fixed choice
/// of dark. That matters because Settings bound every one of these fields through
/// `theme.get_mut(is_light)`: a user who tuned them while in light mode wrote them into `[light]`
/// ONLY, and `[dark]` still holds stock values. Migrating from dark unconditionally would carry
/// stock numbers forward and throw the user's real edits away.
///
/// Per field, the order is: the active mode's table, then the other table, then absent. The
/// cross-table fallback mirrors what serde did here anyway — a key missing from `[light]` was filled
/// from `ChartTheme::default()`, which IS the dark set.
///
/// A file with neither table is the legacy FLAT shape, which `ChartThemeSet::load` also treats as
/// dark; there the document root is the only table.
///
/// The BACKUP is consulted when the live file no longer carries any of the six keys. That is not a
/// belt-and-braces flourish — it is what makes a retry correct. The migration's marker lives in
/// `layout.toml`, so a run whose `charts.json` write lands and whose `layout.toml` write fails is
/// retried on the next launch; by then `AppConfig::save` may well have rewritten `theme.toml`
/// without the now-unknown keys, and a reader that looked only there would resolve DEFAULTS and
/// stamp them over values it had already migrated correctly. Reading the backup makes the whole
/// pass idempotent no matter how many times it runs.
///
/// Returns:
///     Every value the file carried; `None` for each one it did not, plus how the read went.
pub fn read_legacy_chart_graphics() -> LegacyChartGraphics {
    read_legacy_from(&paths::theme_path(), &backup_path(), active_theme_mode())
}

/// The whole decision, over two NAMED files.
///
/// Split out from [`read_legacy_chart_graphics`] purely so it can be exercised: that wrapper reads
/// process-global config paths, and a test that had to point those at a fixture would be reaching
/// into the user's real `cfg/` directory. Everything worth asserting lives here.
///
/// The MODE is passed in rather than read here for the same reason the paths are: it comes from
/// `settings.toml`, and a test that depended on the developer's own file would pass or fail by
/// accident.
///
/// Args:
///     primary_path: The live theme file.
///     backup_path: The pre-migration copy taken beside it.
///     mode: The UI theme mode whose table wins, per field, before the other table is tried.
///
/// Returns:
///     The values to migrate, and how confidently they were obtained.
pub fn read_legacy_from(
    primary_path: &std::path::Path,
    backup_path: &std::path::Path,
    mode: UiThemeMode,
) -> LegacyChartGraphics {
    let primary = read_one(primary_path, mode);
    // Keys found, or a file that genuinely holds none of them and parsed fine: nothing to add.
    if primary.source == LegacySource::Read && primary.has_any() {
        return primary;
    }
    let backup = read_one(backup_path, mode);
    if backup.has_any() {
        return backup;
    }
    // Neither carried a value. An UNREADABLE file on either side still outranks "absent": it means
    // something is there and we failed to look, which must not be committed as a finished migration.
    if primary.source == LegacySource::Unreadable || backup.source == LegacySource::Unreadable {
        return LegacyChartGraphics {
            source: LegacySource::Unreadable,
            ..LegacyChartGraphics::default()
        };
    }
    primary
}

/// Read the six values out of ONE theme file.
///
/// Args:
///     path: The file to read; either the live `theme.toml` or its pre-migration backup.
///     mode: The UI theme mode whose table is preferred.
///
/// Returns:
///     Whatever that file carried, and whether it could be read at all.
fn read_one(path: &std::path::Path, mode: UiThemeMode) -> LegacyChartGraphics {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return LegacyChartGraphics::default();
        }
        Err(e) => {
            // Present but unreadable — locked by a backup tool, a permission problem, a half-written
            // file. Saying "absent" here would freeze the user on stock values forever.
            log::warn!("{} не прочитан: {e}", path.display());
            return LegacyChartGraphics {
                source: LegacySource::Unreadable,
                ..LegacyChartGraphics::default()
            };
        }
    };
    let Ok(doc) = text.parse::<toml::Value>() else {
        log::warn!("{} не разобран", path.display());
        return LegacyChartGraphics {
            source: LegacySource::Unreadable,
            ..LegacyChartGraphics::default()
        };
    };

    // Same discriminator `ChartThemeSet::load` uses, deliberately: the two must never disagree
    // about which shape a file is in.
    let tables: Vec<&toml::Value> = if text.contains("[dark") || text.contains("[light") {
        let (first, second) = if mode == UiThemeMode::Light {
            ("light", "dark")
        } else {
            ("dark", "light")
        };
        [first, second]
            .into_iter()
            .filter_map(|key| doc.get(key))
            .collect()
    } else {
        vec![&doc]
    };

    let pick = |key: &str| -> Option<&toml::Value> { tables.iter().find_map(|t| t.get(key)) };

    LegacyChartGraphics {
        source: LegacySource::Read,
        marker_scale: pick("marker_scale").and_then(as_f32),
        trade_volume_alpha: pick("trade_volume_alpha").and_then(as_f32),
        candle_volume_style: pick("candle_volume_style").and_then(as_u8),
        candle_volume_height: pick("candle_volume_height").and_then(as_f32),
        candle_volume_alpha: pick("candle_volume_alpha").and_then(as_f32),
        candle_volume_scale: pick("candle_volume_scale").and_then(as_rgb),
    }
}

#[cfg(test)]
mod tests;
