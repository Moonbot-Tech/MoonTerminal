//! Verified `cloudflared` acquisition using centralized [`crate::config::paths`].
//!
//! Official Cloudflare GitHub releases are selected by `target_os`/`target_arch`. A platform
//! is refused when the current release metadata cannot supply a checksum or GitHub digest.
//! GitHub TLS alone is never treated as integrity. No download runs from constructors.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use flate2::read::GzDecoder;
use serde::Deserialize;

use crate::config::paths;
use crate::update::{DiscoveryError, DiscoveryRetry, GitHubReleaseClient};
use crate::util::time::now_unix_secs;

const CLOUDFLARED_RELEASES_URL: &str =
    "https://api.github.com/repos/cloudflare/cloudflared/releases";
const CLOUDFLARED_DOWNLOAD_PREFIX: &str =
    "https://github.com/cloudflare/cloudflared/releases/download/";
const USER_AGENT: &str = "MoonTerminal-telegram";
const MAX_CLOUDFLARED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RELEASE_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Why an official `cloudflared` binary cannot be selected or installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloudflaredUnavailable {
    /// No official asset is published for this OS/arch pair.
    UnsupportedTarget {
        /// `std::env::consts::OS`.
        os: &'static str,
        /// `std::env::consts::ARCH`.
        arch: &'static str,
    },
    /// The current release did not publish a checksum or GitHub digest for the asset.
    ChecksumMissing {
        /// Official asset file name.
        asset: &'static str,
    },
    /// Body checksum and GitHub digest disagreed.
    ChecksumMismatch {
        /// Official asset file name.
        asset: &'static str,
    },
    /// No non-draft, non-prerelease Cloudflare release was listed.
    NoStableRelease,
    /// GitHub asked the client to wait.
    RateLimited,
    /// Transport or 5xx failure.
    Transport,
    /// Response violated the bounded metadata contract.
    Protocol,
    /// Asset URL was outside the official Cloudflare GitHub download namespace.
    InvalidUrl,
    /// macOS tarball did not contain a `cloudflared` regular file.
    ArchiveLayout,
}

/// Official Cloudflare asset chosen for this target, with a required digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudflaredAsset {
    /// Exact Git tag from the GitHub release.
    pub tag: String,
    /// Official asset file name.
    pub name: &'static str,
    /// Canonical GitHub download URL.
    pub download_url: String,
    /// Size reported by GitHub.
    pub size: u64,
    /// SHA-256 from the official checksum list and/or GitHub digest.
    pub sha256: [u8; 32],
    /// Whether the downloaded bytes are a gzip tar that must be unpacked.
    pub gzip_tar: bool,
}

