//! The station's binary from the MoonTerminal release, for a server that has none yet: the first
//! install (STATION.md §1 п. 21, §6). Every later update is the server's own — its installed
//! binary finds the release (`moon-station-admin update-from-release`); a server without one
//! cannot, so the terminal fetches it here, by the same discovery and digest checks as its own
//! updater, and sends it through the helper's `update`.

use std::path::{Path, PathBuf};

use anyhow::Context;
use moon_core::update::{
    BuildIdentity, GitHubReleaseClient, ReleaseDiscovery, UpdateEligibility,
    station_asset_name_for_arch,
};

/// Download the newest release's station binary for a server of `arch` (`uname -m`) into `dir`.
///
/// Args:
///     arch: The server's architecture, as the setup's probe reported it.
///     dir: A directory of this machine whose parent exists; created when missing. The binary
///         lands in it under its asset name.
///     say: One line on what is fetched.
///
/// Returns:
///     The downloaded file, checked against the release's SHA-256 digest.
pub fn fetch_station(arch: &str, dir: &Path, say: &mut dyn FnMut(&str)) -> anyhow::Result<PathBuf> {
    let asset = station_asset_name_for_arch(arch)
        .with_context(|| format!("no station binary is released for a {arch} server"))?;
    // The newest release that carries it, whichever it is: a server with no station compares
    // against no version of its own.
    let mut discovery =
        ReleaseDiscovery::for_asset(BuildIdentity::from_release_base("v0.0.0"), asset);
    let scan = discovery
        .scan()
        .map_err(|e| anyhow::anyhow!("GitHub releases: {e}"))?;
    let UpdateEligibility::Available(release) = scan.eligibility else {
        anyhow::bail!("no MoonTerminal release carries {asset} with a published digest yet");
    };
    say(&format!(
        "station: {asset} from release {} ({} MB)",
        release.release_tag(),
        release.asset_size() / 1_000_000
    ));
    GitHubReleaseClient::new()
        .download_verified_to(&release, &dir.join(asset))
        .with_context(|| format!("download {asset} of {}", release.release_tag()))
}
