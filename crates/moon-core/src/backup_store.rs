//! Shared filesystem lifecycle for application-managed snapshot directories.
//!
//! Domain modules assemble their own contents and define their exact name grammar and retention
//! policy. This module owns the safety-sensitive mechanics they must not reimplement: protected
//! roots, process-unique staging, stale staging cleanup, atomic publication, and nonrecursive
//! deletion that preserves a whole snapshot when foreign content appears.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

/// Prefix that keeps incomplete directories outside every completed-name grammar.
pub(crate) const STAGING_PREFIX: &str = ".incoming-";
/// Inactivity required before a staging directory can be treated as a crash leftover.
const STAGING_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// Shared sequence for staging claims made by every backup domain in this process.
static STAGING_SEQ: AtomicU32 = AtomicU32::new(0);

/// Result of publishing an exact-name snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExactPublication {
    /// This process atomically published its staged directory.
    Created(PathBuf),
    /// A concurrent process published and domain validation accepted the winner.
    Existing(PathBuf),
}

/// Filesystem store for one backup domain.
pub(crate) struct SnapshotStore<'a> {
    /// Domain subdirectory such as `backups/settings` or `backups/strategies`.
    root: &'a Path,
    /// Every regular filename this domain may place in a completed snapshot.
    expected: &'a [&'a str],
    /// Whether a removable snapshot must contain every expected file.
    require_all: bool,
}