/// Return the official Cloudflare asset name for the compiled target, if one exists.
pub fn official_asset_name() -> Result<(&'static str, bool), CloudflaredUnavailable> {
    official_asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Map a Rust target triple OS/arch onto Cloudflare's published asset names.
///
/// Args:
///     os: `target_os` value such as `windows`, `linux`, or `macos`.
///     arch: `target_arch` value such as `x86_64` or `aarch64`.
///
/// Returns:
///     `(asset_name, gzip_tar)` for a raw binary or a `.tgz` archive.
///
/// Errors:
///     [`CloudflaredUnavailable::UnsupportedTarget`] when Cloudflare publishes no matching asset.
pub fn official_asset_name_for(
    os: &str,
    arch: &str,
) -> Result<(&'static str, bool), CloudflaredUnavailable> {
    let spec = match (os, arch) {
        ("windows", "x86_64") => ("cloudflared-windows-amd64.exe", false),
        ("windows", "x86") => ("cloudflared-windows-386.exe", false),
        ("linux", "x86_64") => ("cloudflared-linux-amd64", false),
        ("linux", "aarch64") => ("cloudflared-linux-arm64", false),
        ("linux", "x86") => ("cloudflared-linux-386", false),
        ("linux", "arm") => ("cloudflared-linux-arm", false),
        ("macos", "x86_64") => ("cloudflared-darwin-amd64.tgz", true),
        ("macos", "aarch64") => ("cloudflared-darwin-arm64.tgz", true),
        _ => {
            return Err(CloudflaredUnavailable::UnsupportedTarget {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            });
        }
    };
    Ok(spec)
}

/// Discover the current official Cloudflare asset for this target without downloading it.
///
/// Args:
///     client: Bounded GitHub client aimed at `cloudflare/cloudflared`.
///
/// Returns:
///     Asset metadata including a required SHA-256.
///
/// Errors:
///     Typed unavailability when the platform, checksum, or release metadata is missing.
pub fn resolve_official_cloudflared(
    client: &GitHubReleaseClient,
) -> Result<CloudflaredAsset, CloudflaredUnavailable> {
    let (name, gzip_tar) = official_asset_name()?;
    let (status, _etag, body) = client
        .fetch_release_list_page(
            USER_AGENT,
            1,
            5,
            None,
            now_unix_secs(),
            MAX_RELEASE_RESPONSE_BYTES,
        )
        .map_err(map_discovery)?;
    if status != 200 {
        return Err(CloudflaredUnavailable::Protocol);
    }
    let releases: Vec<GithubRelease> =
        serde_json::from_slice(&body).map_err(|_| CloudflaredUnavailable::Protocol)?;
    let release = releases
        .iter()
        .find(|release| !release.draft && !release.prerelease)
        .ok_or(CloudflaredUnavailable::NoStableRelease)?;
    let checksums = parse_published_checksums(release.body.as_deref().unwrap_or(""));
    let mut matching = release.assets.iter().filter(|asset| asset.name == name);
    let asset = matching
        .next()
        .ok_or(CloudflaredUnavailable::ChecksumMissing { asset: name })?;
    if matching.next().is_some() {
        return Err(CloudflaredUnavailable::Protocol);
    }
    if asset.size == 0 || asset.size > MAX_CLOUDFLARED_BYTES {
        return Err(CloudflaredUnavailable::Protocol);
    }
    validate_cloudflared_download_url(&asset.browser_download_url, &release.tag_name, name)?;
    let body_digest = checksums.get(name).copied();
    let github_digest = asset.digest.as_deref().and_then(parse_sha256_digest);
    let sha256 = match (body_digest, github_digest) {
        (Some(left), Some(right)) if left == right => left,
        (Some(_), Some(_)) => {
            return Err(CloudflaredUnavailable::ChecksumMismatch { asset: name });
        }
        (Some(digest), None) | (None, Some(digest)) => digest,
        (None, None) => {
            return Err(CloudflaredUnavailable::ChecksumMissing { asset: name });
        }
    };
    Ok(CloudflaredAsset {
        tag: release.tag_name.clone(),
        name,
        download_url: asset.browser_download_url.clone(),
        size: asset.size,
        sha256,
        gzip_tar,
    })
}

/// Download and promote a verified `cloudflared` binary to [`paths::cloudflared_executable_path`].
///
/// Existing dest files are reused. Callers must not invoke this from tests or `cargo check`.
///
/// Args:
///     client: Bounded GitHub client aimed at `cloudflare/cloudflared`.
///     asset: Metadata from [`resolve_official_cloudflared`].
///
/// Returns:
///     Path of the promoted executable.
pub fn download_verified_cloudflared(
    client: &GitHubReleaseClient,
    asset: &CloudflaredAsset,
) -> Result<PathBuf, CloudflaredUnavailable> {
    let dest = paths::cloudflared_executable_path();
    if dest.is_file() {
        return Ok(dest);
    }
    if asset.gzip_tar {
        let archive = paths::cloudflared_staging_dir().join(asset.name);
        client
            .stage_verified_asset(
                &asset.download_url,
                &archive,
                asset.size,
                &asset.sha256,
                USER_AGENT,
                MAX_CLOUDFLARED_BYTES,
            )
            .map_err(|_| CloudflaredUnavailable::Transport)?;
        extract_cloudflared_from_tgz(&archive, &dest).map_err(|error| {
            let _ = error;
            CloudflaredUnavailable::ArchiveLayout
        })?;
        let _ = fs::remove_file(&archive);
        set_executable_or_unpromote(&dest)?;
        return Ok(dest);
    }
    client
        .stage_verified_asset(
            &asset.download_url,
            &dest,
            asset.size,
            &asset.sha256,
            USER_AGENT,
            MAX_CLOUDFLARED_BYTES,
        )
        .map_err(|_| CloudflaredUnavailable::Transport)?;
    set_executable_or_unpromote(&dest)?;
    Ok(dest)
}

/// Resolve and, if absent, download the verified `cloudflared` for this target.
///
/// This talks to GitHub and must not run during compile-only validation.
pub fn ensure_verified_cloudflared() -> Result<PathBuf, CloudflaredUnavailable> {
    let dest = paths::cloudflared_executable_path();
    if dest.is_file() {
        return Ok(dest);
    }
    let client = GitHubReleaseClient::for_github_releases(CLOUDFLARED_RELEASES_URL);
    let asset = resolve_official_cloudflared(&client)?;
    download_verified_cloudflared(&client, &asset)
}

/// Construct the production GitHub client for Cloudflare releases.
pub fn cloudflared_github_client() -> GitHubReleaseClient {
    GitHubReleaseClient::for_github_releases(CLOUDFLARED_RELEASES_URL)
}

/// Parse Cloudflare's published `SHA256 Checksums` block from a release body.
///
/// Args:
///     body: GitHub release markdown body.
///
/// Returns:
///     Map of exact asset file name to digest bytes.
pub fn parse_published_checksums(body: &str) -> HashMap<String, [u8; 32]> {
    let mut checksums = HashMap::new();
    for line in body.lines() {
        let line = line.trim().trim_start_matches('`').trim_end_matches('`');
        let Some((name, hex)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let hex = hex.trim();
        if name.is_empty()
            || hex.len() != 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            continue;
        }
        if let Some(digest) = parse_hex_sha256(hex) {
            checksums.insert(name.to_owned(), digest);
        }
    }
    checksums
}

/// Require a Cloudflare GitHub release-download URL for the exact tag and asset.
fn validate_cloudflared_download_url(
    url: &str,
    tag: &str,
    asset: &str,
) -> Result<(), CloudflaredUnavailable> {
    if tag.is_empty()
        || tag.contains('/')
        || tag.contains('\\')
        || tag.contains('\0')
        || tag.contains("..")
    {
        return Err(CloudflaredUnavailable::InvalidUrl);
    }
    let expected = format!("{CLOUDFLARED_DOWNLOAD_PREFIX}{tag}/{asset}");
    if url != expected {
        return Err(CloudflaredUnavailable::InvalidUrl);
    }
    Ok(())
}

/// Parse GitHub's `sha256:` digest field.
fn parse_sha256_digest(value: &str) -> Option<[u8; 32]> {
    parse_hex_sha256(value.strip_prefix("sha256:")?)
}

/// Parse 64 hex characters into a SHA-256 digest.
fn parse_hex_sha256(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    let bytes = hex.as_bytes();
    for (index, slot) in digest.iter_mut().enumerate() {
        *slot = u8::from_str_radix(
            std::str::from_utf8(&bytes[index * 2..index * 2 + 2]).ok()?,
            16,
        )
        .ok()?;
    }
    Some(digest)
}

/// Convert the shared GitHub release-client error into a cloudflared-specific refusal.
fn map_discovery(error: DiscoveryError) -> CloudflaredUnavailable {
    match error.retry() {
        DiscoveryRetry::RateLimited => CloudflaredUnavailable::RateLimited,
        DiscoveryRetry::Transient => CloudflaredUnavailable::Transport,
        DiscoveryRetry::Protocol => CloudflaredUnavailable::Protocol,
    }
}

/// Unpack the `cloudflared` regular file from a verified gzip tar.
fn extract_cloudflared_from_tgz(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let file = File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    let decoder = GzDecoder::new(file);
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow!("cloudflared dest has no parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = dest.with_extension("extract.part");
    extract_named_ustar(decoder, "cloudflared", &tmp)?;
    fs::rename(&tmp, dest).with_context(|| format!("promote extracted {}", dest.display()))?;
    Ok(())
}

/// Read ustar members until `wanted` basename is copied, then stop.
fn extract_named_ustar(mut reader: impl Read, wanted: &str, dest: &Path) -> anyhow::Result<()> {
    let mut header = [0u8; 512];
    loop {
        let mut read = 0;
        while read < 512 {
            let n = reader.read(&mut header[read..])?;
            if n == 0 {
                if read == 0 {
                    bail!("archive ended before {wanted}");
                }
                bail!("truncated tar header");
            }
            read += n;
        }
        if header.iter().all(|byte| *byte == 0) {
            bail!("archive ended before {wanted}");
        }
        let name = ustar_name(&header)?;
        let size = ustar_octal(&header[124..136])?;
        if size > MAX_CLOUDFLARED_BYTES {
            bail!("tar member exceeds the allowed size");
        }
        let typeflag = header[156];
        let basename = Path::new(&name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if (typeflag == 0 || typeflag == b'0') && basename == wanted {
            let mut out = File::create(dest)?;
            copy_exact(&mut reader, &mut out, size)?;
            out.sync_all()?;
            skip_bytes(&mut reader, tar_padding(size))?;
            return Ok(());
        }
        skip_bytes(&mut reader, size.saturating_add(tar_padding(size)))?;
    }
}

/// Bytes needed to pad a tar member to a 512-byte record.
fn tar_padding(size: u64) -> u64 {
    (512 - (size % 512)) % 512
}

/// Copy exactly `size` bytes from `reader` to `writer`.
fn copy_exact(reader: &mut impl Read, writer: &mut impl Write, size: u64) -> anyhow::Result<()> {
    let mut remaining = size;
    let mut buf = [0u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            bail!("truncated tar member");
        }
        writer.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}

/// Discard exactly `size` bytes from `reader`.
fn skip_bytes(reader: &mut impl Read, size: u64) -> io::Result<()> {
    let mut remaining = size;
    let mut buf = [0u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated tar padding",
            ));
        }
        remaining -= n as u64;
    }
    Ok(())
}

