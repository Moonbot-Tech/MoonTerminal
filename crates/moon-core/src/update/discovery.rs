//! Bounded paginated discovery with transactional ETag caches and retry policy.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::anyhow;
use ureq::http::{HeaderMap, HeaderValue};

use super::release::{
    eligible_release, AvailableRelease, BuildIdentity, GitHubRelease, ReleaseVersion,
    UpdateEligibility,
};
use super::GitHubReleaseClient;
use crate::util::time::now_unix_secs;

const RELEASES_PER_PAGE: usize = 100;
const MAX_RELEASE_PAGES: usize = 2;
const MAX_RELEASE_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const PAGE_SENTINEL_SECONDS: u64 = 24 * 60 * 60;
const LOW_RATE_REMAINING: u64 = 5;
const MAX_ETAG_BYTES: usize = 512;
const GITHUB_API_VERSION: &str = "2022-11-28";

#[cfg(test)]
mod tests;

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
        let first = release_page(
            &self.client,
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
            let second = release_page(
                &self.client,
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

/// Fetch one exact conditional release-list page and classify its scheduling metadata.
///
/// Args:
///     client: Shared bounded GitHub transport facade.
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
    client: &GitHubReleaseClient,
    page: usize,
    etag: Option<&HeaderValue>,
    now_unix: u64,
) -> Result<PageFetch, DiscoveryError> {
    let mut request = client
        .agent
        .get(&client.releases_url)
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
            if let Some(existing_tag) = tags_by_version.get(&candidate.version()) {
                if existing_tag != candidate.release_tag() {
                    anyhow::bail!("release list contains ambiguous tags for one numeric version");
                }
            } else {
                tags_by_version.insert(candidate.version(), candidate.release_tag().to_owned());
            }
            let is_new_greatest = candidate.version() > baseline
                && greatest
                    .as_ref()
                    .is_none_or(|current| candidate.version() > current.version());
            if is_new_greatest {
                greatest = Some(candidate);
            }
        }
    }
    Ok(greatest)
}

/// Clone one opaque response ETag only while it stays inside the updater header bound.
fn bounded_etag(headers: &HeaderMap) -> Option<HeaderValue> {
    headers
        .get("etag")
        .filter(|value| value.as_bytes().len() <= MAX_ETAG_BYTES)
        .cloned()
}

/// Parse one strict unsigned-decimal response header.
fn decimal_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    let value = headers.get(name)?.to_str().ok()?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Return the later of two optional absolute deadlines.
fn later_deadline(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}
