//! Collision-free, bounded-lifetime paths for downloaded daily archives.

use super::{ArchiveError, ArchiveFormat, ArchiveRoute};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Final and unique temporary paths for one daily archive.
pub(super) struct Paths {
    pub(super) file: PathBuf,
    part: PathBuf,
}

/// Key paths by route rather than host so spot and futures archives cannot collide.
pub(super) fn paths(
    root: &Path,
    route: ArchiveRoute,
    market: &str,
    day: chrono::NaiveDate,
) -> Result<Paths, ArchiveError> {
    if market.is_empty()
        || !market
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ArchiveError::UnknownSymbol);
    }
    let route_name = match route {
        ArchiveRoute::BinanceUmDaily => "binance_um_daily",
        ArchiveRoute::BinanceCmDaily => "binance_cm_daily",
        ArchiveRoute::BinanceSpotDaily => "binance_spot_daily",
        ArchiveRoute::BybitTradingDaily => "bybit_trading_daily",
    };
    let extension = match route.format() {
        ArchiveFormat::ZipCsv => "zip",
        ArchiveFormat::GzCsv => "csv.gz",
    };
    let file = root
        .join(route_name)
        .join(format!("{market}-{day}.{extension}"));
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let part = file.with_file_name(format!(
        "{market}-{day}.{extension}.{}-{}.part",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(Paths { file, part })
}

type DayLocks = Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>;
static KEYS: OnceLock<DayLocks> = OnceLock::new();

/// One same-day lock user; the map entry leaves with the last user instead of the archive file.
pub(super) struct InFlight {
    path: PathBuf,
    key: Arc<Mutex<()>>,
}

impl InFlight {
    /// Serialize this caller with other current users of the same archive day.
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.key.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl Drop for InFlight {
    /// Remove only the final reference: current and waiting users keep their shared lock alive.
    fn drop(&mut self) {
        let mut keys = KEYS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // A new user cannot clone between this count and removal while the map lock is held.
        if Arc::strong_count(&self.key) == 2 {
            keys.remove(&self.path);
        }
    }
}

/// Serialize callers for the same day even after its disposable file has been extracted.
pub(super) fn in_flight(path: &Path) -> InFlight {
    let key = KEYS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(path.to_owned())
        .or_default()
        .clone();
    InFlight {
        path: path.to_owned(),
        key,
    }
}

#[cfg(test)]
/// Whether this archive day still has a process-global lock entry.
pub(super) fn contains_key(path: &Path) -> bool {
    KEYS.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(path)
}

/// Remove disposable files on every success and failure exit.
pub(super) struct Cleanup(Paths);
impl Cleanup {
    /// Own both paths before the first operation can fail.
    pub(super) fn new(paths: Paths) -> Self {
        Self(paths)
    }
    /// Return the final compressed path.
    pub(super) fn file(&self) -> &Path {
        &self.0.file
    }
    /// Return this caller's unique partial path.
    pub(super) fn part(&self) -> &Path {
        &self.0.part
    }
}
impl Drop for Cleanup {
    /// Best-effort cleanup; startup sweeping recovers any interrupted or failed deletion.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0.part);
        let _ = std::fs::remove_file(&self.0.file);
    }
}

/// Sweep leftovers once per root before any new downloads for that root begin.
pub(super) fn sweep_once(root: &Path) -> std::io::Result<()> {
    static ROOTS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let mut roots = ROOTS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if roots.contains(root) {
        return Ok(());
    }
    sweep(root)?;
    roots.insert(root.to_owned());
    Ok(())
}

/// Delete partials and archives older than one hour without following directory symlinks.
fn sweep(root: &Path) -> std::io::Result<()> {
    sweep_with_remove(root, &|path| std::fs::remove_file(path))
}

/// Sweep through the real directory walk while treating individual deletion failures as leftovers.
pub(super) fn sweep_with_remove(
    root: &Path,
    remove: &impl Fn(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            sweep_with_remove(&entry.path(), remove)?;
        } else if kind.is_file() {
            let path = entry.path();
            let partial = path
                .extension()
                .is_some_and(|extension| extension == "part");
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age >= Duration::from_secs(3600));
            if partial || stale {
                let _ = remove(&path);
            }
        }
    }
    Ok(())
}
