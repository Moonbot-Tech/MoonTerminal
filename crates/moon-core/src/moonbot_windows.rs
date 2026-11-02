//! Moonbot for Windows, fetched for a user who wants the bot with its own windows rather than a
//! core on a server: download the release archive, unpack it into a folder of the user's choice
//! and find `MoonBot.exe` to start. The connect wizard does the rest (starts it, closes the
//! terminal).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;

/// Moonbot's Windows release archive.
pub const RELEASE_URL: &str = "https://api.moonbot.pro/files/MoonBot.zip";
/// The folder made inside a chosen one that is not empty.
const SUBFOLDER: &str = "Moonbot";
/// The archive is about 25 MB; anything far past that is not it.
const MAX_BYTES: u64 = 300 * 1024 * 1024;
/// What the archive may unpack to, all entries together: MoonBot is well under 200 MB, and an
/// archive that inflates past this is not MoonBot.
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
/// The whole download, on a slow line.
const TIMEOUT: Duration = Duration::from_secs(600);
/// The program to start, at the archive's root or one folder down.
const EXE: &str = "MoonBot.exe";

/// Where Moonbot goes for the folder the user chose: the folder itself when it is new or empty,
/// else a `Moonbot` folder inside it, which must be new or empty too. Never over files.
pub fn target_dir(chosen: &Path) -> anyhow::Result<PathBuf> {
    if is_free(chosen)? {
        return Ok(chosen.to_path_buf());
    }
    let inside = chosen.join(SUBFOLDER);
    anyhow::ensure!(
        is_free(&inside)?,
        "{} and {} already hold files: choose an empty folder",
        chosen.display(),
        inside.display()
    );
    Ok(inside)
}

/// Download the archive at `url`, unpack it into `dir` and return the path of `MoonBot.exe`.
///
/// `dir` is the free folder [`target_dir`] picked: a failed unpack takes it out again, so the next
/// try lands in the same place rather than beside a half-unpacked copy.
pub fn install(url: &str, dir: &Path) -> anyhow::Result<PathBuf> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .https_only(true)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let bytes = agent
        .get(url)
        .call()
        .with_context(|| format!("download {url}"))?
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .with_context(|| format!("read {url}"))?;
    let unpacked = unpack(&bytes, dir)
        .and_then(|()| find_exe(dir).ok_or_else(|| anyhow::anyhow!("the archive has no {EXE}")));
    if unpacked.is_err() {
        let _ = std::fs::remove_dir_all(dir);
    }
    unpacked
}

/// Unpack `zip` into `dir`. An entry whose path would leave `dir` is refused, the whole archive
/// with it, and so is an archive that inflates past [`MAX_UNPACKED`].
pub(crate) fn unpack(zip: &[u8], dir: &Path) -> anyhow::Result<()> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip)).context("open the archive")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let mut left = MAX_UNPACKED;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name()?.into_owned();
        let relative = entry.enclosed_name().ok_or_else(|| {
            anyhow::anyhow!("the archive entry {name:?} points outside its folder")
        })?;
        let path = dir.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Read one byte past what is left: a declared size is the archive's word, the bytes are not.
        let mut data = Vec::new();
        (&mut entry).take(left + 1).read_to_end(&mut data)?;
        left = left.checked_sub(data.len() as u64).ok_or_else(|| {
            anyhow::anyhow!("the archive unpacks to more than {MAX_UNPACKED} bytes")
        })?;
        std::fs::write(&path, data).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(())
}

/// `MoonBot.exe` at `dir`'s root or one folder down — archives differ in that.
pub(crate) fn find_exe(dir: &Path) -> Option<PathBuf> {
    let is_exe = |path: &Path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(EXE))
    };
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    if let Some(exe) = entries.iter().find(|p| p.is_file() && is_exe(p)) {
        return Some(exe.clone());
    }
    entries.iter().filter(|p| p.is_dir()).find_map(|sub| {
        std::fs::read_dir(sub)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.is_file() && is_exe(p))
    })
}

/// Whether `dir` is missing or empty.
fn is_free(dir: &Path) -> anyhow::Result<bool> {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(anyhow::Error::new(e).context(format!("read {}", dir.display()))),
    }
}

#[cfg(test)]
mod tests;