/// Read a NUL-terminated ustar name field.
fn ustar_name(header: &[u8; 512]) -> anyhow::Result<String> {
    let raw = header[..100].split(|byte| *byte == 0).next().unwrap_or(&[]);
    String::from_utf8(raw.to_vec()).context("tar name is not UTF-8")
}

/// Parse a tar octal size field.
fn ustar_octal(field: &[u8]) -> anyhow::Result<u64> {
    let text = field
        .iter()
        .copied()
        .take_while(|byte| *byte != 0 && *byte != b' ')
        .map(char::from)
        .collect::<String>();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(&text, 8).context("tar size is not octal")
}

/// Set executable bits, and delete `dest` if that fails after the file was already promoted.
///
/// A leftover non-executable dest would make [`ensure_verified_cloudflared`] treat the cache as
/// ready on the next call.
fn set_executable_or_unpromote(dest: &Path) -> Result<(), CloudflaredUnavailable> {
    match set_executable(dest) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(dest);
            Err(error)
        }
    }
}

/// Mark the installed binary executable on platforms that require permission bits.
fn set_executable(path: &Path) -> Result<(), CloudflaredUnavailable> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|_| CloudflaredUnavailable::Transport)?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).map_err(|_| CloudflaredUnavailable::Transport)?;
    }
    let _ = path;
    Ok(())
}

/// Minimal GitHub release metadata needed to select a verified cloudflared asset.
#[derive(Clone, Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

/// Minimal GitHub asset metadata used for filename, digest, and download validation.
#[derive(Clone, Debug, Deserialize)]
struct GithubAsset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}
