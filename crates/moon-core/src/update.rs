//! Stable GitHub release discovery and verified update downloads.
//!
//! This module deliberately stops at a verified download: the staged Windows or Linux executable,
//! the macOS disk image, or the station's Linux binary. Process coordination, replacement,
//! restart, and rollback belong to the GPUI shell on Windows and Linux; on macOS the shell only
//! opens the image; the station's binary is installed by its server helper
//! (`moon-station-admin update-from-release`). The one exception is [`swap`]: the Linux file and
//! process steps of the shell's transaction live here, where the Linux CI job can test them.

use std::path::PathBuf;
use std::time::Duration;

mod discovery;
mod download;
mod release;
#[cfg(target_os = "linux")]
pub mod swap;

pub use discovery::{DiscoveryError, DiscoveryResult, DiscoveryRetry, ReleaseDiscovery};
pub use release::{
    AvailableRelease, BuildIdentity, ReleaseAsset, ReleaseVersion, UpdateEligibility,
    station_asset_name, station_asset_name_for_arch,
};

#[cfg(test)]
mod tests;

/// Exact release asset name of the Linux terminal, for the release pipeline's contract test.
///
/// Returns:
///     The name a Linux x86_64 build's updater looks for in a release.
pub fn linux_terminal_asset_name() -> &'static str {
    release::LINUX_X86_64_ASSET_NAME
}

/// Whether releases carry a terminal for the target this binary was compiled for.
///
/// `false` means no release can ever be offered here — Linux on an architecture other than
/// x86_64, or another OS altogether — so the caller has no reason to start discovery.
pub fn terminal_asset_exists_for_this_target() -> bool {
    release::platform_asset_name().is_some()
}

const RELEASES_URL: &str = "https://api.github.com/repos/Moonbot-Tech/MoonTerminal/releases";

/// Synchronous GitHub client intended for a dedicated background executor.
#[derive(Clone)]
pub struct GitHubReleaseClient {
    /// Release-list pages: small bodies under one whole-call ceiling.
    agent: ureq::Agent,
    /// Release assets: no whole-call ceiling, only a bound on each silent wait.
    download_agent: ureq::Agent,
    releases_url: String,
}

/// Whole-call ceiling for one release-list page.
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);

/// Longest wait for each download step before the body: name lookup, connect, request write.
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

impl GitHubReleaseClient {
    /// Build a bounded HTTPS-only client for public release metadata and assets.
    ///
    /// Returns:
    ///     Client with a bounded redirect chain, a whole-call timeout on release metadata and a
    ///     per-read timeout on asset downloads.
    pub fn new() -> Self {
        Self::build(RELEASES_URL, true)
    }

    /// Build one transport with an explicit release-list endpoint and HTTPS policy.
    ///
    /// Args:
    ///     releases_url: Exact release-list endpoint.
    ///     https_only: Whether the transport rejects all plain-HTTP requests.
    ///
    /// Returns:
    ///     A bounded synchronous GitHub client.
    fn build(releases_url: &str, https_only: bool) -> Self {
        Self::build_with(releases_url, https_only, METADATA_TIMEOUT)
    }

