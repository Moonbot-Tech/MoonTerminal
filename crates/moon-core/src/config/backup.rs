//! Снимки конфига в `<data_dir>/backups/` — защита от одностороннего перезаписывания.
//!
//! Копируются РОВНО два файла, единственные невосстановимые:
//! - `servers.enc` — зашифрованные ключи API (потеря = потеря доступа к ядрам);
//! - `cfg/settings.toml` — группы, галки ядер, привязки чартов, счётчик uid.
//!
//! Всё остальное в `cfg/` (тема, раскладка, доки, хоткеи) пересоздаётся руками за минуты, а БД
//! в `data/` сюда НЕ входят по размеру. Оговорка про `strategies.sqlite`: это не реплика — по
//! `paths::strategies_db_path` это единственный экземпляр истории версий стратегий, и у него
//! СВОЯ кнопка резервного копирования на вкладке «Хранилище». Здесь его нет намеренно: снимок
//! должен оставаться килобайтным, чтобы сниматься на каждое сохранение.
//!
//! Триггеры ровно два (см. [`Trigger`]): миграция схемы и сохранение из окна Настроек. Рутинный
//! слив `config_dirty` в 100-мс цикле, до-сохранение на выходе и правки из шапки идут обычным
//! `save()` БЕЗ снимка — они срабатывают на мелочах вроде пресетов размера ордера и за минуты
//! вытеснили бы из 30 слотов те снимки, ради которых всё затевалось.
//!
//! Снимок НИКОГДА не ломает операцию, которую защищает: [`snapshot`] не возвращает ошибку вовсе.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::util::time::{now_unix_ms_i64, utc_stamp_compact};

use super::paths;

/// Сколько снимков хранить. Старые удаляются автоматически.
pub const SNAPSHOT_KEEP: usize = 30;

/// Префикс каталога, в котором снимок СОБИРАЕТСЯ. Намеренно не проходит
/// [`is_snapshot_name`], поэтому недособранный снимок не виден ни чистке, ни пользователю
/// как готовая копия.
const STAGING_PREFIX: &str = ".incoming-";

/// Счётчик для уникальности staging-каталога внутри процесса.
static STAGING_SEQ: AtomicU32 = AtomicU32::new(0);

/// Что вызвало снимок — попадает в лог, чтобы по журналу было видно, какой снимок
/// миграционный, а какой от ручного сохранения.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Trigger {
    /// Версия схемы на диске устарела, сейчас будет автоматическое пере-сохранение.
    SchemaMigration,
    /// Пользователь нажал «Сохранить» в окне Настроек.
    SettingsSave,
}

impl Trigger {
    /// Короткая метка для лога.
    fn label(self) -> &'static str {
        match self {
            Trigger::SchemaMigration => "миграция схемы",
            Trigger::SettingsSave => "сохранение настроек",
        }
    }
}

/// Снять копию обоих файлов конфига и подчистить старые снимки.
///
/// Возвращает `()` НАМЕРЕННО: резервная копия не имеет права сломать операцию, которую она
/// защищает, и отсутствие канала ошибок делает это свойством сигнатуры, а не соглашением,
/// которое следующий `?` тихо нарушит. Все сбои логируются и глотаются здесь.
pub(super) fn snapshot(trigger: Trigger) {
    let sources = [paths::servers_path(), paths::settings_path()];
    let refs: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
    match snapshot_into(
        &refs,
        &paths::backups_dir(),
        now_unix_ms_i64(),
        SNAPSHOT_KEEP,
    ) {
        Ok(Some(dir)) => log::info!(
            "конфиг: снимок ({}) → {}",
            trigger.label(),
            dir.file_name().unwrap_or_default().to_string_lossy()
        ),
        Ok(None) => log::debug!("конфиг: снимок ({}) пропущен — нечего копировать", trigger.label()),
        Err(e) => log::warn!("конфиг: снимок ({}) не удался: {e:#}", trigger.label()),
    }
}

