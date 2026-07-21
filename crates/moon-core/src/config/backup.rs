//! Config snapshots in `<data_dir>/backups/` protect against one-way overwrites.
//!
//! EXACTLY two files are copied, the only ones that cannot be reconstructed:
//! - `servers.enc` — encrypted API keys (losing it means losing access to the cores);
//! - `cfg/settings.toml` — groups, enabled cores, chart bindings, and the uid counter.
//!
//! Everything else in `cfg/` (theme, layout, docks, hotkeys) can be recreated manually within
//! minutes, while the databases in `data/` are excluded because of their size. One caveat is
//! `strategies.sqlite`: according to `paths::strategies_db_path`, it is the sole copy of the
//! strategy version history, not a replica, and it has its OWN backup button on the Storage tab.
//! It is deliberately excluded here: snapshots must remain kilobytes in size so every deliberate
//! save can take one.
//!
//! There are exactly two triggers (see [`Trigger`]): schema migration and a save from the Settings
//! window. The routine `config_dirty` drain in the 100-ms loop, the final save on exit, and header
//! edits use ordinary `save()` WITHOUT a snapshot. They fire for small changes such as order-size
//! presets and would evict the valuable snapshots from all 30 slots within minutes.
//!
//! A snapshot NEVER breaks the operation it protects: [`snapshot`] does not return an error at all.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::util::time::{now_unix_ms_i64, utc_stamp_compact};

use super::paths;
use super::SnapshotOutcome;

/// Number of snapshots to retain. Older snapshots are removed automatically.
pub const SNAPSHOT_KEEP: usize = 30;

/// Prefix of the directory where a snapshot is ASSEMBLED.
///
/// It deliberately fails [`is_snapshot_name`], so neither pruning nor the user can see an
/// incomplete snapshot as a finished copy.
const STAGING_PREFIX: &str = ".incoming-";

/// Counter that makes staging directories unique within the process.
static STAGING_SEQ: AtomicU32 = AtomicU32::new(0);

/// Prefix for staging directories owned by THIS process: `.incoming-<pid>-`.
///
/// Pruning removes only these. Removing every `.incoming-*` is unsafe: another terminal instance
/// using the same portable directory may currently be copying into its own directory, and pruning
/// it would destroy that snapshot halfway through construction.
fn own_staging_prefix() -> String {
    format!("{STAGING_PREFIX}{}-", std::process::id())
}

/// Reason a snapshot was taken, logged to distinguish migrations from deliberate saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Trigger {
    /// The on-disk schema is outdated and is about to be saved automatically.
    SchemaMigration,
    /// The user clicked Save in the Settings window.
    SettingsSave,
}

impl Trigger {
    /// Short label used in log messages.
    fn label(self) -> &'static str {
        match self {
            Trigger::SchemaMigration => "миграция схемы",
            Trigger::SettingsSave => "сохранение настроек",
        }
    }
}

/// Copy both config files and prune old snapshots.
///
/// Deliberately returns [`SnapshotOutcome`], NOT `Result`: a backup must not break the operation it
/// protects, and removing the error channel makes that a property of the signature rather than a
/// convention that the next `?` can silently violate. Failure cannot be silent either, because
/// Settings would otherwise report success when no rollback copy was created.
pub(super) fn snapshot(trigger: Trigger) -> SnapshotOutcome {
    let sources = [paths::servers_path(), paths::settings_path()];
    let refs: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
    match snapshot_into(
        &refs,
        &paths::backups_dir(),
        now_unix_ms_i64(),
        SNAPSHOT_KEEP,
    ) {
        Ok(Some(dir)) => {
            log::info!(
                "конфиг: снимок ({}) → {}",
                trigger.label(),
                dir.file_name().unwrap_or_default().to_string_lossy()
            );
            SnapshotOutcome::Ok
        }
        // Nothing existed to copy on first launch; this is not a failure.
        Ok(None) => {
            log::debug!(
                "конфиг: снимок ({}) пропущен — нечего копировать",
                trigger.label()
            );
            SnapshotOutcome::Ok
        }
        Err(e) => {
            log::warn!("конфиг: снимок ({}) не удался: {e:#}", trigger.label());
            SnapshotOutcome::Failed
        }
    }
}