impl<'a> SnapshotStore<'a> {
    /// Create a domain store without touching the filesystem.
    ///
    /// Args:
    ///     root: Domain backup directory beneath the shared `backups` parent.
    ///     expected: Complete allowlist of regular files owned inside one snapshot.
    ///     require_all: Whether all allowlisted files must exist before removal.
    pub(crate) fn new(root: &'a Path, expected: &'a [&'a str], require_all: bool) -> Self {
        Self {
            root,
            expected,
            require_all,
        }
    }

    /// Claim a unique staging directory after validating the domain root and cleaning stale work.
    pub(crate) fn create_staging(&self) -> anyhow::Result<PathBuf> {
        ensure_safe_backup_path(self.root)?;
        std::fs::create_dir_all(self.root)?;
        self.cleanup_stale_staging();
        for _ in 0..100 {
            let candidate = self.root.join(format!(
                "{STAGING_PREFIX}{}-{}",
                std::process::id(),
                STAGING_SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&candidate) {
                Ok(()) => return Ok(candidate),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("could not claim a unique backup staging directory")
    }

    /// Remove one staging directory created by the caller after a failed assembly or publication.
    pub(crate) fn discard_staging(&self, staging: &Path) {
        if staging.parent() == Some(self.root)
            && staging
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(is_staging_name)
        {
            let _ = std::fs::remove_dir_all(staging);
        }
    }

    /// Publish a snapshot under the first free `base`, `base-01`, ... name.
    ///
    /// This is the config/manual policy: every operation remains distinct even when another
    /// snapshot was taken in the same second.
    pub(crate) fn publish_distinct(&self, staging: &Path, base: &str) -> anyhow::Result<PathBuf> {
        let mut last_error: Option<std::io::Error> = None;
        for attempt in 0..=99u32 {
            let name = if attempt == 0 {
                base.to_owned()
            } else {
                format!("{base}-{attempt:02}")
            };
            let destination = self.root.join(name);
            if std::fs::symlink_metadata(&destination).is_ok() {
                continue;
            }
            match std::fs::rename(staging, &destination) {
                Ok(()) => return Ok(destination),
                Err(error) => last_error = Some(error),
            }
        }
        self.discard_staging(staging);
        match last_error {
            Some(error) => Err(anyhow::Error::from(error)
                .context(format!("failed to publish distinct backup {base}"))),
            None => anyhow::bail!("all backup names for {base} are occupied"),
        }
    }

    /// Publish one canonical slot name or accept a concurrently published domain-valid winner.
    ///
    /// Args:
    ///     staging: Fully assembled unpublished directory.
    ///     name: Exact canonical slot name; no collision suffix is permitted.
    ///     completed: Domain validator for an existing winner, such as SQLite header + marker.
    pub(crate) fn publish_exact(
        &self,
        staging: &Path,
        name: &str,
        completed: impl Fn(&Path) -> bool,
    ) -> anyhow::Result<ExactPublication> {
        let destination = self.root.join(name);
        if std::fs::symlink_metadata(&destination).is_ok() {
            if completed(&destination) {
                self.discard_staging(staging);
                return Ok(ExactPublication::Existing(destination));
            }
            self.discard_staging(staging);
            anyhow::bail!(
                "exact backup path is occupied by an incomplete or foreign entry: {}",
                destination.display()
            );
        }
        match std::fs::rename(staging, &destination) {
            Ok(()) => Ok(ExactPublication::Created(destination)),
            Err(_error) if completed(&destination) => {
                self.discard_staging(staging);
                Ok(ExactPublication::Existing(destination))
            }
            Err(error) => {
                self.discard_staging(staging);
                Err(anyhow::Error::from(error)
                    .context(format!("failed to publish exact backup {name}")))
            }
        }
    }

    /// Remove every complete owned snapshot selected by the domain's retention policy.
    pub(crate) fn prune_where(
        &self,
        owns_name: impl Fn(&str) -> bool,
        should_remove: impl Fn(&str) -> bool,
        completed: impl Fn(&Path) -> bool,
    ) -> usize {
        self.owned_names(owns_name)
            .into_iter()
            .filter(|name| should_remove(name))
            .filter(|name| completed(&self.root.join(name)))
            .filter(|name| self.remove_owned_snapshot(name))
            .count()
    }

    /// Enumerate directory names accepted by the domain's complete grammar without following links.
    fn owned_names(&self, owns_name: impl Fn(&str) -> bool) -> Vec<String> {
        if ensure_safe_backup_path(self.root).is_err() {
            return Vec::new();
        }
        let Ok(entries) = std::fs::read_dir(self.root) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let kind = entry.file_type().ok()?;
                if !kind.is_dir() {
                    return None;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                owns_name(&name).then_some(name)
            })
            .collect()
    }

    /// Delete a snapshot only when its complete entry set belongs to this domain.
    fn remove_owned_snapshot(&self, name: &str) -> bool {
        let directory = self.root.join(name);
        let Ok(mut entries) = std::fs::read_dir(&directory) else {
            return false;
        };
        let mut found = Vec::<OsString>::new();
        while let Some(entry) = entries.next() {
            let Ok(entry) = entry else {
                return false;
            };
            let Ok(kind) = entry.file_type() else {
                return false;
            };
            let name = entry.file_name();
            if !kind.is_file()
                || !self
                    .expected
                    .iter()
                    .any(|expected| name == OsStr::new(expected))
            {
                return false;
            }
            found.push(name);
        }
        drop(entries);
        if found.is_empty()
            || (self.require_all
                && self
                    .expected
                    .iter()
                    .any(|expected| !found.iter().any(|name| name == OsStr::new(expected))))
        {
            return false;
        }

        // Move every owned file to a private quarantine before removing the public directory.
        // If another writer adds content or any move fails, rolling the files back preserves the
        // complete snapshot instead of leaving it half-deleted.
        let Ok(quarantine) = self.create_staging() else {
            return false;
        };
        let mut moved = Vec::<OsString>::new();
        for file_name in &found {
            if std::fs::rename(directory.join(file_name), quarantine.join(file_name)).is_err() {
                restore_files(&quarantine, &directory, &moved);
                self.discard_staging(&quarantine);
                return false;
            }
            moved.push(file_name.clone());
        }
        if std::fs::remove_dir(&directory).is_err() {
            restore_files(&quarantine, &directory, &moved);
            self.discard_staging(&quarantine);
            return false;
        }
        self.discard_staging(&quarantine);
        true
    }

    /// Remove staging directories only after a full day without directory or payload activity.
    fn cleanup_stale_staging(&self) {
        let Ok(entries) = std::fs::read_dir(self.root) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if !kind.is_dir() || !name.to_str().is_some_and(is_staging_name) {
                continue;
            }
            if staging_is_stale(&entry.path(), now) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// Restore quarantined files after a snapshot removal could not complete.
fn restore_files(quarantine: &Path, directory: &Path, file_names: &[OsString]) {
    for file_name in file_names.iter().rev() {
        let _ = std::fs::rename(quarantine.join(file_name), directory.join(file_name));
    }
}

/// Return whether a directory name is exactly `.incoming-<pid>-<sequence>`.
fn is_staging_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(STAGING_PREFIX) else {
        return false;
    };
    let Some((pid, sequence)) = rest.split_once('-') else {
        return false;
    };
    !pid.is_empty()
        && !sequence.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
}

/// Return the fixed 15-byte UTC timestamp prefix when its shape is exactly `YYYYMMDD-HHMMSS`.
pub(crate) fn timestamp_prefix(name: &str) -> Option<&str> {
    let stamp = name.get(..15)?;
    let bytes = stamp.as_bytes();
    (bytes.get(8) == Some(&b'-')
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[9..].iter().all(u8::is_ascii_digit))
    .then_some(stamp)
}

/// Reject a domain backup directory or its shared parent when either is a link or non-directory.
fn ensure_safe_backup_path(root: &Path) -> anyhow::Result<()> {
    for path in [root.parent(), Some(root)].into_iter().flatten() {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("backup path is a symlink: {}", path.display())
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => anyhow::bail!("backup path is not a directory: {}", path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Return whether a staging directory had no directory or payload activity for 24 hours.
fn staging_is_stale(staging: &Path, now: SystemTime) -> bool {
    let latest = std::iter::once(staging.to_path_buf())
        .chain(
            std::fs::read_dir(staging)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path()),
        )
        .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
        .max();
    latest
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= STAGING_STALE_AFTER)
}

#[cfg(test)]
mod tests;
