//! Общие load/save открытых TOML-файлов (settings/layout/theme): один паттерн
//! «нет файла → дефолт; битый → лог + on_corrupt + дефолт» вместо трёх копий.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Результат загрузки конфига, по которому вызывающий решает, безопасна ли обратная запись.
/// Четыре исхода НЕ взаимозаменяемы.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigLoad {
    /// Файл прочитан и разобран; состояние в памяти отражает его содержимое.
    Present,
    /// Файла нет — первый запуск; дефолты можно безопасно сохранить.
    Absent,
    /// Файл есть, но не разбирается, и `on_corrupt` успешно увёл его в `.bak`.
    /// Исходные байты сохранены, поэтому отсутствующий путь можно заполнить дефолтами.
    ///
    /// Неудачный карантин даёт [`ConfigLoad::Unreadable`]: файл пользователя остаётся на месте,
    /// и разрешение записи уничтожило бы его.
    Corrupt,
    /// Файл может существовать и содержать корректные данные, но не читаться из-за прав,
    /// sharing-ошибки или невыгруженного облачного плейсхолдера.
    ///
    /// Возвращённые дефолты можно использовать в памяти, но нельзя сохранять: временная ошибка
    /// чтения не должна приводить к перезаписи исправного конфига.
    Unreadable,
}

impl ConfigLoad {
    /// Можно ли записывать поверх этого файла.
    ///
    /// Намеренно ИСЧЕРПЫВАЮЩИЙ `match`, а не сравнение с одним «плохим» вариантом. Сравнение
    /// вида `== Unreadable` открыто по умолчанию: пятый вариант, добавленный позже (обрыв
    /// чтения, сбой расшифровки, таймаут выгрузки облачного плейсхолдера), молча посчитался
    /// бы пригодным для записи — то есть ровно тот отказ, против которого всё это и сделано.
    /// Здесь новый вариант не соберётся, пока автор не укажет его сторону.
    pub fn permits_overwrite(self) -> bool {
        match self {
            // Файл прочитан, отсутствует или уже уведён в `.bak` — исходных байтов под
            // записью либо нет, либо они сохранены.
            ConfigLoad::Present | ConfigLoad::Absent | ConfigLoad::Corrupt => true,
            // Файл пользователя на месте и не прочитан: писать нельзя.
            ConfigLoad::Unreadable => false,
        }
    }
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
        // Это НЕ отсутствие файла: различие не даёт вызывающему превратить временный сбой чтения
        // в постоянную перезапись при досейве.
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

/// Построить дефолты отсутствующего или нечитаемого файла десериализацией ПУСТОГО TOML,
/// а не вызовом `T::default()`.
///
/// Эти пути не взаимозаменяемы. Поле с `#[serde(default = "...")]` получает заданное значение
/// только при ДЕСЕРИАЛИЗАЦИИ, а производный `Default` возвращает `0` / `false` / `""`. Поэтому
/// прямой `T::default()` для отсутствующего файла дал бы, например, `ui_scale = 0.0` вместо `1.0`
/// и схлопнул бы все области клика UI до нулевого размера.
///
/// Разбор пустого документа запускает тот же путь дефолтов, что и неполный существующий файл.
/// `T::default()` остаётся запасным вариантом для типа, который нельзя построить из пустого TOML.
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
/// Контракт дефолтов для отсутствующих на диске файлов конфига.
mod tests {
    use super::super::schema::{default_ui_font_delta, default_ui_scale, SettingsFile};
    use super::{load_or_default, load_or_default_status, ConfigLoad};
    use std::path::{Path, PathBuf};

    /// Привязывает ветку отсутствующего файла в [`load_or_default`] к
    /// `defaults_for_absent_file`.
    ///
    /// Возможная поломка: счесть `defaults_for_absent_file` лишней обёрткой и заменить её на
    /// `T::default()`. Это компилируется, но `#[serde(default = "...")]` работает только при
    /// десериализации, поэтому производный `Default` обнулит поля с настоящим serde-дефолтом;
    /// `ui_scale = 0.0` оставит видимый UI без областей клика.
    ///
    /// Оракул независим от загрузчика: `schema::default_*` — те же функции, которые использует
    /// ветка свежего конфига в `config::mod`, поэтому два пути не могут разойтись.
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

    /// Существующий, но НЕЧИТАЕМЫЙ файл нельзя считать отсутствующим.
    ///
    /// Конструкция `let Ok(text) = read_to_string(..) else { default }` сводит любую ошибку —
    /// отказ в доступе, sharing violation или невыгруженный облачный плейсхолдер — к ответу
    /// «первый запуск». Получившийся `version = 0` помечает конфиг грязным и запускает досейв в
    /// `AppConfig::load`, который заменил бы исправный `settings.toml` дефолтами.
    ///
    /// Возможная поломка: свернуть `match` в более короткий `let Ok(..) else`. На машине, где
    /// файл читается исправно, такой вариант ведёт себя одинаково и не выдаёт риск при проверке.
    ///
    /// Каталог — переносимый способ получить ошибку чтения не типа `NotFound`: ни одна платформа
    /// не сообщает `NotFound` для существующего пути, который не является файлом.
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

    /// Действительно отсутствующий файл даёт `Absent`, чтобы первый запуск мог сохранить дефолты.
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
