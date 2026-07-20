//! Общие load/save открытых TOML-файлов (settings/layout/theme): один паттерн
//! «нет файла → дефолт; битый → лог + on_corrupt + дефолт» вместо трёх копий.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Прочитать TOML в `T`. Нет файла → дефолт (первый запуск). Битый файл →
/// лог, `on_corrupt(path)` (например, увод в `.bak`) и дефолт — не падаем и
/// не теряем данные молча.
pub fn load_or_default<T: Default + DeserializeOwned>(
    path: &Path,
    label: &str,
    on_corrupt: impl FnOnce(&Path),
) -> T {
    let Ok(text) = std::fs::read_to_string(path) else {
        return defaults_for_absent_file(label);
    };
    match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("{label} повреждён ({e}); беру дефолт");
            on_corrupt(path);
            defaults_for_absent_file(label)
        }
    }
}

/// Defaults for a file that is absent or unreadable, built by deserializing an EMPTY TOML
/// document rather than by calling `T::default()`.
///
/// The two are not interchangeable, and the difference is a silent data bug. A field whose real
/// default is declared as `#[serde(default = "...")]` gets that value only while DESERIALIZING —
/// a derived `Default` hands back `0` / `false` / `""` instead. So a config file that merely
/// LACKED a field came out correct, while a config file that was MISSING ENTIRELY came out zeroed.
/// That is how an absent `settings.toml` produced `ui_scale = 0.0` instead of `1.0` and collapsed
/// every hit rectangle in the UI to zero size.
///
/// Parsing an empty document runs the exact same defaulting path a present-but-incomplete file
/// takes, so "no file" and "empty file" can no longer disagree. `T::default()` stays as the
/// fallback for a type that genuinely cannot be built from nothing.
fn defaults_for_absent_file<T: Default + DeserializeOwned>(label: &str) -> T {
    toml::from_str("").unwrap_or_else(|e| {
        log::warn!("{label}: пустой документ не разбирается ({e}); беру Default");
        T::default()
    })
}

/// Записать значение как человекочитаемый TOML.
pub fn save<T: Serialize>(path: &Path, value: &T, label: &str) -> anyhow::Result<()> {
    write_atomic(path, toml::to_string_pretty(value)?.as_bytes(), label)?;
    Ok(())
}

/// Записать файл через временный sibling + rename. Прямой `fs::write` может оставить
/// обрубленный конфиг при падении процесса или питания ровно во время записи.
pub(super) fn write_atomic(path: &Path, bytes: &[u8], label: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| format!("создание папки для {label}"))?;
    }
    let tmp = atomic_tmp_path(path);
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .with_context(|| format!("создание временного {label}"))?;
        file.write_all(bytes)
            .with_context(|| format!("запись временного {label}"))?;
        file.sync_all()
            .with_context(|| format!("flush временного {label}"))?;
        drop(file);
        std::fs::rename(&tmp, path).with_context(|| format!("замена {label}"))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result
}

fn atomic_tmp_path(path: &Path) -> PathBuf {
    let pid = std::process::id();
    let mut ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map_or_else(|| "tmp".to_string(), |e| format!("{e}.tmp"));
    ext.push('.');
    ext.push_str(&pid.to_string());
    path.with_extension(ext)
}

#[cfg(test)]
/// Defaulting contract for config files that are absent from disk.
mod tests {
    use super::super::schema::{default_ui_font_delta, default_ui_scale, SettingsFile};
    use super::load_or_default;
    use std::path::PathBuf;

    /// Pins the absent-file branch of [`load_or_default`] to `defaults_for_absent_file`.
    ///
    /// The plausible edit: someone reads `defaults_for_absent_file` as a pointless wrapper and
    /// collapses it back to `T::default()`. That compiles, reads as a simplification, and is
    /// wrong — `#[serde(default = "...")]` runs only while deserializing, so a derived `Default`
    /// zeroes every field whose real default lives in such an attribute. Shipped once already:
    /// an absent `settings.toml` loaded `ui_scale` as `0.0` instead of `1.0`, which scaled every
    /// hit rectangle to zero size and left the UI visible but unclickable.
    ///
    /// The oracle is independent of the loader: `schema::default_*` are the same functions the
    /// fresh-config path in `config::mod` uses, so loader and fresh-config cannot drift apart.
    #[test]
    fn an_absent_file_yields_the_schema_defaults_not_zeroes() {
        let missing = PathBuf::from("no-such-dir-4f21c8/no-such-settings.toml");
        assert!(
            !missing.exists(),
            "the fixture path must genuinely not exist"
        );

        let cfg: SettingsFile = load_or_default(&missing, "settings.toml", |_| {
            panic!("on_corrupt must not fire for a file that is merely absent")
        });

        assert_eq!(
            cfg.ui_scale,
            default_ui_scale(),
            "an absent settings.toml must load the schema default scale, not f32::default()"
        );
        assert_eq!(
            cfg.ui_font_delta,
            default_ui_font_delta(),
            "an absent settings.toml must load the schema default font delta, not f32::default()"
        );
    }
}
