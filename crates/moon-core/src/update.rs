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

use anyhow::{anyhow, bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use ureq::http::{HeaderMap, HeaderValue};

use crate::util::time::now_unix_secs;

#[cfg(test)]
mod tests;

const RELEASES_URL: &str = "https://api.github.com/repos/Moonbot-Tech/MoonTerminal/releases";
const DOWNLOAD_PREFIX: &str = "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/";
const WINDOWS_ASSET_NAME: &str = "MoonTerminal.exe";
const RELEASES_PER_PAGE: usize = 100;
const MAX_RELEASE_PAGES: usize = 2;
const MAX_RELEASE_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
const PAGE_SENTINEL_SECONDS: u64 = 24 * 60 * 60;
const LOW_RATE_REMAINING: u64 = 5;
const MAX_ETAG_BYTES: usize = 512;
const GITHUB_API_VERSION: &str = "2022-11-28";

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

/// Scheduling class for a failed release-discovery scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryRetry {
    /// GitHub explicitly refused work because a primary or secondary limit was reached.
    RateLimited,
    /// The request failed in transport or GitHub returned a temporary server failure.
    Transient,
    /// The response violated the bounded updater metadata contract.
    Protocol,
}

/// Failed discovery result carrying only scheduling-safe server metadata.
#[derive(Debug)]
pub struct DiscoveryError {
    retry: DiscoveryRetry,
    not_before_unix: Option<u64>,
    source: anyhow::Error,
}

impl DiscoveryError {
    /// Return the local retry policy class for this failure.
    pub fn retry(&self) -> DiscoveryRetry {
        self.retry
    }

    /// Return GitHub's absolute earliest retry time, when a valid header supplied one.
    pub fn not_before_unix(&self) -> Option<u64> {
        self.not_before_unix
    }

    /// Construct a typed discovery failure without exposing response bodies or headers.
    ///
    /// Args:
    ///     retry: Local scheduling class for the failure.
    ///     not_before_unix: Optional absolute server deadline.
    ///     source: Sanitized underlying error.
    ///
    /// Returns:
    ///     A discovery error safe to log and schedule.
    fn new(retry: DiscoveryRetry, not_before_unix: Option<u64>, source: anyhow::Error) -> Self {
        Self {
            retry,
            not_before_unix,
            source,
        }
    }
}

impl fmt::Display for DiscoveryError {
    /// Format only the sanitized updater error chain.
    ///
    /// Args:
    ///     formatter: Destination formatter supplied by the standard library.
    ///
    /// Returns:
    ///     The formatter result.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.source)
    }
}

impl std::error::Error for DiscoveryError {
    /// Return the underlying sanitized discovery error.
    ///
    /// Returns:
    ///     The wrapped error as a standard error source.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Complete successful scan plus any GitHub deadline that should delay the next scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryResult {
    /// Eligibility reduced from one complete cached first-200-release snapshot.
    pub eligibility: UpdateEligibility,
    /// Absolute reset time reported with a nearly exhausted successful response.
    pub defer_until_unix: Option<u64>,
}

/// Stateful conditional discovery session bound to one executable build identity.
pub struct ReleaseDiscovery {
    client: GitHubReleaseClient,
    identity: BuildIdentity,
    pages: [Option<CachedReleasePage>; MAX_RELEASE_PAGES],
    page_two_refresh_at: u64,
}

impl ReleaseDiscovery {
    /// Create a production HTTPS discovery session for one executable baseline.
    ///
    /// Args:
    ///     identity: Immutable release baseline embedded in the running executable.
    ///
    /// Returns:
    ///     A new HTTPS-only discovery session with an empty page cache.
    pub fn new(identity: BuildIdentity) -> Self {
        Self::with_client(identity, GitHubReleaseClient::new())
    }

    /// Revalidate the bounded release snapshot once.
    ///
    /// Returns:
    ///     Complete eligibility and an optional GitHub reset deadline.
    ///
    /// Errors:
    ///     Returns a typed failure without committing a partial page-cache update.
    pub fn scan(&mut self) -> Result<DiscoveryResult, DiscoveryError> {
        self.scan_at(now_unix_secs())
    }

