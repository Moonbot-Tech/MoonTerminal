//! Общие load/save открытых TOML-файлов (settings/layout/theme): один паттерн
//! «нет файла → дефолт; битый → лог + on_corrupt + дефолт» вместо трёх копий.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// How a config file loaded. The caller needs this to decide whether writing the file back is
/// safe — the four cases are NOT interchangeable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigLoad {
    /// Read and parsed. Whatever is in memory reflects the file.
    Present,
    /// No such file — a first run. Defaults are correct and saving them is correct.
    Absent,
    /// Present but unparseable, AND successfully quarantined by `on_corrupt` (moved to `.bak`).
    /// The original bytes survive, so writing defaults over the now-absent path is safe.
    ///
    /// A quarantine that FAILED reports [`ConfigLoad::Unreadable`] instead: the user's file is
    /// still sitting there, and authorizing a write over it would destroy it.
    Corrupt,
    /// The file may well exist and hold good data, but reading it FAILED — a permission or
    /// sharing error, or a cloud placeholder that could not be hydrated.
    ///
    /// The defaults returned alongside this are safe to USE but must never be SAVED: the caller
    /// would overwrite a healthy config with defaults on the strength of a transient read error.
    Unreadable,
}

/// Прочитать TOML в `T`, сообщив КАК он прочитался. Нет файла → дефолт (первый
/// запуск). Битый файл → лог, `on_corrupt(path)` (например, увод в `.bak`) и дефолт.
/// Нечитаемый файл → дефолт + [`ConfigLoad::Unreadable`], чтобы вызывающий не
/// перезаписал живой конфиг (см. `ConfigLoad`).
pub fn load_or_default_status<T: Default + DeserializeOwned>(
    path: &Path,
    label: &str,
    on_corrupt: impl FnOnce(&Path) -> bool,
) -> (T, ConfigLoad) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (defaults_for_absent_file(label), ConfigLoad::Absent);
        }
        // NOT the same as "absent". Distinguishing them is what stops a transient read failure
        // from being laundered into a permanent overwrite by the caller's re-save.
        Err(e) => {
            log::error!(
                "{label}: файл есть, но не читается ({e}); работаю на дефолтах В ПАМЯТИ и НЕ \
                 перезаписываю файл"
            );
            return (defaults_for_absent_file(label), ConfigLoad::Unreadable);
        }
    };
    match toml::from_str(&text) {
        Ok(v) => (v, ConfigLoad::Present),
        Err(e) => {
            log::warn!("{label} повреждён ({e}); беру дефолт");
            // Провалившийся карантин НЕ даёт права на запись: файл пользователя остался на
            // месте, и сохранение дефолтов уничтожило бы его.
            let status = if on_corrupt(path) {
                ConfigLoad::Corrupt
            } else {
                log::error!("{label}: карантин не удался — запись файла запрещена");
                ConfigLoad::Unreadable
            };
            (defaults_for_absent_file(label), status)
        }
    }
}

/// Как [`load_or_default_status`], но без статуса — для файлов раскладки/темы, где
/// нечитаемый файл не приводит к автоматической перезаписи.
pub fn load_or_default<T: Default + DeserializeOwned>(
    path: &Path,
    label: &str,
    on_corrupt: impl FnOnce(&Path),
) -> T {
    load_or_default_status(path, label, |p| {
        on_corrupt(p);
        true
    })
    .0
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
    use super::{load_or_default, load_or_default_status, ConfigLoad};
    use std::path::{Path, PathBuf};

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

    /// A file that exists but cannot be READ must not be reported as absent.
    ///
    /// The two used to be indistinguishable: `let Ok(text) = read_to_string(..) else { default }`
    /// mapped every error — permission denied, a sharing violation, a cloud placeholder that
    /// failed to hydrate — onto the same "first run" answer. Downstream that yields `version = 0`,
    /// which marks the config dirty, which drives the automatic re-save in `AppConfig::load` — so
    /// one transient read failure silently replaced a healthy `settings.toml` (groups, per-core
    /// active flags, chart bundles, the uid counter) with defaults.
    ///
    /// The plausible edit this catches: collapsing the match back to `let Ok(..) else` because it
    /// reads more cleanly. That compiles and behaves identically on every machine where the file
    /// reads fine, which is every developer machine.
    ///
    /// A directory is the portable way to force a non-`NotFound` read error: no platform returns
    /// `NotFound` for a path that exists but is not a file.
    #[test]
    fn an_unreadable_file_is_not_reported_as_absent() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        assert!(dir.is_dir(), "the fixture must be a directory that exists");

        let (_cfg, status) = load_or_default_status::<SettingsFile>(&dir, "settings.toml", |_| {
            panic!("on_corrupt must not fire for a file that could not be read at all")
        });

        assert_eq!(
            status,
            ConfigLoad::Unreadable,
            "a path that exists but does not read must report Unreadable, never Absent"
        );
    }

    /// A genuinely missing file still reports `Absent`, so the first run can save its defaults.
    #[test]
    fn a_missing_file_reports_absent() {
        let missing = Path::new("no-such-dir-4f21c8/no-such-settings.toml");
        assert!(
            !missing.exists(),
            "the fixture path must genuinely not exist"
        );

        let (_cfg, status) =
            load_or_default_status::<SettingsFile>(missing, "settings.toml", |_| {
                panic!("on_corrupt must not fire for a file that is merely absent")
            });

        assert_eq!(status, ConfigLoad::Absent);
    }
}
