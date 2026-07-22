//! Shared load/save logic for plaintext TOML config files: one pattern of
//! "missing → defaults; corrupt → log + on_corrupt + defaults" instead of duplicates.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Config-load outcome used by the caller to decide whether writing back is safe.
///
/// The four outcomes are NOT interchangeable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigLoad {
    /// The file was read and parsed; in-memory state reflects its contents.
    Present,
    /// The file is absent on first launch; defaults can be saved safely.
    Absent,
    /// The file exists but does not parse, and `on_corrupt` moved it to `.bak` successfully.
    /// The original bytes are preserved, so defaults can fill the now-absent path.
    ///
    /// Failed quarantine yields [`ConfigLoad::Unreadable`]: the user's file remains in place and
    /// permitting a write would destroy it.
    Corrupt,
    /// The file may exist and contain valid data but be unreadable because of permissions, a
    /// sharing violation, or an unhydrated cloud placeholder.
    ///
    /// Returned defaults may be used in memory but must not be saved: a temporary read failure must
    /// not overwrite a valid config.
    Unreadable,
}

impl ConfigLoad {
    /// Return whether this file may be overwritten.
    ///
    /// Deliberately uses an EXHAUSTIVE `match` rather than comparing with one "bad" variant. A
    /// check such as `== Unreadable` is open by default: a fifth variant added later for an
    /// interrupted read, decryption failure, or cloud-placeholder hydration timeout would silently
    /// become writable, precisely the failure this design prevents. A new variant cannot compile
    /// here until its author assigns it to one side.
    pub fn permits_overwrite(self) -> bool {
        match self {
            // The file was read, is absent, or was moved to `.bak`; original bytes are either
            // absent from the destination or preserved.
            ConfigLoad::Present | ConfigLoad::Absent | ConfigLoad::Corrupt => true,
            // The user's file remains in place and was not read, so writing is forbidden.
            ConfigLoad::Unreadable => false,
        }
    }
}

/// Read TOML into `T` and report HOW it was read.
///
/// A missing file yields defaults for first launch. A corrupt file is logged, passed to
/// `on_corrupt(path)` for quarantine such as a move to `.bak`, and replaced with defaults. An
/// unreadable file yields defaults plus [`ConfigLoad::Unreadable`] so the caller cannot overwrite
/// the live config; see [`ConfigLoad`].
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
        // This is NOT file absence: the distinction prevents the caller from turning a temporary
        // read failure into a permanent overwrite during write-back.
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
            // Failed quarantine does NOT permit writing: the user's file remains in place and
            // saving defaults would destroy it.
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

/// Like [`load_or_default_status`] without the status, for layout/theme files whose unreadable
/// state does not trigger an automatic overwrite.
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

/// Build defaults for an absent or unreadable file by deserializing EMPTY TOML rather than calling
/// `T::default()`.
///
/// These paths are not interchangeable. A field with `#[serde(default = "...")]` receives that
/// value only during DESERIALIZATION, while derived `Default` returns `0`, `false`, or `""`.
/// Calling `T::default()` for an absent file would, for example, produce `ui_scale = 0.0` instead
/// of `1.0` and collapse every UI hit target to zero size.
///
/// Parsing an empty document follows the same defaulting path as an incomplete existing file.
/// `T::default()` remains a fallback for a type that cannot be built from empty TOML.
fn defaults_for_absent_file<T: Default + DeserializeOwned>(label: &str) -> T {
    toml::from_str("").unwrap_or_else(|e| {
        log::warn!("{label}: пустой документ не разбирается ({e}); беру Default");
        T::default()
    })
}

/// Writes a value as human-readable TOML.
pub fn save<T: Serialize>(path: &Path, value: &T, label: &str) -> anyhow::Result<()> {
    write_atomic(path, toml::to_string_pretty(value)?.as_bytes(), label)?;
    Ok(())
}

/// Writes a file through a temporary sibling plus rename. A direct `fs::write` can leave a
/// truncated config if the process or power fails during the write.
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
/// Defaulting contract for config files absent from disk.
mod tests;