    /// Build a discovery session around an explicit transport.
    ///
    /// Args:
    ///     identity: Immutable release baseline for eligibility decisions.
    ///     client: Transport used for release-list requests.
    ///
    /// Returns:
    ///     A discovery session with an empty page cache.
    fn with_client(identity: BuildIdentity, client: GitHubReleaseClient) -> Self {
        Self {
            client,
            identity,
            pages: [None, None],
            page_two_refresh_at: 0,
        }
    }

    /// Revalidate at a fixed Unix time so retry and sentinel decisions stay deterministic.
    ///
    /// Args:
    ///     now_unix: Current whole Unix seconds used for deadline comparisons.
    ///
    /// Returns:
    ///     Complete eligibility and an optional successful-response deadline.
    ///
    /// Errors:
    ///     Returns a typed failure without committing an incomplete page snapshot.
    fn scan_at(&mut self, now_unix: u64) -> Result<DiscoveryResult, DiscoveryError> {
        let Some(baseline) = self.identity.baseline() else {
            return Ok(DiscoveryResult {
                eligibility: UpdateEligibility::Unsupported,
                defer_until_unix: None,
            });
        };

        let mut staged = self.pages.clone();
        let first = self.client.release_page(
            1,
            staged[0].as_ref().and_then(|page| page.etag.as_ref()),
            now_unix,
        )?;
        let first_low = first.rate.low_remaining();
        let mut defer_until = first.rate.success_defer_until(now_unix);
        let first_changed = apply_page_update(&mut staged[0], first.update)?;
        let first_len = staged[0].as_ref().map_or(0, |page| page.releases.len());

        if first_len < RELEASES_PER_PAGE {
            staged[1] = None;
            let eligibility = eligibility_from_pages(&staged, baseline)?;
            self.pages = staged;
            self.page_two_refresh_at = 0;
            return Ok(DiscoveryResult {
                eligibility,
                defer_until_unix: defer_until,
            });
        }

        let page_two_due =
            staged[1].is_none() || first_changed || now_unix >= self.page_two_refresh_at;
        if page_two_due && first_low {
            if first_changed || staged[1].is_none() {
                return Err(DiscoveryError::new(
                    DiscoveryRetry::RateLimited,
                    defer_until,
                    anyhow!("GitHub release scan deferred before its required second page"),
                ));
            }
        }
        let mut next_page_two_refresh_at = self.page_two_refresh_at;
        if page_two_due && !first_low {
            let second = self.client.release_page(
                2,
                staged[1].as_ref().and_then(|page| page.etag.as_ref()),
                now_unix,
            )?;
            defer_until = later_deadline(defer_until, second.rate.success_defer_until(now_unix));
            apply_page_update(&mut staged[1], second.update)?;
            next_page_two_refresh_at = now_unix.saturating_add(PAGE_SENTINEL_SECONDS);
        }

        let eligibility = eligibility_from_pages(&staged, baseline)?;
        self.pages = staged;
        self.page_two_refresh_at = next_page_two_refresh_at;
        Ok(DiscoveryResult {
            eligibility,
            defer_until_unix: defer_until,
        })
    }
}

/// One complete bounded release-list representation and its exact validator.
#[derive(Clone)]
struct CachedReleasePage {
    etag: Option<HeaderValue>,
    releases: Vec<GitHubRelease>,
}

/// Conditional response action for one exact release-list page.
enum PageUpdate {
    /// Replace the cached representation with a newly decoded response.
    Fresh(CachedReleasePage),
    /// Reuse the complete cached representation for this exact page.
    NotModified,
}

/// One page response split into cache mutation and request-budget metadata.
struct PageFetch {
    update: PageUpdate,
    rate: RateHeaders,
}

/// Sanitized GitHub rate-limit headers relevant to future request scheduling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RateHeaders {
    remaining: Option<u64>,
    reset_unix: Option<u64>,
    retry_after_unix: Option<u64>,
}

