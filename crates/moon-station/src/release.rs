//! The station's version and its updates from the terminal's GitHub releases (STATION.md §1 п. 20,
//! §4.6).
//!
//! The version is the terminal's release tag the station was built from (`build.rs`, the
//! terminal's own rules). A release that carries `moon-station-<arch>` newer than it is an update:
//! the bot's "Status" offers it, and "Update" files a request — a file without parameters in the
//! data root. The station runs unprivileged and does not install anything itself: a systemd path
//! unit (`moon-station-update.path`) sees the file and starts the helper as root, and the helper
//! runs THIS binary as `release-fetch` to find the release and download it, checked against the
//! immutable release's SHA-256 digest by the terminal's own updater code (`moon_core::update`).
//! The request names no version, so a station taken over cannot pick what root installs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use moon_core::update::{
    BuildIdentity, GitHubReleaseClient, ReleaseDiscovery, UpdateEligibility, station_asset_name,
};
use moon_tg::{ReleaseCheck, ReleaseFailure, UpdateRefusal};

/// The update request's name in the data root; the path unit watches for it.
pub const UPDATE_REQUEST: &str = "update.request";
/// The last update's verdict with its time, written by the helper into the data root.
const UPDATE_RESULT: &str = "update.result";
/// The path unit that turns the request into an update, as enabled: the link `systemctl enable`
/// makes. A helper from a terminal that installs it puts it there.
const UPDATE_PATH_ENABLED: &str = "/etc/systemd/system/paths.target.wants/moon-station-update.path";
/// A request older than this was never taken: the path unit that takes it at once has stopped or
/// failed — the helper's `update-from-release`, run from the terminal, brings it back.
const STALE_REQUEST: Duration = Duration::from_secs(15 * 60);
/// How long the chat's "Status" waits for GitHub before it answers without the release.
const CHECK_WAIT: Duration = Duration::from_secs(10);

/// The release tag the station was built from, `unknown` for a build outside a tagged history.
fn release_base() -> &'static str {
    option_env!("MOONTERMINAL_RELEASE_BASE").unwrap_or("unknown")
}

/// The station's version as it reports it: the release tag and the exact revision.
pub fn version() -> String {
    let revision = option_env!("MOONTERMINAL_GIT_REV").unwrap_or("unknown");
    match BuildIdentity::from_release_base(release_base()).baseline() {
        Some(version) => format!("{version} ({revision})"),
        None => format!("dev ({revision})"),
    }
}

/// The station's look at the latest release, for the chat's "Status": one discovery session kept
/// for the process, so an unchanged release list costs GitHub a conditional request only.
#[derive(Clone, Default)]
pub struct ReleaseWatch {
    discovery: Arc<Mutex<Option<ReleaseDiscovery>>>,
}

impl ReleaseWatch {
    /// [`Self::check`], waited for at most [`CHECK_WAIT`]: a GitHub that does not answer costs the
    /// chat a line in its status, not its whole answer. The look itself runs on, and its session
    /// keeps what it learns for the next one.
    pub fn check_within(&self) -> ReleaseCheck {
        let (tx, rx) = mpsc::channel();
        let watch = self.clone();
        let started = std::thread::Builder::new()
            .name("release-check".into())
            .spawn(move || {
                let _ = tx.send(watch.check());
            });
        if let Err(error) = started {
            return ReleaseCheck::Failed(ReleaseFailure::Unavailable(format!(
                "no thread for the check: {error}"
            )));
        }
        rx.recv_timeout(CHECK_WAIT)
            .unwrap_or(ReleaseCheck::Failed(ReleaseFailure::Timeout))
    }