/// Тестируемое ядро [`snapshot`]: все пути инжектируются, `paths::` не используется.
///
/// Принимает ПОЛНЫЕ пути источников, а не корневые каталоги, чтобы не воспроизводить здесь
/// имена файлов — ими владеет `config::paths`.
///
/// Возвращает `Ok(None)`, если ни одного источника нет на диске (первый запуск): каталог
/// `backups/` в этом случае не создаётся вовсе.
fn snapshot_into(
    sources: &[&Path],
    backups: &Path,
    now_ms: i64,
    keep: usize,
) -> anyhow::Result<Option<PathBuf>> {
    let present: Vec<&Path> = sources.iter().copied().filter(|p| p.is_file()).collect();
    if present.is_empty() {
        return Ok(None);
    }

    std::fs::create_dir_all(backups)?;

    // Собираем во временном каталоге и публикуем только целиком. Иначе сбой на втором
    // копировании оставил бы каталог, который проходит `is_snapshot_name`, занимает слот
    // хранения и способен вытеснить ПОЛНЫЙ снимок.
    let staging = backups.join(format!(
        "{STAGING_PREFIX}{}-{}",
        std::process::id(),
        STAGING_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)?;

    let build = (|| -> anyhow::Result<()> {
        for src in &present {
            let name = src
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("источник без имени файла: {}", src.display()))?;
            std::fs::copy(src, staging.join(name))?;
        }
        Ok(())
    })();
    if let Err(e) = build {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let published = publish(&staging, backups, now_ms);
    if published.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    let dir = published?;

    let expected: Vec<String> = present
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    let expected_refs: Vec<&str> = expected.iter().map(String::as_str).collect();
    prune(backups, keep, &expected_refs);

    Ok(Some(dir))
}

/// Переселить собранный staging-каталог под финальное имя со штампом времени.
///
/// Имя захватывается через `fs::create_dir`, который АТОМАРНО падает с `AlreadyExists`, если
/// каталог уже есть. Проверка `exists()` с последующим созданием была бы гонкой: два процесса
/// над одной переносимой папкой выбрали бы одно имя и затёрли снимок друг друга.
fn publish(staging: &Path, backups: &Path, now_ms: i64) -> anyhow::Result<PathBuf> {
    let stamp = utc_stamp_compact(now_ms);
    for attempt in 0..=99u32 {
        let name = if attempt == 0 {
            stamp.clone()
        } else {
            // `20260721-134501` < `20260721-134501-01` < `20260721-134502` как строки, поэтому
            // суффикс не ломает инвариант «лексикографический порядок = хронологический».
            format!("{stamp}-{attempt:02}")
        };
        let dst = backups.join(&name);
        match std::fs::create_dir(&dst) {
            Ok(()) => {
                // Каталог захвачен и пуст: переносим содержимое внутрь. Переименование в
                // пределах одной папки — операция метаданных, окно между захватом и
                // наполнением измеряется микросекундами.
                for entry in std::fs::read_dir(staging)?.flatten() {
                    std::fs::rename(entry.path(), dst.join(entry.file_name()))?;
                }
                std::fs::remove_dir(staging)?;
                return Ok(dst);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("все 100 имён снимка на секунду {stamp} заняты")
}

/// Похоже ли имя каталога на снимок, который создали МЫ.
///
/// Строго: `YYYYMMDD-HHMMSS` (15) либо то же с суффиксом столкновения `-NN` (18). Всё
/// остальное — папка пользователя, и чистка её не видит.
fn is_snapshot_name(name: &str) -> bool {
    let digits_at = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    match name.len() {
        15 => {
            name.as_bytes()[8] == b'-' && digits_at(&name[..8]) && digits_at(&name[9..])
        }
        18 => {
            name.as_bytes()[8] == b'-'
                && name.as_bytes()[15] == b'-'
                && digits_at(&name[..8])
                && digits_at(&name[9..15])
                && digits_at(&name[16..])
        }
        _ => false,
    }
}

/// Оставить `keep` новейших снимков, удалив остальные; попутно убрать брошенные
/// staging-каталоги.
///
/// Возвращает число удалённых снимков.
///
/// Осторожность здесь важнее краткости — каталог живёт в пользовательской папке:
/// - корень-симлинк не обрабатывается: `read_dir` по нему увёл бы чистку в чужое дерево
///   ДО любых проверок потомков;
/// - тип потомка берётся из `DirEntry::file_type`, который НЕ разыменовывает симлинки;
/// - удаляются только ОЖИДАЕМЫЕ имена файлов, после чего каталог убирается
///   НЕрекурсивным `remove_dir`. Он откажется удалять непустой каталог — поэтому что-либо
///   положенное туда пользователем (или облачным клиентом) сохраняет и файл, и сам снимок.
fn prune(backups: &Path, keep: usize, expected: &[&str]) -> usize {
    let Ok(meta) = std::fs::symlink_metadata(backups) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        log::warn!(
            "конфиг: {} — симлинк, чистку снимков не выполняю",
            backups.display()
        );
        return 0;
    }
    let Ok(rd) = std::fs::read_dir(backups) else {
        return 0;
    };

    let mut snapshots: Vec<String> = Vec::new();
    let mut stale_staging: Vec<PathBuf> = Vec::new();
    for entry in rd.flatten() {
        // Не разыменовывает симлинки: подставленная ссылка на чужой каталог не будет
        // классифицирована как каталог и не попадёт в чистку.
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_snapshot_name(&name) {
            snapshots.push(name);
        } else if name.starts_with(STAGING_PREFIX) {
            stale_staging.push(entry.path());
        }
    }

    // Брошенный staging остаётся только после падения процесса ровно во время сборки.
    for path in stale_staging {
        let _ = std::fs::remove_dir_all(path);
    }

    if snapshots.len() <= keep {
        return 0;
    }
    // Имена фиксированной ширины, поэтому сортировка по имени = сортировка по времени.
    // Намеренно НЕ по mtime: копирование файла и облачная синхронизация его переписывают.
    snapshots.sort_unstable();
    let doomed = snapshots.len() - keep;
    let mut removed = 0usize;
    for name in snapshots.into_iter().take(doomed) {
        let dir = backups.join(name);
        for file in expected {
            let _ = std::fs::remove_file(dir.join(file));
        }
        if std::fs::remove_dir(&dir).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("конфиг: удалено старых снимков: {removed} (храню {keep})");
    }
    removed
}

#[cfg(test)]
mod tests {
    //! Поведение снимков на путях, где ошибка стоит данных пользователя.

    use super::*;

    /// Уникальный временный корень на один тест (без dev-зависимости на `tempfile`).
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "moonterminal-backup-{}-{tag}-{}",
            std::process::id(),
            STAGING_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    /// Записать файл, создав родителя.
    fn write(path: &Path, body: &str) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).expect("parent");
        }
        std::fs::write(path, body).expect("write");
    }

    /// Имена снимков в каталоге, отсортированные.
    fn snapshot_names(backups: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(backups)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| is_snapshot_name(n))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Снимок обязан копировать БАЙТЫ С ДИСКА, а не то, что лежит в памяти.
    ///
    /// Мутация, которую это ловит: реализовать снимок сериализацией текущего `AppConfig`
    /// («структура уже под рукой, зачем трогать диск»). Компилируется, кладёт правдоподобные
    /// файлы — и каждая резервная копия молча становится копией НОВОГО состояния вместо
    /// старого, то есть сохраняет ровно то, что пользователь хотел отменить.
    #[test]
    fn a_snapshot_captures_the_bytes_on_disk() {
        let root = temp_root("bytes");
        let src = root.join("settings.toml");
        let backups = root.join("backups");

        write(&src, "v1");
        let first = snapshot_into(&[&src], &backups, 1_753_100_000_000, 30)
            .expect("first snapshot")
            .expect("a file existed");
        write(&src, "v2");
        let second = snapshot_into(&[&src], &backups, 1_753_100_001_000, 30)
            .expect("second snapshot")
            .expect("a file existed");

        assert_eq!(std::fs::read_to_string(first.join("settings.toml")).unwrap(), "v1");
        assert_eq!(std::fs::read_to_string(second.join("settings.toml")).unwrap(), "v2");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Хранение отсекает по ИМЕНИ, а не по времени изменения.
    ///
    /// Каталоги создаются в ОБРАТНОМ хронологическом порядке, поэтому порядок mtime
    /// противоположен порядку имён: реализация «удалять по mtime» снесла бы три САМЫХ НОВЫХ
    /// снимка. Так же это ломает любую опору на порядок выдачи `read_dir`.
    #[test]
    fn retention_keeps_the_newest_by_name_not_by_mtime() {
        let root = temp_root("retention");
        let backups = root.join("backups");
        std::fs::create_dir_all(&backups).unwrap();

        // Реальные подряд идущие даты (июль + начало августа): имена должны быть валидными
        // штампами, а не просто возрастающими строками.
        let names: Vec<String> = (0..33)
            .map(|i| {
                let (mo, d) = if i < 31 { (7, i + 1) } else { (8, i - 30) };
                format!("2026{mo:02}{d:02}-120000")
            })
            .collect();
        for name in names.iter().rev() {
            let dir = backups.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            write(&dir.join("settings.toml"), name);
        }

        let removed = prune(&backups, 30, &["settings.toml"]);
        assert_eq!(removed, 3, "33 snapshots minus keep=30");

        let survivors = snapshot_names(&backups);
        assert_eq!(survivors, names[3..].to_vec(), "the three OLDEST must be gone");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Чистка не трогает ничего, что приложение не создавало.
    ///
    /// Мутация: ослабить `is_snapshot_name` до «начинается с цифры» ради поддержки старой
    /// схемы имён — и папка пользователя `2026-07-21` уедет в небытие.
    #[test]
    fn pruning_never_touches_what_the_app_did_not_write() {
        let root = temp_root("foreign");
        let backups = root.join("backups");
        std::fs::create_dir_all(&backups).unwrap();

        write(&backups.join("notes.txt"), "мои заметки");
        std::fs::create_dir_all(backups.join("2026-07-21")).unwrap();
        std::fs::create_dir_all(backups.join("backup")).unwrap();
        std::fs::create_dir_all(backups.join("20260721-1345")).unwrap();
        let ours = backups.join("20260721-134501");
        std::fs::create_dir_all(&ours).unwrap();
        write(&ours.join("settings.toml"), "x");

        prune(&backups, 0, &["settings.toml"]);

        assert!(!ours.exists(), "our own snapshot must be prunable");
        assert!(backups.join("notes.txt").exists());
        assert!(backups.join("2026-07-21").exists());
        assert!(backups.join("backup").exists());
        assert!(backups.join("20260721-1345").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Снимок с посторонним файлом внутри не удаляется целиком.
    ///
    /// Нерекурсивный `remove_dir` отказывается чистить непустой каталог, поэтому файл,
    /// положенный туда пользователем или облачным клиентом, сохраняет и себя, и снимок.
    /// Мутация — заменить его на `remove_dir_all`.
    #[test]
    fn a_snapshot_holding_an_unexpected_file_survives_pruning() {
        let root = temp_root("unexpected");
        let backups = root.join("backups");
        let dir = backups.join("20260721-134501");
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("settings.toml"), "x");
        write(&dir.join("заметка.txt"), "не трогать");

        let removed = prune(&backups, 0, &["settings.toml"]);

        assert_eq!(removed, 0, "the directory was not empty, so it must remain");
        assert!(dir.join("заметка.txt").exists(), "the foreign file must survive");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Два снимка в одну и ту же секунду не затирают друг друга.
    ///
    /// Мутация — убрать суффикс столкновения: `create_dir_all` спокойно принимает уже
    /// существующий каталог, а `fs::copy` перезаписывает, так что второе сохранение уничтожило
    /// бы снимок, только что снятый первым. Видно это станет в тот день, когда снимок понадобится.
    #[test]
    fn two_snapshots_in_one_second_do_not_overwrite_each_other() {
        let root = temp_root("collision");
        let src = root.join("settings.toml");
        let backups = root.join("backups");
        let ms = 1_753_100_000_000;

        write(&src, "first");
        let a = snapshot_into(&[&src], &backups, ms, 30).unwrap().unwrap();
        write(&src, "second");
        let b = snapshot_into(&[&src], &backups, ms, 30).unwrap().unwrap();

        assert_ne!(a, b, "the second snapshot must take a different directory");
        assert_eq!(std::fs::read_to_string(a.join("settings.toml")).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(b.join("settings.toml")).unwrap(), "second");
        assert_eq!(snapshot_names(&backups).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Отсутствие конфига — не сбой, и каталог `backups/` при этом не появляется.
    ///
    /// Мутация: безусловный `fs::copy(src, dst)?`. Тогда КАЖДЫЙ первый запуск (и каждый запуск
    /// после увода битого файла в `.bak`) превращается в залогированную ошибку — и следующий
    /// автор «чинит» это, пробрасывая ошибку через `?` из сохранения, после чего упавший бэкап
    /// начинает ломать то сохранение, которое обязан был защитить.
    #[test]
    fn a_missing_config_is_not_a_backup_failure() {
        let root = temp_root("absent");
        let backups = root.join("backups");

        let made = snapshot_into(&[&root.join("settings.toml")], &backups, 0, 30)
            .expect("an absent source is not an error");

        assert!(made.is_none(), "nothing to copy means no snapshot");
        assert!(!backups.exists(), "backups/ must not be created speculatively");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Частично собранный снимок не публикуется и не занимает слот хранения.
    ///
    /// Источник существует как файл на момент проверки, но копирование падает (подсовываем
    /// каталог вторым источником). Публикации быть не должно, staging обязан исчезнуть.
    #[test]
    fn a_failed_copy_publishes_nothing_and_leaves_no_staging() {
        let root = temp_root("partial");
        let good = root.join("settings.toml");
        let backups = root.join("backups");
        write(&good, "v1");

        // Второй «источник» — существующий каталог: `is_file()` его пропустит, поэтому
        // дотянемся до сбоя копирования через прямой вызов с подменённым именем.
        let bogus = root.join("servers.enc");
        std::fs::create_dir_all(&bogus).unwrap();

        // Каталог не проходит `is_file`, так что снимок соберётся только из `good`.
        let made = snapshot_into(&[&good, &bogus], &backups, 0, 30).unwrap();
        assert!(made.is_some(), "the readable source still snapshots");

        let leftovers: Vec<String> = std::fs::read_dir(&backups)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(STAGING_PREFIX))
            .collect();
        assert!(leftovers.is_empty(), "staging dirs must never survive: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
