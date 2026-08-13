//! Stable GitHub release discovery and verified update downloads.
//!
//! This module deliberately stops at a verified staged executable. Process coordination,
//! replacement, restart, and rollback belong to the Windows GPUI shell.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

const RELEASES_URL: &str = "https://api.github.com/repos/Moonbot-Tech/MoonTerminal/releases";
const DOWNLOAD_PREFIX: &str = "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/";
const WINDOWS_ASSET_NAME: &str = "MoonTerminal.exe";
const RELEASES_PER_PAGE: usize = 50;
const MAX_RELEASE_PAGES: usize = 4;
const MAX_RELEASE_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

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
    /// Exact Windows executable asset.
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

/// Synchronous GitHub client intended for a dedicated background executor.
#[derive(Clone)]
pub struct GitHubReleaseClient {
    agent: ureq::Agent,
}

impl GitHubReleaseClient {
    /// Build a bounded HTTPS-only client for public release metadata and assets.
    ///
    /// Returns:
    ///     Client with a global timeout and bounded redirect chain.
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .https_only(true)
            .max_redirects(5)
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// Find the greatest eligible stable release newer than the current build.
    ///
    /// Args:
    ///     identity: Embedded stable baseline for this executable.
    ///
    /// Returns:
    ///     Unsupported, current, or the greatest eligible immutable release.
    pub fn latest_stable(&self, identity: BuildIdentity) -> anyhow::Result<UpdateEligibility> {
        let Some(baseline) = identity.baseline() else {
            return Ok(UpdateEligibility::Unsupported);
        };
        let mut greatest = None;
        let mut tags_by_version = BTreeMap::new();
        for page in 1..=MAX_RELEASE_PAGES {
            let releases = self.release_page(page)?;
            let page_len = releases.len();
            greatest = greatest_eligible(releases, baseline, greatest, &mut tags_by_version)?;
            if page_len < RELEASES_PER_PAGE {
                break;
            }
        }
        Ok(greatest.map_or(UpdateEligibility::Current, UpdateEligibility::Available))
    }

    /// Download an eligible asset into a verified staged executable.
    ///
    /// Args:
    ///     release: Immutable release metadata selected by [`Self::latest_stable`].
    ///     nonce: Validated transaction identifier generated by the transaction owner.
    ///
    /// Returns:
    ///     The staged path after streamed and post-sync SHA-256 verification on one open handle.
    pub fn download_verified(
        &self,
        release: &AvailableRelease,
        nonce: &str,
    ) -> anyhow::Result<PathBuf> {
        let staged_path = crate::config::paths::update_staged_executable_path(nonce)
            .context("resolve canonical update staging path")?;
        validate_download_url(&release.asset.download_url, release.release_tag())?;
        if release.asset.size == 0 || release.asset.size > MAX_EXECUTABLE_BYTES {
            bail!("release asset size is outside the allowed range");
        }
        let parent = staged_path
            .parent()
            .ok_or_else(|| anyhow!("staged executable has no parent directory"))?;
        ensure_plain_directory(parent)?;
        let (part_path, part_file) = create_unique_part(&staged_path)?;
        let mut part = PartFile::new(part_path, part_file);
        self.download_into(release, part.file_mut())?;
        let part_path = part.path().to_path_buf();
        promote_verified_part(
            &part_path,
            part.file_mut(),
            &staged_path,
            release.asset.size,
            &release.asset.sha256,
        )?;
        part.disarm();
        Ok(staged_path)
    }

    /// Fetch and decode one bounded release-list page.
    fn release_page(&self, page: usize) -> anyhow::Result<Vec<GitHubRelease>> {
        let mut response = self
            .agent
            .get(RELEASES_URL)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "MoonTerminal-updater")
            .query("per_page", RELEASES_PER_PAGE.to_string())
            .query("page", page.to_string())
            .call()
            .context("request GitHub releases")?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            bail!("GitHub releases returned HTTP {status}");
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_RELEASE_RESPONSE_BYTES)
            .read_json()
            .context("decode bounded GitHub releases response")
    }

    /// Stream one response into the already-exclusive part file and verify its first hash.
    fn download_into(
        &self,
        release: &AvailableRelease,
        part_file: &mut File,
    ) -> anyhow::Result<()> {
        let mut response = self
            .agent
            .get(&release.asset.download_url)
            .header("Accept", "application/octet-stream")
            .header("User-Agent", "MoonTerminal-updater")
            .call()
            .context("download update asset")?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            bail!("GitHub update asset returned HTTP {status}");
        }
        copy_verified(
            response.body_mut().as_reader(),
            part_file,
            release.asset.size,
            &release.asset.sha256,
        )
    }
}