    /// Whether a newer release carries this station's binary. Blocking — GitHub is asked — so it
    /// runs off the main loop.
    pub fn check(&self) -> ReleaseCheck {
        let Some(asset) = station_asset_name() else {
            return ReleaseCheck::Failed(ReleaseFailure::UnsupportedArchitecture(
                std::env::consts::ARCH.into(),
            ));
        };
        let identity = BuildIdentity::from_release_base(release_base());
        if identity.baseline().is_none() {
            return ReleaseCheck::Unversioned;
        }
        let Ok(mut discovery) = self.discovery.lock() else {
            return ReleaseCheck::Failed(ReleaseFailure::Unavailable(
                "the release check failed earlier".into(),
            ));
        };
        let discovery =
            discovery.get_or_insert_with(|| ReleaseDiscovery::for_asset(identity, asset));
        match discovery.scan() {
            Ok(result) => match result.eligibility {
                UpdateEligibility::Available(release) => {
                    ReleaseCheck::Newer(release.version().to_string())
                }
                UpdateEligibility::Current => ReleaseCheck::Current,
                UpdateEligibility::Unsupported => ReleaseCheck::Unversioned,
            },
            Err(error) => ReleaseCheck::Failed(ReleaseFailure::Unavailable(error.to_string())),
        }
    }
}

/// File the request for the root updater: the latest release, found and checked by it.
///
/// Args:
///     data_root: The station's data root, where the path unit watches.
///
/// Returns:
///     Why the request was not filed: the server has no updater enabled (set up by an older
///     terminal), one is already filed, or the file could not be written.
pub fn request_update(data_root: &Path) -> Result<(), UpdateRefusal> {
    if !Path::new(UPDATE_PATH_ENABLED).exists() {
        return Err(UpdateRefusal::UpdaterMissing);
    }
    request_update_file(data_root)
}

/// File a request without replacing one already waiting for the updater.
/// Returns a typed refusal; the file operations and stale deadline are unchanged.
fn request_update_file(data_root: &Path) -> Result<(), UpdateRefusal> {
    let request = data_root.join(UPDATE_REQUEST);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&request)
    {
        Ok(_) => {
            log::info!("update: requested from the bot's chat");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let age = std::fs::metadata(&request)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok());
            // A watcher that runs takes the file at once: one this old was never taken.
            match age.is_some_and(|age| age >= STALE_REQUEST) {
                true => Err(UpdateRefusal::RequestStale),
                false => Err(UpdateRefusal::AlreadyRunning),
            }
        }
        Err(e) => Err(UpdateRefusal::WriteFailed(format!(
            "{}: {e}",
            request.display()
        ))),
    }
}

/// The last update's verdict as the helper left it: `<UTC time> <its last line>`; `None` before
/// the first one, or where it cannot be read.
pub fn last_update(data_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(data_root.join(UPDATE_RESULT)).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.chars().take(300).collect())
}

/// `moon-station release-fetch --out <path>`: what the root updater runs. Prints one line —
/// `release=<tag> sha256=<hex>` with the binary verified at `<path>`, or `release=current` /
/// `release=unversioned` with nothing downloaded; fails with why otherwise.
pub fn fetch(mut args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    const USAGE: &str = "usage: moon-station release-fetch --out <path>";
    let out = match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--out"), Some(path), None) => PathBuf::from(path),
        _ => anyhow::bail!(USAGE),
    };
    let asset = station_asset_name().with_context(|| {
        format!(
            "no station binary is released for {}",
            std::env::consts::ARCH
        )
    })?;
    let identity = BuildIdentity::from_release_base(release_base());
    if identity.baseline().is_none() {
        println!("release=unversioned");
        return Ok(());
    }
    let scan = ReleaseDiscovery::for_asset(identity, asset)
        .scan()
        .map_err(|e| anyhow::anyhow!("GitHub releases: {e}"))?;
    match scan.eligibility {
        UpdateEligibility::Available(release) => {
            GitHubReleaseClient::new()
                .download_verified_to(&release, &out)
                .with_context(|| format!("download {} {}", release.release_tag(), asset))?;
            let sha256: String = release
                .asset_sha256()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            println!("release={} sha256={sha256}", release.release_tag());
        }
        UpdateEligibility::Current => println!("release=current"),
        UpdateEligibility::Unsupported => println!("release=unversioned"),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
