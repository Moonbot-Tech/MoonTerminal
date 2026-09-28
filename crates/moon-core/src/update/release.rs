//! Immutable release eligibility and exact asset metadata policy.

use std::fmt;

use anyhow::bail;
use serde::Deserialize;

const DOWNLOAD_PREFIX: &str = "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/";
const WINDOWS_ASSET_NAME: &str = "MoonTerminal.exe";
const MACOS_ASSET_NAME: &str = "MoonTerminal.dmg";
pub(super) const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

#[cfg(test)]
mod tests;

/// Exact release asset name the update path installs on one target OS.
///
/// Args:
///     target_os: Value of `std::env::consts::OS` for the build being served.
///
/// Returns:
///     `MoonTerminal.dmg` on macOS and `MoonTerminal.exe` everywhere else.
pub(super) fn asset_name_for_os(target_os: &str) -> &'static str {
    if target_os == "macos" {
        MACOS_ASSET_NAME
    } else {
        WINDOWS_ASSET_NAME
    }
}

/// Exact release asset name for the platform this binary was compiled for.
pub(super) fn platform_asset_name() -> &'static str {
    asset_name_for_os(std::env::consts::OS)
}

/// Stable release version encoded by legacy `v0.21` or canonical `v0.24.1` tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseVersion {
    /// Stable release major component.
    pub major: u64,
    /// Stable release minor component.
    pub minor: u64,
    /// Stable release patch component, or zero for a legacy two-component tag.
    pub patch: u64,
}

impl ReleaseVersion {
    /// Parse a canonical stable tag without prerelease or build suffixes.
    ///
    /// Args:
    ///     tag: Candidate Git tag.
    ///
    /// Returns:
    ///     Parsed version for `vMAJOR.MINOR` or `vMAJOR.MINOR.PATCH` without leading zeroes.
    pub fn parse(tag: &str) -> Option<Self> {
        let body = tag.strip_prefix('v')?;
        let mut components = body.split('.');
        let major = components.next()?;
        let minor = components.next()?;
        let patch = components.next().unwrap_or("0");
        if components.next().is_some()
            || !canonical_number(major)
            || !canonical_number(minor)
            || !canonical_number(patch)
        {
            return None;
        }
        Some(Self {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
            patch: patch.parse().ok()?,
        })
    }
}

impl fmt::Display for ReleaseVersion {
    /// Format the normalized version in three-component display form.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "v{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Embedded stable release baseline used to decide whether an update is newer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    baseline: Option<ReleaseVersion>,
}

impl BuildIdentity {
    /// Parse the release baseline emitted by the GPUI build script.
    ///
    /// Args:
    ///     release_base: Canonical stable tag or `unknown`.
    ///
    /// Returns:
    ///     Identity that fails closed when the baseline is absent or malformed.
    pub fn from_release_base(release_base: &str) -> Self {
        Self {
            baseline: ReleaseVersion::parse(release_base),
        }
    }

    /// Return the trusted comparison baseline, if one was embedded.
    ///
    /// Returns:
    ///     Canonical stable version or `None` for development/unknown builds.
    pub fn baseline(self) -> Option<ReleaseVersion> {
        self.baseline
    }
}

/// Exact release asset metadata needed for a verified download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// Exact asset file name the URL ends with.
    name: &'static str,
    /// HTTPS GitHub release download URL.
    download_url: String,
    /// Byte length reported by the immutable release metadata.
    size: u64,
    /// Expected SHA-256 digest bytes.
    sha256: [u8; 32],
}

/// Stable immutable release eligible for installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableRelease {
    /// Canonical stable release version.
    version: ReleaseVersion,
    /// Exact Git tag returned by the immutable release metadata.
    release_tag: String,
    /// Exact platform asset (`MoonTerminal.exe` or `MoonTerminal.dmg`).
    asset: ReleaseAsset,
}

impl AvailableRelease {
    /// Return the canonical stable version offered to the user.
    pub fn version(&self) -> ReleaseVersion {
        self.version
    }

    /// Return the exact immutable release tag used by GitHub URLs and the installer manifest.
    ///
    /// Returns:
    ///     The original two- or three-component Git tag without numeric reconstruction.
    pub fn release_tag(&self) -> &str {
        &self.release_tag
    }

    /// Return the immutable asset byte length.
    pub fn asset_size(&self) -> u64 {
        self.asset.size
    }