/// Testable core of [`snapshot`]: every path is injected and `paths::` is not used.
///
/// Accepts FULL source paths rather than root directories so this module does not duplicate file
/// names owned by `config::paths`.
///
/// Returns `Ok(None)` when no source exists on disk, as on first launch. In that case the
/// `backups/` directory is not created at all.
fn snapshot_into(
    sources: &[&Path],
    backups: &Path,
    now_ms: i64,
    keep: usize,
) -> anyhow::Result<Option<PathBuf>> {
    // Do NOT use `is_file()`: it returns `false` both when a file is absent and when metadata
    // cannot be read (permissions, a share, or an unhydrated cloud placeholder). Silently dropping
    // an unreadable source would publish a ONE-file snapshot as successful, after which saving
    // would overwrite both files. Absence is not a failure; every other outcome is.
    let mut present: Vec<&Path> = Vec::new();
    for src in sources.iter().copied() {
        match std::fs::metadata(src) {
            Ok(m) if m.is_file() => present.push(src),
            Ok(_) => anyhow::bail!("источник снимка не файл: {}", src.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::from(e)
                    .context(format!("источник снимка не читается: {}", src.display())))
            }
        }
    }
    if present.is_empty() {
        return Ok(None);
    }

    std::fs::create_dir_all(backups)?;

    // Assemble in a temporary directory and publish only as a complete unit. Otherwise, failure
    // during the second copy would leave a directory that passes `is_snapshot_name`, consumes a
    // retention slot, and can evict a COMPLETE snapshot.
    let staging = backups.join(format!(
        "{}{}",
        own_staging_prefix(),
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

    // Use ALL sources, not only those present now: pruning needs every name this module can ever
    // write, not the subset observed during one run.
    let expected: Vec<&str> = sources
        .iter()
        .filter_map(|p| p.file_name()?.to_str())
        .collect();
    prune(backups, keep, &expected);

    Ok(Some(dir))
}

/// Publish an assembled staging directory under its final timestamped name.
///
/// Publication is ONE rename of the entire directory rather than creation of the final directory
/// followed by moving files individually. A failure after the first move would leave a correctly
/// named directory with incomplete contents that passes [`is_snapshot_name`], consumes a retention
/// slot, and can evict a COMPLETE snapshot. A single `rename` prevents this: the snapshot is either
/// visible in full or not visible at all.
///
/// Renaming a directory onto an EXISTING name fails on Windows and, for a non-empty directory, on
/// Unix, so `fs::rename` also acts as an atomic claim on the name.
///
/// ACCEPTED RESIDUAL RISK: on Unix, `rename` MAY replace an existing EMPTY directory, and the
/// preceding `exists()` check is a TOCTOU race. Triggering it requires an empty timestamp-named
/// directory, which this code never creates: this function is the sole producer of final names and
/// creates them by renaming an already POPULATED staging directory. The remaining possibilities are
/// a directory created manually by the user or a second process running an older code version.
/// Claiming the name first with `fs::create_dir` and then filling it would trade this narrow
/// cross-version race for a broad one: process death during population would leave a correctly
/// named directory with incomplete contents. Process death is more likely than two versions running
/// concurrently, so `rename` is the chosen tradeoff.
fn publish(staging: &Path, backups: &Path, now_ms: i64) -> anyhow::Result<PathBuf> {
    let stamp = utc_stamp_compact(now_ms);
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..=99u32 {
        let name = if attempt == 0 {
            stamp.clone()
        } else {
            // As strings, `20260721-134501` < `20260721-134501-01` < `20260721-134502`, so the
            // suffix preserves the invariant that lexicographic order equals chronological order.
            format!("{stamp}-{attempt:02}")
        };
        let dst = backups.join(&name);
        if dst.exists() {
            continue;
        }
        match std::fs::rename(staging, &dst) {
            Ok(()) => return Ok(dst),
            // Another writer claimed the name between the check and rename; try the next one.
            Err(e) => last = Some(e),
        }
    }
    match last {
        Some(e) => {
            Err(anyhow::Error::from(e).context(format!("публикация снимка {stamp} не удалась")))
        }
        None => anyhow::bail!("все 100 имён снимка на секунду {stamp} заняты"),
    }
}

/// Return whether a directory name looks like a snapshot created by THIS module.
///
/// Strictly accepts `YYYYMMDD-HHMMSS` (15 characters) or the same name with a collision suffix
/// `-NN` (18 characters). Everything else is a user directory and remains invisible to pruning.
fn is_snapshot_name(name: &str) -> bool {
    let digits_at = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    match name.len() {
        15 => name.as_bytes()[8] == b'-' && digits_at(&name[..8]) && digits_at(&name[9..]),
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

/// Keep the newest `keep` snapshots, remove older ones, and clean up abandoned staging directories.
///
/// Returns the number of snapshots removed.
///
/// Caution matters more than brevity because this directory lives in the user's folder:
/// - a symlinked root is ignored because `read_dir` would enter an unrelated tree BEFORE any child
///   checks;
/// - child types come from `DirEntry::file_type`, which does NOT follow symlinks;
/// - only EXPECTED file names are removed, after which the directory is removed with non-recursive
///   `remove_dir`. It refuses to remove a non-empty directory, so anything placed there by the user
///   or a cloud client preserves both itself and the snapshot.
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
    let own_prefix = own_staging_prefix();
    for entry in rd.flatten() {
        // This does not follow symlinks: a substituted link to an unrelated directory is not
        // classified as a directory and therefore never enters pruning.
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_snapshot_name(&name) {
            snapshots.push(name);
        } else if name.starts_with(&own_prefix) {
            stale_staging.push(entry.path());
        }
    }

    // Remove only OUR abandoned staging directory, left when an earlier call in this process
    // failed during assembly. Directories owned by other pids may contain live work from another
    // instance and remain untouched. They are litter, but safe litter: an incomplete config copy
    // in the user's folder that snapshot enumeration cannot see.
    for path in stale_staging {
        let _ = std::fs::remove_dir_all(path);
    }

    if snapshots.len() <= keep {
        return 0;
    }
    // Fixed-width names make name order equal time order. Deliberately do NOT use mtime because
    // file copying and cloud synchronization rewrite it.
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
    //! Snapshot behavior on paths where an error can cost user data.

    use super::*;

    /// Create a unique temporary root for one test without a dev-dependency on `tempfile`.
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

    /// Write a file after creating its parent directory.
    fn write(path: &Path, body: &str) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).expect("parent");
        }
        std::fs::write(path, body).expect("write");
    }

    /// Return the sorted snapshot names in a directory.
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

    /// Every snapshot must reread the file rather than reuse previously read contents.
    ///
    /// The plausible mutation this catches is caching source contents, for example by moving the
    /// read outside the loop or storing it statically to avoid reading the disk twice. The second
    /// snapshot would then preserve the FIRST snapshot's bytes, leaving every backup one save
    /// behind and restoring a different state from the one the user expects.
    #[test]
    fn each_snapshot_rereads_the_file_from_disk() {
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

        assert_eq!(
            std::fs::read_to_string(first.join("settings.toml")).unwrap(),
            "v1"
        );
        assert_eq!(
            std::fs::read_to_string(second.join("settings.toml")).unwrap(),
            "v2"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Retention prunes by NAME rather than modification time.
    ///
    /// Directories are created in REVERSE chronological order, making mtime order the opposite of
    /// name order. An implementation that prunes by mtime would remove the three NEWEST snapshots.
    /// This construction also catches any reliance on the enumeration order of `read_dir`.
    #[test]
    fn retention_keeps_the_newest_by_name_not_by_mtime() {
        let root = temp_root("retention");
        let backups = root.join("backups");
        std::fs::create_dir_all(&backups).unwrap();

        // Real consecutive dates spanning July and early August ensure that names are valid
        // timestamps rather than merely increasing strings.
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
        assert_eq!(
            survivors,
            names[3..].to_vec(),
            "the three OLDEST must be gone"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Pruning never touches anything the application did not create.
    ///
    /// The plausible mutation is weakening `is_snapshot_name` to "starts with a digit" to support
    /// an older naming scheme, which would delete the user's `2026-07-21` directory.
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

    /// A snapshot containing an unexpected file is not removed wholesale.
    ///
    /// Non-recursive `remove_dir` refuses to remove a non-empty directory, so a file placed there
    /// by the user or a cloud client preserves both itself and the snapshot. The plausible mutation
    /// is replacing it with `remove_dir_all`.
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
        assert!(
            dir.join("заметка.txt").exists(),
            "the foreign file must survive"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two snapshots taken in the same second do not overwrite each other.
    ///
    /// The plausible mutation is removing the collision suffix: `create_dir_all` accepts an
    /// existing directory and `fs::copy` overwrites files, so the second save would destroy the
    /// snapshot taken by the first. The defect becomes visible only when that snapshot is needed.
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
        assert_eq!(
            std::fs::read_to_string(a.join("settings.toml")).unwrap(),
            "first"
        );
        assert_eq!(
            std::fs::read_to_string(b.join("settings.toml")).unwrap(),
            "second"
        );
        assert_eq!(snapshot_names(&backups).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A missing config is not a failure and does not create the `backups/` directory.
    ///
    /// The plausible mutation is an unconditional `fs::copy(src, dst)?`. It turns EVERY first
    /// launch, and every launch after a corrupt file is moved to `.bak`, into a logged error. A
    /// later author may then "fix" it by propagating the error with `?` from saving, causing a
    /// failed backup to break the very save it must protect.
    #[test]
    fn a_missing_config_is_not_a_backup_failure() {
        let root = temp_root("absent");
        let backups = root.join("backups");

        let made = snapshot_into(&[&root.join("settings.toml")], &backups, 0, 30)
            .expect("an absent source is not an error");

        assert!(made.is_none(), "nothing to copy means no snapshot");
        assert!(
            !backups.exists(),
            "backups/ must not be created speculatively"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A source that EXISTS but is not a readable file must fail the entire snapshot rather than
    /// disappear from it silently.
    ///
    /// The plausible mutation this catches, and one that existed here, is filtering sources with
    /// `p.is_file()`. It returns `false` both when a file is absent and when metadata cannot be
    /// read, so an unreadable `servers.enc` silently disappeared while a ONE-file snapshot was
    /// published as successful. Saving then overwrote both files, leaving the rollback copy without
    /// API keys precisely when it was needed.
    ///
    /// A directory in place of a file is a portable way to produce "exists, but is not a file."
    #[test]
    fn an_unreadable_source_fails_instead_of_publishing_a_partial_snapshot() {
        let root = temp_root("partial");
        let good = root.join("settings.toml");
        let backups = root.join("backups");
        write(&good, "v1");
        let bogus = root.join("servers.enc");
        std::fs::create_dir_all(&bogus).unwrap();

        let res = snapshot_into(&[&good, &bogus], &backups, 0, 30);

        assert!(
            res.is_err(),
            "a source that exists but is not a file must fail the snapshot"
        );
        assert!(
            snapshot_names(&backups).is_empty(),
            "nothing may be published when a source could not be read"
        );
        let leftovers: Vec<String> = std::fs::read_dir(&backups)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with(STAGING_PREFIX))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            leftovers.is_empty(),
            "staging dirs must never survive: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