impl RateHeaders {
    /// Parse only strict decimal GitHub scheduling headers.
    ///
    /// Args:
    ///     headers: Untrusted response headers.
    ///     now_unix: Current whole Unix seconds for relative and expired deadlines.
    ///
    /// Returns:
    ///     Sanitized optional rate-limit metadata.
    fn from_headers(headers: &HeaderMap, now_unix: u64) -> Self {
        let remaining = decimal_header(headers, "x-ratelimit-remaining");
        let reset_unix =
            decimal_header(headers, "x-ratelimit-reset").filter(|deadline| *deadline > now_unix);
        let retry_after_unix =
            decimal_header(headers, "retry-after").map(|seconds| now_unix.saturating_add(seconds));
        Self {
            remaining,
            reset_unix,
            retry_after_unix,
        }
    }

    /// Return whether optional pagination should preserve the remaining shared-IP budget.
    fn low_remaining(self) -> bool {
        self.remaining
            .is_some_and(|remaining| remaining <= LOW_RATE_REMAINING)
    }

    /// Return the reset deadline carried by a nearly exhausted successful response.
    ///
    /// Args:
    ///     now_unix: Current whole Unix seconds used to reject stale deadlines.
    ///
    /// Returns:
    ///     A future reset deadline only when the remaining budget is low.
    fn success_defer_until(self, now_unix: u64) -> Option<u64> {
        self.low_remaining()
            .then_some(self.reset_unix)
            .flatten()
            .filter(|deadline| *deadline > now_unix)
    }

    /// Return the latest valid server deadline for a rejected request.
    fn rejected_not_before(self) -> Option<u64> {
        later_deadline(
            self.retry_after_unix,
            (self.remaining == Some(0))
                .then_some(self.reset_unix)
                .flatten(),
        )
    }
}

/// Synchronous GitHub client intended for a dedicated background executor.
#[derive(Clone)]
pub struct GitHubReleaseClient {
    agent: ureq::Agent,
    releases_url: String,
}

impl GitHubReleaseClient {
    /// Build a bounded HTTPS-only client for public release metadata and assets.
    ///
    /// Returns:
    ///     Client with a global timeout and bounded redirect chain.
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
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .https_only(https_only)
            .max_redirects(5)
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
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

    /// Fetch one exact conditional release-list page and classify its scheduling metadata.
    ///
    /// Args:
    ///     page: One-based release-list page number.
    ///     etag: Optional exact validator for that page URL.
    ///     now_unix: Current whole Unix seconds for scheduling headers.
    ///
    /// Returns:
    ///     A fresh or not-modified page action plus sanitized rate metadata.
    ///
    /// Errors:
    ///     Returns a typed transport, rate-limit, or protocol failure without exposing headers or
    ///     response bodies.
    fn release_page(
        &self,
        page: usize,
        etag: Option<&HeaderValue>,
        now_unix: u64,
    ) -> Result<PageFetch, DiscoveryError> {
        let mut request = self
            .agent
            .get(&self.releases_url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "MoonTerminal-updater")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .query("per_page", RELEASES_PER_PAGE.to_string())
            .query("page", page.to_string());
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag.clone());
        }
        let mut response = request.call().map_err(|error| {
            DiscoveryError::new(
                DiscoveryRetry::Transient,
                None,
                anyhow!(error).context("request GitHub releases"),
            )
        })?;
        let status = response.status().as_u16();
        let rate = RateHeaders::from_headers(response.headers(), now_unix);
        if status == 304 {
            return Ok(PageFetch {
                update: PageUpdate::NotModified,
                rate,
            });
        }
        if status == 403 || status == 429 {
            return Err(DiscoveryError::new(
                DiscoveryRetry::RateLimited,
                rate.rejected_not_before(),
                anyhow!("GitHub releases returned HTTP {status}"),
            ));
        }
        if (500..600).contains(&status) {
            return Err(DiscoveryError::new(
                DiscoveryRetry::Transient,
                rate.rejected_not_before(),
                anyhow!("GitHub releases returned HTTP {status}"),
            ));
        }
        if status != 200 {
            return Err(DiscoveryError::new(
                DiscoveryRetry::Protocol,
                None,
                anyhow!("GitHub releases returned HTTP {status}"),
            ));
        }
        let etag = bounded_etag(response.headers());
        let releases = response
            .body_mut()
            .with_config()
            .limit(MAX_RELEASE_RESPONSE_BYTES)
            .read_json()
            .map_err(|error| {
                DiscoveryError::new(
                    DiscoveryRetry::Protocol,
                    None,
                    anyhow!(error).context("decode bounded GitHub releases response"),
                )
            })?;
        Ok(PageFetch {
            update: PageUpdate::Fresh(CachedReleasePage { etag, releases }),
            rate,
        })
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

