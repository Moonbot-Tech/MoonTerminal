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
///
/// A failure to DECRYPT is reported as the typed [`crypto::AccessError`] so a caller can tell
/// "ask the user for the encryption password" apart from "there is no way into this file". It also
/// seals the vault, so no later save can overwrite a file whose key slots are still needed.
pub fn read_servers() -> anyhow::Result<ServersFile> {
    let bytes = std::fs::read(paths::servers_path()).context("чтение servers.enc")?;
    let plain = crypto::decrypt_config(&bytes)?;
    let sf = toml::from_str(std::str::from_utf8(&plain)?).context("разбор servers.enc")?;
    Ok(sf)
}

/// Encrypts and writes servers.enc.
///
/// Refuses to write over a file this process never opened; see [`refuse_blind_overwrite`].
pub fn write_servers(sf: &ServersFile) -> anyhow::Result<()> {
    ensure_servers_writable()?;
    let enc = crypto::encrypt_config(toml::to_string(sf)?.as_bytes())?;
    super::toml_io::write_atomic(&paths::servers_path(), &enc, "servers.enc")
        .context("запись servers.enc")?;
    // Only now are the key slots actually on disk. Marking them persisted before the write would
    // hide a failed one: the flag is the only thing that asks for the retry.
    crypto::mark_persisted();
    Ok(())
}

/// Check whether `servers.enc` may be written at all, before any file is touched.
///
/// Called by `AppConfig::save` BEFORE the config-pair write begins. The guard must not fire from
/// inside that sequence: by then the pending marker exists and `settings.toml` has not been
/// written, so refusing there would leave the pair permanently marked as half-replaced.
pub(super) fn ensure_servers_writable() -> anyhow::Result<()> {
    refuse_blind_overwrite(
        paths::servers_path().exists(),
        crypto::access_state().opened,
    )
}

/// Refuse to replace an existing `servers.enc` that this process never opened.
///
/// `crypto` seals itself after a FAILED decryption, but it cannot see the case where decryption
/// was never attempted at all: `MOON_CONFIG_PLAINTEXT` returns from `AppConfig::load` before the
/// file is read, and a Settings save in that mode would then encrypt the synthetic config under
/// brand-new key material straight over the user's real cores — and over the password slot that
/// was the way back into them.
///
/// The check lives here rather than in `crypto` because the path is what makes it decidable, and
/// `crypto` deliberately knows nothing about paths.
fn refuse_blind_overwrite(file_exists: bool, opened: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        !file_exists || opened,
        "запись servers.enc запрещена: файл существует, но не был прочитан в этом запуске — \
         перезапись уничтожила бы ключи ядер"
    );
    Ok(())
}

#[cfg(test)]
mod tests;

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