    /// Return the immutable asset SHA-256 digest.
    pub fn asset_sha256(&self) -> [u8; 32] {
        self.asset.sha256
    }

    /// Return the exact asset file name selected for this platform.
    pub fn asset_name(&self) -> &'static str {
        self.asset.name
    }

    /// Return the canonical GitHub download URL retained from immutable metadata.
    pub(super) fn download_url(&self) -> &str {
        &self.asset.download_url
    }
}

/// Result of comparing release metadata with the embedded build identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateEligibility {
    /// The build has no trustworthy stable baseline.
    Unsupported,
    /// No strictly newer eligible stable release exists.
    Current,
    /// A strictly newer immutable release with a verified-asset contract exists.
    Available(AvailableRelease),
}

/// Minimal release-list response used for fail-closed eligibility decisions.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    immutable: bool,
    assets: Vec<GitHubAsset>,
}

/// Minimal GitHub asset response needed for exact platform asset selection.
#[derive(Clone, Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

/// Convert one API release into an eligible candidate for this platform.
///
/// Args:
///     release: One bounded release-list record borrowed from the page cache.
///
/// Returns:
///     A cloned install candidate when every stable immutable metadata check passes.
///
/// Errors:
///     Returns an error for duplicate platform assets or a malformed canonical download URL.
pub(super) fn eligible_release(
    release: &GitHubRelease,
) -> anyhow::Result<Option<AvailableRelease>> {
    eligible_release_for(release, platform_asset_name())
}

/// Convert one API release into an eligible candidate carrying one exact asset name.
///
/// Args:
///     release: One bounded release-list record borrowed from the page cache.
///     asset_name: Exact platform asset name from [`asset_name_for_os`].
///
/// Returns:
///     A cloned install candidate when every stable immutable metadata check passes.
///
/// Errors:
///     Returns an error for duplicate assets of that name or a malformed canonical download URL.
pub(super) fn eligible_release_for(
    release: &GitHubRelease,
    asset_name: &'static str,
) -> anyhow::Result<Option<AvailableRelease>> {
    if release.draft || release.prerelease || !release.immutable {
        return Ok(None);
    }
    let Some(version) = ReleaseVersion::parse(&release.tag_name) else {
        return Ok(None);
    };
    let mut matching = release
        .assets
        .iter()
        .filter(|asset| asset.name == asset_name);
    let Some(asset) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        bail!(
            "release {} has duplicate {asset_name} assets",
            release.tag_name
        );
    }
    if asset.size == 0 || asset.size > MAX_EXECUTABLE_BYTES {
        return Ok(None);
    }
    validate_download_url(&asset.browser_download_url, &release.tag_name, asset_name)?;
    let Some(digest) = asset.digest.as_deref().and_then(parse_sha256) else {
        return Ok(None);
    };
    Ok(Some(AvailableRelease {
        version,
        release_tag: release.tag_name.clone(),
        asset: ReleaseAsset {
            name: asset_name,
            download_url: asset.browser_download_url.clone(),
            size: asset.size,
            sha256: digest,
        },
    }))
}

/// Require an exact GitHub release-download namespace and matching source tag.
///
/// Args:
///     url: Candidate browser-download URL returned by GitHub.
///     release_tag: Exact stable tag from the same immutable release object.
///     asset_name: Exact asset file name the URL must end with.
///
/// Errors:
///     Returns an error when the tag is malformed or the URL names another repository, tag, or
///     asset.
pub(super) fn validate_download_url(
    url: &str,
    release_tag: &str,
    asset_name: &str,
) -> anyhow::Result<()> {
    if ReleaseVersion::parse(release_tag).is_none() {
        bail!("release asset URL carries a non-canonical Git tag");
    }
    let expected = format!("{DOWNLOAD_PREFIX}{release_tag}/");
    if url.strip_prefix(&expected) != Some(asset_name) {
        bail!("release asset URL is outside the canonical GitHub namespace");
    }
    Ok(())
}

/// Parse GitHub's mandatory `sha256:` asset digest into fixed bytes.
pub(super) fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    let hex = value.strip_prefix("sha256:")?;
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    for (index, output) in digest.iter_mut().enumerate() {
        let offset = index * 2;
        *output = u8::from_str_radix(&hex[offset..offset + 2], 16).ok()?;
    }
    Some(digest)
}

/// Reject leading zeroes while accepting the single digit zero.
fn canonical_number(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_digit()) && (value == "0" || !value.starts_with('0'))
}