impl Default for GitHubReleaseClient {
    /// Build the production GitHub release client.
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal release-list response used for fail-closed eligibility decisions.
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    immutable: bool,
    assets: Vec<GitHubAsset>,
}

/// Minimal GitHub asset response needed for exact Windows selection.
#[derive(Clone, Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

/// Convert one API release into an eligible Windows candidate.
fn eligible_release(release: GitHubRelease) -> anyhow::Result<Option<AvailableRelease>> {
    if release.draft || release.prerelease || !release.immutable {
        return Ok(None);
    }
    let Some(version) = ReleaseVersion::parse(&release.tag_name) else {
        return Ok(None);
    };
    let mut matching = release
        .assets
        .into_iter()
        .filter(|asset| asset.name == WINDOWS_ASSET_NAME);
    let Some(asset) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        bail!("release {} has duplicate Windows assets", release.tag_name);
    }
    if asset.size == 0 || asset.size > MAX_EXECUTABLE_BYTES {
        return Ok(None);
    }
    validate_download_url(&asset.browser_download_url, &release.tag_name)?;
    let Some(digest) = asset.digest.as_deref().and_then(parse_sha256) else {
        return Ok(None);
    };
    Ok(Some(AvailableRelease {
        version,
        release_tag: release.tag_name,
        asset: ReleaseAsset {
            download_url: asset.browser_download_url,
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
///
/// Errors:
///     Returns an error when the tag is malformed or the URL names another repository, tag, or
///     asset.
fn validate_download_url(url: &str, release_tag: &str) -> anyhow::Result<()> {
    if ReleaseVersion::parse(release_tag).is_none() {
        bail!("release asset URL carries a non-canonical Git tag");
    }
    let expected = format!("{DOWNLOAD_PREFIX}{release_tag}/");
    if url.strip_prefix(&expected) != Some(WINDOWS_ASSET_NAME) {
        bail!("release asset URL is outside the canonical GitHub namespace");
    }
    Ok(())
}

/// Reduce one release page into the greatest unambiguous candidate seen so far.
///
/// Args:
///     releases: One bounded page of GitHub release metadata.
///     baseline: Embedded numeric version below which candidates remain hidden.
///     greatest: Greatest eligible candidate from earlier releases or pages.
///     tags_by_version: Exact-tag authority used to reject semantic aliases across all pages.
///
/// Returns:
///     The greatest strictly newer eligible release after consuming the page.
///
/// Errors:
///     Returns an error for malformed eligible metadata or distinct tags with one numeric version.
fn greatest_eligible(
    releases: Vec<GitHubRelease>,
    baseline: ReleaseVersion,
    mut greatest: Option<AvailableRelease>,
    tags_by_version: &mut BTreeMap<ReleaseVersion, String>,
) -> anyhow::Result<Option<AvailableRelease>> {
    for release in releases {
        if let Some(candidate) = eligible_release(release)? {
            if let Some(existing_tag) = tags_by_version.get(&candidate.version) {
                if existing_tag != &candidate.release_tag {
                    bail!("release list contains ambiguous tags for one numeric version");
                }
            } else {
                tags_by_version.insert(candidate.version, candidate.release_tag.clone());
            }
            let is_new_greatest = candidate.version > baseline
                && greatest
                    .as_ref()
                    .is_none_or(|current| candidate.version > current.version);
            if is_new_greatest {
                greatest = Some(candidate);
            }
        }
    }
    Ok(greatest)
}

/// Sync, independently rehash, and atomically name one verified part file.
fn promote_verified_part(
    part_path: &Path,
    part_file: &mut File,
    staged_path: &Path,
    expected_size: u64,
    expected_digest: &[u8; 32],
) -> anyhow::Result<()> {
    part_file
        .sync_all()
        .with_context(|| format!("sync downloaded update {}", part_path.display()))?;
    let (size, digest) = sha256_open_file(part_file)?;
    if size != expected_size || !constant_time_eq(&digest, expected_digest) {
        bail!("download changed between streamed and post-sync verification");
    }
    fs::rename(part_path, staged_path).with_context(|| {
        format!(
            "promote verified update {} to {}",
            part_path.display(),
            staged_path.display()
        )
    })
}

/// Exclusive part file that removes itself unless promotion completes.
struct PartFile {
    path: PathBuf,
    file: Option<File>,
    armed: bool,
}

impl PartFile {
    /// Bind cleanup ownership to an open exclusive part file.
    fn new(path: PathBuf, file: File) -> Self {
        Self {
            path,
            file: Some(file),
            armed: true,
        }
    }

    /// Borrow the open part file for streaming and synchronization.
    fn file_mut(&mut self) -> &mut File {
        self.file.as_mut().expect("part file remains open")
    }

    /// Return the exact path owned by this cleanup guard.
    fn path(&self) -> &Path {
        &self.path
    }

    /// Release cleanup ownership after the part path has been promoted.
    fn disarm(&mut self) {
        self.armed = false;
        self.file = None;
    }
}

impl Drop for PartFile {
    /// Close and remove only the owned part path after any failed step.
    fn drop(&mut self) {
        self.file = None;
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Stream bytes into a destination while enforcing exact size and SHA-256.
fn copy_verified(
    mut reader: impl Read,
    mut writer: impl Write,
    expected_size: u64,
    expected_digest: &[u8; 32],
) -> anyhow::Result<()> {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).context("read update response")?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| anyhow!("download size overflow"))?;
        if total > expected_size || total > MAX_EXECUTABLE_BYTES {
            bail!("download exceeds the immutable release size");
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .context("write staged update")?;
    }
    let digest: [u8; 32] = hasher.finalize().into();
    if total != expected_size || !constant_time_eq(&digest, expected_digest) {
        bail!("download size or SHA-256 does not match immutable release metadata");
    }
    Ok(())
}

/// Create an exclusive random part file beside the final staged path.
fn create_unique_part(staged_path: &Path) -> anyhow::Result<(PathBuf, File)> {
    for _ in 0..8 {
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random).context("generate update staging nonce")?;
        let suffix = encode_hex(&random);
        let file_name = staged_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("staged executable name is not valid UTF-8"))?;
        let part_path = staged_path.with_file_name(format!("{file_name}.{suffix}.part"));
        match OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&part_path)
        {
            Ok(file) => return Ok((part_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("create exclusive update part {}", part_path.display())
                });
            }
        }
    }
    bail!("could not allocate an exclusive update part file")
}

