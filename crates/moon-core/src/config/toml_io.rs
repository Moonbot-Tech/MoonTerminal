//! Общие load/save открытых TOML-файлов (settings/layout/theme): один паттерн
//! «нет файла → дефолт; битый → лог + on_corrupt + дефолт» вместо трёх копий.

use std::path::Path;

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
    std::fs::write(path, toml::to_string_pretty(value)?)
        .with_context(|| format!("запись {label}"))?;
    Ok(())
}