/// Apply one conditional page response to its exact cached representation.
///
/// Args:
///     cached: Cache slot for the exact requested page URL.
///     update: Fresh representation or not-modified action.
///
/// Returns:
///     Whether a fresh representation replaced the slot.
///
/// Errors:
///     Returns a protocol failure when `304` has no matching cached validator.
fn apply_page_update(
    cached: &mut Option<CachedReleasePage>,
    update: PageUpdate,
) -> Result<bool, DiscoveryError> {
    match update {
        PageUpdate::Fresh(page) => {
            *cached = Some(page);
            Ok(true)
        }
        PageUpdate::NotModified if cached.as_ref().is_some_and(|page| page.etag.is_some()) => {
            Ok(false)
        }
        PageUpdate::NotModified => Err(DiscoveryError::new(
            DiscoveryRetry::Protocol,
            None,
            anyhow!("GitHub returned 304 without a matching cached validator"),
        )),
    }
}

/// Reduce one complete cached snapshot through the existing immutable-release policy.
///
/// Args:
///     pages: Complete committed or staged first-200 page snapshot.
///     baseline: Embedded executable version used for strict-newer comparison.
///
/// Returns:
///     Current or available eligibility for a supported build identity.
///
/// Errors:
///     Returns a protocol failure for malformed or ambiguous eligible release metadata.
fn eligibility_from_pages(
    pages: &[Option<CachedReleasePage>; MAX_RELEASE_PAGES],
    baseline: ReleaseVersion,
) -> Result<UpdateEligibility, DiscoveryError> {
    let mut greatest = None;
    let mut tags_by_version = BTreeMap::new();
    for page in pages.iter().flatten() {
        greatest = greatest_eligible(&page.releases, baseline, greatest, &mut tags_by_version)
            .map_err(|error| DiscoveryError::new(DiscoveryRetry::Protocol, None, error))?;
    }
    Ok(greatest.map_or(UpdateEligibility::Current, UpdateEligibility::Available))
}

/// Clone one opaque response ETag only while it stays inside the updater header bound.
///
/// Args:
///     headers: Untrusted response headers.
///
/// Returns:
///     The opaque validator when present and within the byte bound.
fn bounded_etag(headers: &HeaderMap) -> Option<HeaderValue> {
    headers
        .get("etag")
        .filter(|value| value.as_bytes().len() <= MAX_ETAG_BYTES)
        .cloned()
}

/// Parse one strict unsigned-decimal response header.
///
/// Args:
///     headers: Untrusted response headers.
///     name: Case-insensitive header name to inspect.
///
/// Returns:
///     A parsed unsigned integer only when every byte is an ASCII digit.
fn decimal_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    let value = headers.get(name)?.to_str().ok()?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Return the later of two optional absolute deadlines.
///
/// Args:
///     left: First optional Unix deadline.
///     right: Second optional Unix deadline.
///
/// Returns:
///     The later present deadline, or `None` when both are absent.
fn later_deadline(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

/// Minimal release-list response used for fail-closed eligibility decisions.
#[derive(Clone, Debug, Deserialize)]
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
///
/// Args:
///     release: One bounded release-list record borrowed from the page cache.
///
/// Returns:
///     A cloned install candidate when every stable immutable metadata check passes.
///
/// Errors:
///     Returns an error for duplicate Windows assets or a malformed canonical download URL.
fn eligible_release(release: &GitHubRelease) -> anyhow::Result<Option<AvailableRelease>> {
    if release.draft || release.prerelease || !release.immutable {
        return Ok(None);
    }
    let Some(version) = ReleaseVersion::parse(&release.tag_name) else {
        return Ok(None);
    };
    let mut matching = release
        .assets
        .iter()
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
        release_tag: release.tag_name.clone(),
        asset: ReleaseAsset {
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
///     The greatest strictly newer eligible release after reading the page.
///
/// Errors:
///     Returns an error for malformed eligible metadata or distinct tags with one numeric version.
fn greatest_eligible(
    releases: &[GitHubRelease],
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
