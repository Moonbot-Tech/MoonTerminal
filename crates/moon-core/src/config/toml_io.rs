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
        return T::default();
    };
    match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("{label} повреждён ({e}); беру дефолт");
            on_corrupt(path);
            T::default()
        }
    }
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
