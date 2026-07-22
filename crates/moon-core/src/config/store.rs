//! Low-level config-file I/O: encrypted servers.enc and plaintext settings.toml.
//! This module contains NO domain logic; merging and uids belong in `reconcile`.
//!
//! A corrupt settings.toml must not be lost silently: move it to `.bak` and continue with
//! defaults. Otherwise one invalid value could erase all groups and server checkboxes.

use std::path::Path;

use anyhow::Context;

use super::crypto;
use super::paths;
use super::schema::{ServersFile, SettingsFile};
use super::toml_io::ConfigLoad;

/// Decrypts and parses servers.enc. Read/decryption failures are fatal because these are
/// user secrets and must not be discarded silently.
pub fn read_servers() -> anyhow::Result<ServersFile> {
    let bytes = std::fs::read(paths::servers_path()).context("чтение servers.enc")?;
    let plain = crypto::decrypt(&bytes)?;
    let sf = toml::from_str(std::str::from_utf8(&plain)?).context("разбор servers.enc")?;
    Ok(sf)
}

/// Encrypts and writes servers.enc.
pub fn write_servers(sf: &ServersFile) -> anyhow::Result<()> {
    let enc = crypto::encrypt(toml::to_string(sf)?.as_bytes())?;
    super::toml_io::write_atomic(&paths::servers_path(), &enc, "servers.enc")
        .context("запись servers.enc")?;
    Ok(())
}

/// Read settings.toml together with its read STATUS.
///
/// A missing file returns defaults for first launch. A corrupt file is moved to `.bak`, logged,
/// and replaced with defaults so data is not lost silently. An unreadable file returns defaults
/// plus [`ConfigLoad::Unreadable`].
///
/// Status is required here because `AppConfig::load` automatically saves an outdated schema.
/// Without this distinction, one temporary read failure could overwrite the live config with
/// defaults.
pub fn read_settings() -> (SettingsFile, ConfigLoad) {
    super::toml_io::load_or_default_status(&paths::settings_path(), "settings.toml", backup_corrupt)
}

/// Writes settings.toml as plaintext, human-readable TOML without secrets.
pub fn write_settings(sf: &SettingsFile) -> anyhow::Result<()> {
    super::toml_io::save(&paths::settings_path(), sf, "settings.toml")
}

/// Rename corrupt settings.toml to settings.toml.bak instead of discarding it silently.
///
/// Returns `false` when quarantine FAILED. The distinction is critical: successful quarantine
/// means the original bytes survive in `.bak`, so defaults can safely be written to the now-absent
/// path. Failure means the user's file remains in place and must not be overwritten.
fn backup_corrupt(path: &Path) -> bool {
    let bak = path.with_extension("toml.bak");
    match std::fs::rename(path, &bak) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("не удалось увести битый settings.toml в .bak: {e}");
            false
        }
    }
}