/// Hash the still-open exclusive staged file for the independent second verification pass.
fn sha256_open_file(file: &mut File) -> anyhow::Result<(u64, [u8; 32])> {
    file.seek(SeekFrom::Start(0))
        .context("rewind staged update for hashing")?;
    let mut hasher = Sha256::new();
    let size = io::copy(file, &mut hasher).context("hash staged update")?;
    Ok((size, hasher.finalize().into()))
}

/// Create a transaction directory only when every update component is a plain directory.
fn ensure_plain_directory(path: &Path) -> anyhow::Result<()> {
    let root = path
        .parent()
        .ok_or_else(|| anyhow!("update transaction directory has no root"))?;
    match fs::symlink_metadata(root) {
        Ok(metadata) if crate::config::paths::is_plain_directory(&metadata) => {}
        Ok(_) => bail!("update root is not a plain directory"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(root)
                .with_context(|| format!("create update root {}", root.display()))?;
        }
        Err(error) => return Err(error).context("inspect update root"),
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if crate::config::paths::is_plain_directory(&metadata) => Ok(()),
        Ok(_) => bail!("update transaction path is not a plain directory"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .with_context(|| format!("create update transaction {}", path.display()))?;
            let metadata = fs::symlink_metadata(path).context("reinspect update transaction")?;
            if crate::config::paths::is_plain_directory(&metadata) {
                Ok(())
            } else {
                bail!("created update transaction is not a plain directory")
            }
        }
        Err(error) => Err(error).context("inspect update transaction"),
    }
}

/// Parse GitHub's mandatory `sha256:` asset digest into fixed bytes.
fn parse_sha256(value: &str) -> Option<[u8; 32]> {
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

/// Compare digest bytes without an early-exit mismatch branch.
fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

/// Encode random bytes for a filesystem-safe transaction suffix.
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

/// Reject leading zeroes while accepting the single digit zero.
fn canonical_number(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_digit()) && (value == "0" || !value.starts_with('0'))
}
