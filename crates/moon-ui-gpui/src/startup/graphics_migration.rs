//! One-shot carry-over of the trade-mark and bottom-volume settings out of `theme.toml`.
//!
//! Those six values used to live on `ChartTheme` and are now fields of
//! [`moon_core::config::ChartGraphicsCfg`], which is per chart TAB. Nothing about that move is
//! automatic for an existing install: once the fields left `ChartTheme`, serde stops reading the
//! keys, so without this pass every user would silently land on stock values.
//!
//! A successful pass runs once, guarded by `WindowLayout::chart_graphics_from_theme_migrated`; a
//! deferred pass retries on the next startup. Every pass must run BEFORE anything can rewrite
//! `theme.toml` — see the call site in `startup`.

use moon_core::config::ChartGraphicsCfg;
use moon_core::config::layout::WindowLayout;
use moon_core::config::paths;
use moon_core::config::theme_legacy::{self, LegacyChartGraphics, LegacySource};

use crate::persistence::chart_persist::ChartTabSpec;

/// Fold the legacy values into one config, leaving every absent field at its new default.
///
/// Starts from `ChartGraphicsCfg::default()` rather than reaching for the `def_*` helpers behind it:
/// those are private to `moon_core::config::layout` and deliberately stay that way, so `Default` is
/// the one public home of every shipped value and this crate needs no second copy of any of them.
///
/// The `marker_scale` rule is the one judgement call here and it is not recoverable from the file.
/// `ChartTheme` was `#[serde(default)]` and its save serialized the WHOLE struct, so every
/// `theme.toml` ever written carries `marker_scale = 1.0` — whether the user chose it or never
/// opened Settings. "Untouched" and "explicitly 1.0" are byte-identical on disk. Since 1.0 was the
/// shipped default and shrinking it is the point of this change, exactly 1.0 is read as untouched
/// and becomes the new default; anything else is the user's own number and carries over verbatim.
/// `1x` is one click away in the popup for the minority who did choose it.
///
/// Args:
///     legacy: Whatever the old theme file still carried.
///
/// Returns:
///     The settings to store, NORMALIZED — the migration is a store site like any other, and a
///     hand-edited `nan` reaching `VolumeStyleGpu` would rebake the cached band texture forever.
fn resolve(legacy: LegacyChartGraphics) -> ChartGraphicsCfg {
    let mut cfg = ChartGraphicsCfg::default();
    if let Some(v) = legacy.marker_scale {
        // Anything but the old default is a deliberate choice and survives untouched.
        if v != 1.0 {
            cfg.marker_scale = v;
        }
    }
    if let Some(v) = legacy.trade_volume_alpha {
        cfg.trade_volume_alpha = v;
    }
    if let Some(v) = legacy.candle_volume_style {
        cfg.candle_volume_style = v;
    }
    if let Some(v) = legacy.candle_volume_height {
        cfg.candle_volume_height = v;
    }
    if let Some(v) = legacy.candle_volume_alpha {
        cfg.candle_volume_alpha = v;
    }
    if let Some(v) = legacy.candle_volume_scale {
        cfg.candle_volume_scale = v;
    }
    moon_chart::normalize_chart_graphics(cfg)
}

/// Copy the six migrated values into one tab spec, leaving its own five settings alone.
fn stamp(spec: &mut ChartGraphicsCfg, from: ChartGraphicsCfg) {
    spec.marker_scale = from.marker_scale;
    spec.trade_volume_alpha = from.trade_volume_alpha;
    spec.candle_volume_style = from.candle_volume_style;
    spec.candle_volume_height = from.candle_volume_height;
    spec.candle_volume_alpha = from.candle_volume_alpha;
    spec.candle_volume_scale = from.candle_volume_scale;
}

/// Carry the six legacy values into the global default and into every tab that holds an override.
///
/// Stamping the existing overrides is not a guess. A spec carrying `Some(cfg)` was written by a
/// build in which these six values COULD NOT be per tab, so that tab was drawing the global theme
/// value; writing the migrated value in restores exactly what it drew. Without this, the tabs a user
/// customized most are the only ones that lose their appearance — the other five fields in each spec
/// are untouched.
///
/// It does NOT set the one-shot marker and it does NOT save anything. Both are the caller's, because
/// the marker may only be committed once the tab specs are known to be on disk; see the call site.
///
/// Args:
///     layout: The loaded window layout, whose `chart_graphics` becomes the new global default.
///     specs: Every loaded chart tab spec; those with an override are stamped in place.
///
/// Returns:
///     True when the migration ran and the caller must now persist, false when it had already run.
pub(super) fn migrate_chart_graphics_from_theme(
    layout: &mut WindowLayout,
    specs: &mut [ChartTabSpec],
) -> bool {
    if layout.chart_graphics_from_theme_migrated {
        return false;
    }
    // Before anything else reads or writes: the leftover keys in `theme.toml` are NOT a recovery
    // copy, because `AppConfig::save_impl` calls `ChartThemeSet::save` on every settings write and
    // that write drops them. This copy is the only durable one, and it is also what a RETRY reads
    // once the live file has been stripped — see `read_legacy_chart_graphics`.
    if !theme_legacy::backup_legacy_theme_file() && paths::theme_path().exists() {
        // A failed copy is NOT a reason to defer, and deferring here would cause exactly the loss
        // the copy exists to prevent. `unlock::start` runs later in THIS SAME launch and reaches
        // `ChartThemeSet::load`, whose legacy-flat branch re-saves `theme.toml` at once — without
        // the six keys, because `ChartTheme` no longer has them. Deferring would therefore hand the
        // next launch a file that has already been stripped, and it would read stock values and
        // commit them as the user's own. Reading the still-intact file NOW is the only moment that
        // works, so carry on with no safety net rather than with no data.
        log::warn!(
            "не удалось сделать копию {}; переношу настройки графики чарта без запасной копии",
            theme_legacy::backup_path().display()
        );
    }

    let legacy = theme_legacy::read_legacy_chart_graphics();
    if legacy.source == LegacySource::Unreadable {
        // A theme file EXISTS and we failed to read it — locked, half-written, bad permissions.
        // Committing the marker here would freeze the user on stock values forever, so leave the
        // marker unset and let the next launch look again.
        log::warn!(
            "перенос настроек графики чарта отложен: тема не прочитана, повторю при следующем запуске"
        );
        return false;
    }
    let resolved = resolve(legacy);
    stamp(&mut layout.chart_graphics, resolved);
    for spec in specs.iter_mut() {
        if let Some(cfg) = spec.chart_graphics.as_mut() {
            stamp(cfg, resolved);
        }
    }
    true
}