    /// Build the two transports with an explicit release-list ceiling.
    ///
    /// A release asset runs past 100 MB, so a whole-call ceiling would fail every link slower
    /// than size / ceiling. Downloads instead bound each step before the body and each wait for
    /// fresh bytes, the same per-read deadline the trade archive uses for its long daily bodies.
    ///
    /// Args:
    ///     releases_url: Exact release-list endpoint.
    ///     https_only: Whether both transports reject all plain-HTTP requests.
    ///     metadata_timeout: Whole-call ceiling for one release-list page.
    ///
    /// Returns:
    ///     A bounded synchronous GitHub client.
    fn build_with(releases_url: &str, https_only: bool, metadata_timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(metadata_timeout))
            .https_only(https_only)
            .max_redirects(5)
            .http_status_as_error(false)
            .build();
        let download_config = ureq::Agent::config_builder()
            .timeout_resolve(Some(DOWNLOAD_CONNECT_TIMEOUT))
            .timeout_connect(Some(DOWNLOAD_CONNECT_TIMEOUT))
            .timeout_send_request(Some(DOWNLOAD_CONNECT_TIMEOUT))
            .https_only(https_only)
            .max_redirects(5)
            .http_status_as_error(false)
            .build();
        let download_agent = {
            use ureq::unversioned::transport::{Connector, DefaultConnector};
            ureq::Agent::with_parts(
                download_config,
                DefaultConnector::default()
                    .chain(crate::market::trade_replay::archive::transport::ReadTimeout),
                ureq::unversioned::resolver::DefaultResolver::default(),
            )
        };
        Self {
            agent: ureq::Agent::new_with_config(config),
            download_agent,
            releases_url: releases_url.to_owned(),
        }
    }

    /// Build a loopback-capable client that is absent from production artifacts.
    ///
    /// Args:
    ///     releases_url: Loopback fixture endpoint.
    ///
    /// Returns:
    ///     A test-only client that permits plain HTTP.
    #[cfg(test)]
    fn for_test(releases_url: &str) -> Self {
        Self::build(releases_url, false)
    }

    /// Build a loopback-capable client with a shortened release-list ceiling.
    ///
    /// Args:
    ///     releases_url: Loopback fixture endpoint.
    ///     metadata_timeout: Whole-call ceiling for one release-list page.
    ///
    /// Returns:
    ///     A test-only client that permits plain HTTP.
    #[cfg(test)]
    fn for_test_with_metadata_timeout(releases_url: &str, metadata_timeout: Duration) -> Self {
        Self::build_with(releases_url, false, metadata_timeout)
    }

    /// Download an eligible asset into a verified staged executable.
    ///
    /// Args:
    ///     release: Immutable release metadata selected by [`ReleaseDiscovery::scan`].
    ///     nonce: Validated transaction identifier generated by the transaction owner.
    ///
    /// Returns:
    ///     The staged path after streamed and post-sync SHA-256 verification on one open handle.
    pub fn download_verified(
        &self,
        release: &AvailableRelease,
        nonce: &str,
    ) -> anyhow::Result<PathBuf> {
        download::download_verified(self, release, nonce)
    }

    /// Download an eligible asset to an exact destination, such as the macOS disk image.
    ///
    /// Args:
    ///     release: Immutable release metadata selected by [`ReleaseDiscovery::scan`].
    ///     dest: Final file path; its parent directory is created when missing.
    ///
    /// Returns:
    ///     `dest` after the same size and SHA-256 verification as [`Self::download_verified`].
    pub fn download_verified_to(
        &self,
        release: &AvailableRelease,
        dest: &std::path::Path,
    ) -> anyhow::Result<PathBuf> {
        download::download_verified_to(self, release, dest)
    }
}

/// File name the macOS disk image of one release is saved under.
///
/// Args:
///     version: Release version the image installs.
///
/// Returns:
///     `MoonTerminal-<major>.<minor>.<patch>.dmg`, distinct per release so an older image in the
///     same folder is never mistaken for the new one.
pub fn installer_image_file_name(version: ReleaseVersion) -> String {
    format!(
        "MoonTerminal-{}.{}.{}.dmg",
        version.major, version.minor, version.patch
    )
}

/// Path in the user's Downloads folder where one release's disk image is saved.
///
/// Args:
///     version: Release version the image installs.
///
/// Returns:
///     [`crate::config::paths::user_download_path`] for [`installer_image_file_name`], or
///     `None` when no Downloads folder can be resolved.
pub fn installer_image_download_path(version: ReleaseVersion) -> Option<PathBuf> {
    crate::config::paths::user_download_path(&installer_image_file_name(version))
}

impl Default for GitHubReleaseClient {
    /// Build the production GitHub release client.
    fn default() -> Self {
        Self::new()
    }
}
