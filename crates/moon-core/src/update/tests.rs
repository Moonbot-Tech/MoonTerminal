//! Regression tests for stable-release eligibility and verified download bytes.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use super::*;

/// Removing the canonical-form checks would let prerelease-like or ambiguous tags enter numeric
/// ordering and offer an unintended release.
#[test]
fn stable_versions_preserve_legacy_tags_and_accept_canonical_patch_tags() {
    assert_eq!(
        ReleaseVersion::parse("v12.34"),
        Some(ReleaseVersion {
            major: 12,
            minor: 34,
            patch: 0,
        })
    );
    assert_eq!(
        ReleaseVersion::parse("v12.34.5"),
        Some(ReleaseVersion {
            major: 12,
            minor: 34,
            patch: 5,
        })
    );
    assert_eq!(
        BuildIdentity::from_release_base("v0.21").baseline(),
        Some(ReleaseVersion {
            major: 0,
            minor: 21,
            patch: 0,
        })
    );
    for rejected in [
        "12.34",
        "v12",
        "v12.34.0.1",
        "v01.2",
        "v1.02",
        "v1.2.03",
        "v1.2-rc1",
    ] {
        assert_eq!(ReleaseVersion::parse(rejected), None, "accepted {rejected}");
    }
}

/// Making `immutable`, `draft`, or `prerelease` advisory would expose mutable or preview bytes as
/// an automatic update. The fixture digest is the independent SHA-256 of ASCII `abc`.
#[test]
fn eligibility_requires_an_immutable_stable_release_and_exact_asset() {
    let release = fixture_release("v0.22.1");
    let candidate = eligible_release(&release).unwrap().unwrap();
    assert_eq!(
        candidate.version,
        ReleaseVersion {
            major: 0,
            minor: 22,
            patch: 1,
        }
    );
    assert_eq!(candidate.release_tag(), "v0.22.1");
    let legacy_release = fixture_release("v0.21");
    let legacy = eligible_release(&legacy_release).unwrap().unwrap();
    assert_eq!(legacy.release_tag(), "v0.21");
    assert_eq!(legacy.version.patch, 0);
    assert_eq!(
        candidate.asset.sha256,
        [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ]
    );

    let mut mutable = fixture_release("v0.22.1");
    mutable.immutable = false;
    assert_eq!(eligible_release(&mutable).unwrap(), None);
    let mut draft = fixture_release("v0.22.1");
    draft.draft = true;
    assert_eq!(eligible_release(&draft).unwrap(), None);
    let mut prerelease = fixture_release("v0.22.1");
    prerelease.prerelease = true;
    assert_eq!(eligible_release(&prerelease).unwrap(), None);
}

/// Accepting a duplicate exact executable would make asset choice depend on API ordering rather
/// than the release contract.
#[test]
fn eligibility_rejects_duplicate_windows_assets() {
    let mut release = fixture_release("v0.22.1");
    release.assets.push(release.assets[0].clone());
    let error = eligible_release(&release).unwrap_err();
    assert!(error.to_string().contains("duplicate Windows assets"));
}

/// Removing streamed digest enforcement would allow corrupted bytes to reach the executable
/// staging path. This oracle uses the published SHA-256 test vector for `abc`.
#[test]
fn streamed_download_rejects_a_digest_mismatch() {
    let expected =
        parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let mut written = Vec::new();
    copy_verified(&b"abc"[..], &mut written, 3, &expected).unwrap();
    assert_eq!(written, b"abc");

    let mut corrupted = expected;
    corrupted[0] ^= 0xff;
    let error = copy_verified(&b"abc"[..], Vec::new(), 3, &corrupted).unwrap_err();
    assert!(error.to_string().contains("SHA-256"));
    assert!(copy_verified(&b"abc"[..], Vec::new(), 4, &expected).is_err());
    assert!(copy_verified(&b"abc"[..], Vec::new(), 2, &expected).is_err());
}

/// Trusting a URL from another repository or tag would disconnect the selected version from the
/// bytes that are downloaded.
#[test]
fn asset_url_is_bound_to_the_repository_tag_and_exact_name() {
    assert!(validate_download_url(
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.21/MoonTerminal.exe",
        "v0.21"
    )
    .is_ok());
    assert!(validate_download_url(
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.24.1/MoonTerminal.exe",
        "v0.24.1"
    )
    .is_ok());
    for rejected in [
        "https://github.com/other/MoonTerminal/releases/download/v0.24.1/MoonTerminal.exe",
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.24.0/MoonTerminal.exe",
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.24.1/other.exe",
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.24.1/EvilMoonTerminal.exe",
    ] {
        assert!(
            validate_download_url(rejected, "v0.24.1").is_err(),
            "accepted {rejected}"
        );
    }
}

/// Comparing only major/minor inside `greatest_eligible` would keep `v0.24.0` ahead of
/// `v0.24.1`, so users would never see the available patch update.
#[test]
fn reducer_selects_the_greatest_strictly_newer_release() {
    let baseline = ReleaseVersion {
        major: 0,
        minor: 21,
        patch: 0,
    };
    let mut tags_by_version = BTreeMap::new();
    let selected = greatest_eligible(
        &[
            fixture_release("v0.21"),
            fixture_release("v0.24.0"),
            fixture_release("v0.24.1"),
            fixture_release("v0.22.7"),
            fixture_release("v0.20"),
        ],
        baseline,
        None,
        &mut tags_by_version,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        selected.version(),
        ReleaseVersion {
            major: 0,
            minor: 24,
            patch: 1,
        }
    );
    assert!(greatest_eligible(
        &[fixture_release("v0.21")],
        baseline,
        None,
        &mut BTreeMap::new(),
    )
    .unwrap()
    .is_none());
}

/// Treating two spellings of one numeric version as independent candidates would make asset
/// selection depend on GitHub API ordering and could bind the manifest to an arbitrary alias.
#[test]
fn reducer_rejects_distinct_tags_for_the_same_newer_version() {
    let baseline = ReleaseVersion {
        major: 0,
        minor: 21,
        patch: 0,
    };
    let error = greatest_eligible(
        &[fixture_release("v0.24"), fixture_release("v0.24.0")],
        baseline,
        None,
        &mut BTreeMap::new(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("ambiguous tags"));
}

/// Checking aliases only against the selected maximum would miss ambiguous lower releases and
/// let their exact asset identity depend on API ordering across pages.
#[test]
fn reducer_rejects_an_alias_below_the_selected_maximum() {
    let baseline = ReleaseVersion {
        major: 0,
        minor: 20,
        patch: 0,
    };
    let mut tags_by_version = BTreeMap::new();
    let greatest = greatest_eligible(
        &[fixture_release("v0.25.0"), fixture_release("v0.24")],
        baseline,
        None,
        &mut tags_by_version,
    )
    .unwrap();
    let error = greatest_eligible(
        &[fixture_release("v0.24.0")],
        baseline,
        greatest,
        &mut tags_by_version,
    )
    .unwrap_err();
    assert!(error.to_string().contains("ambiguous tags"));
}

/// Stopping after a page-one `304` would leave a release published later on page two invisible
/// until restart. Recorded request headers are independent HTTP wire oracles for each page ETag.
#[test]
fn recurring_discovery_revalidates_each_due_page_and_finds_a_later_release() {
    let page_one = repeated_release_page("v0.21", RELEASES_PER_PAGE);
    let current_page_two = release_page_json(&["v0.21"]);
    let available_page_two = release_page_json(&["v0.24.1"]);
    let script = vec![
        http_response(200, &[("ETag", "\"page-1-a\"")], &page_one),
        http_response(200, &[("ETag", "\"page-2-a\"")], &current_page_two),
        http_response(304, &[("ETag", "\"page-1-a\"")], ""),
        http_response(304, &[("ETag", "\"page-1-a\"")], ""),
        http_response(200, &[("ETag", "\"page-2-b\"")], &available_page_two),
    ];
    let server = ScriptedServer::start(script);
    let client = GitHubReleaseClient::for_test(&server.url);
    let mut discovery =
        ReleaseDiscovery::with_client(BuildIdentity::from_release_base("v0.21"), client);

    assert_eq!(
        discovery.scan_at(1_000).unwrap().eligibility,
        UpdateEligibility::Current
    );
    assert_eq!(
        discovery.scan_at(2_000).unwrap().eligibility,
        UpdateEligibility::Current
    );
    let result = discovery.scan_at(90_000).unwrap();
    let UpdateEligibility::Available(candidate) = result.eligibility else {
        panic!("page-two publication was not discovered");
    };
    assert_eq!(candidate.release_tag(), "v0.24.1");

    let requests = server.finish();
    assert_eq!(requests.len(), 5);
    assert!(requests[0].contains("per_page=100&page=1"));
    assert!(requests[0].contains("x-github-api-version: 2022-11-28"));
    assert!(requests[1].contains("per_page=100&page=2"));
    assert!(requests[2].contains("if-none-match: \"page-1-a\""));
    assert!(requests[3].contains("if-none-match: \"page-1-a\""));
    assert!(requests[4].contains("if-none-match: \"page-2-a\""));
}

/// Committing page one's new ETag before page two succeeds would turn a partial scan into the
/// trusted cache. The next recorded request must therefore retain the old independent validator.
#[test]
fn a_partial_scan_does_not_commit_any_page_cache() {
    let current = repeated_release_page("v0.21", RELEASES_PER_PAGE);
    let changed = repeated_release_page("v0.24.1", RELEASES_PER_PAGE);
    let script = vec![
        http_response(200, &[("ETag", "\"page-1-a\"")], &current),
        http_response(
            200,
            &[("ETag", "\"page-2-a\"")],
            &release_page_json(&["v0.21"]),
        ),
        http_response(200, &[("ETag", "\"page-1-b\"")], &changed),
        http_response(500, &[], ""),
        http_response(500, &[], ""),
    ];
    let server = ScriptedServer::start(script);
    let mut discovery = ReleaseDiscovery::with_client(
        BuildIdentity::from_release_base("v0.21"),
        GitHubReleaseClient::for_test(&server.url),
    );

    discovery.scan_at(1_000).unwrap();
    assert_eq!(
        discovery.scan_at(2_000).unwrap_err().retry(),
        DiscoveryRetry::Transient
    );
    assert!(discovery.scan_at(3_000).is_err());

    let requests = server.finish();
    assert!(requests[2].contains("if-none-match: \"page-1-a\""));
    assert!(requests[4].contains("if-none-match: \"page-1-a\""));
    assert!(!requests[4].contains("page-1-b"));
}

/// Treating `304` as an empty or current page without a representation would permit incomplete
/// metadata to authorize a UI decision instead of failing closed.
#[test]
fn not_modified_without_a_cached_representation_fails_closed() {
    let server = ScriptedServer::start(vec![http_response(304, &[], "")]);
    let mut discovery = ReleaseDiscovery::with_client(
        BuildIdentity::from_release_base("v0.21"),
        GitHubReleaseClient::for_test(&server.url),
    );
    let error = discovery.scan_at(1_000).unwrap_err();
    assert_eq!(error.retry(), DiscoveryRetry::Protocol);
    assert!(error
        .to_string()
        .contains("without a matching cached validator"));
    assert_eq!(server.finish().len(), 1);
}

/// Reusing an unconditional cached body after an unsolicited `304` would treat stale metadata as
/// revalidated even though the preceding `200` supplied no validator to send back.
#[test]
fn not_modified_without_a_sent_validator_fails_closed() {
    let server = ScriptedServer::start(vec![
        http_response(200, &[], &release_page_json(&["v0.21"])),
        http_response(304, &[], ""),
    ]);
    let mut discovery = ReleaseDiscovery::with_client(
        BuildIdentity::from_release_base("v0.21"),
        GitHubReleaseClient::for_test(&server.url),
    );

    discovery.scan_at(1_000).unwrap();
    let error = discovery.scan_at(2_000).unwrap_err();
    assert_eq!(error.retry(), DiscoveryRetry::Protocol);
    assert_eq!(server.finish().len(), 2);
}

/// Ignoring GitHub's relative and exhausted-primary deadlines would allow the background loop to
/// retry before the server-authorized time. Literal headers and a fixed epoch are the oracle.
#[test]
fn rate_headers_preserve_the_latest_valid_server_deadline() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("120"));
    headers.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
    headers.insert("x-ratelimit-reset", HeaderValue::from_static("1300"));
    let rate = RateHeaders::from_headers(&headers, 1_000);
    assert_eq!(rate.rejected_not_before(), Some(1_300));
    assert!(rate.low_remaining());
    assert_eq!(rate.success_defer_until(1_000), Some(1_300));

    headers.insert("retry-after", HeaderValue::from_static("invalid"));
    headers.insert("x-ratelimit-reset", HeaderValue::from_static("999"));
    let invalid = RateHeaders::from_headers(&headers, 1_000);
    assert_eq!(invalid.rejected_not_before(), None);
}

/// Fetching a required second page with five requests left would spend the shared-IP reserve;
/// the first literal response must instead yield one typed reset-aware deferral and no page two.
#[test]
fn a_low_remaining_budget_defers_before_a_required_second_page() {
    let page_one = repeated_release_page("v0.21", RELEASES_PER_PAGE);
    let server = ScriptedServer::start(vec![http_response(
        200,
        &[
            ("ETag", "\"page-1-a\""),
            ("X-RateLimit-Remaining", "5"),
            ("X-RateLimit-Reset", "1300"),
        ],
        &page_one,
    )]);
    let mut discovery = ReleaseDiscovery::with_client(
        BuildIdentity::from_release_base("v0.21"),
        GitHubReleaseClient::for_test(&server.url),
    );
    let error = discovery.scan_at(1_000).unwrap_err();
    assert_eq!(error.retry(), DiscoveryRetry::RateLimited);
    assert_eq!(error.not_before_unix(), Some(1_300));
    assert_eq!(server.finish().len(), 1);
}

/// Treating `429` like a generic network error would shorten an explicit GitHub retry/reset
/// deadline. The later of the two literal server authorities must be retained.
#[test]
fn rate_limited_http_responses_keep_the_later_server_deadline() {
    let server = ScriptedServer::start(vec![http_response(
        429,
        &[
            ("Retry-After", "120"),
            ("X-RateLimit-Remaining", "0"),
            ("X-RateLimit-Reset", "1300"),
        ],
        "",
    )]);
    let mut discovery = ReleaseDiscovery::with_client(
        BuildIdentity::from_release_base("v0.21"),
        GitHubReleaseClient::for_test(&server.url),
    );
    let error = discovery.scan_at(1_000).unwrap_err();
    assert_eq!(error.retry(), DiscoveryRetry::RateLimited);
    assert_eq!(error.not_before_unix(), Some(1_300));
    assert_eq!(server.finish().len(), 1);
}

/// Dropping `Retry-After` from a temporary server failure would let the local 30-minute retry
/// fire before GitHub's explicit recovery deadline.
#[test]
fn transient_http_responses_keep_the_server_deadline() {
    let server = ScriptedServer::start(vec![http_response(500, &[("Retry-After", "3600")], "")]);
    let mut discovery = ReleaseDiscovery::with_client(
        BuildIdentity::from_release_base("v0.21"),
        GitHubReleaseClient::for_test(&server.url),
    );

    let error = discovery.scan_at(1_000).unwrap_err();
    assert_eq!(error.retry(), DiscoveryRetry::Transient);
    assert_eq!(error.not_before_unix(), Some(4_600));
    assert_eq!(server.finish().len(), 1);
}

/// Disabling the production HTTPS-only client would let a release URL cross the transport trust
/// boundary before immutable metadata is checked.
#[test]
fn production_release_client_rejects_plain_http_before_connecting() {
    let error = GitHubReleaseClient::new()
        .agent
        .get("http://127.0.0.1:9/releases")
        .call()
        .unwrap_err();
    assert!(matches!(error, ureq::Error::RequireHttpsOnly(_)));
}

/// A failed post-sync verification must remove its part file, while success promotes only the
/// independently rehashed bytes from the still-exclusive open handle.
#[test]
fn part_guard_cleans_failures_and_promotion_keeps_verified_bytes() {
    let root = test_root("part-promotion");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let staged = root.join("MoonTerminal.exe");
    let expected =
        parse_sha256("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let (path, mut file) = create_unique_part(&staged).unwrap();
    file.write_all(b"bad").unwrap();
    let mut part = PartFile::new(path.clone(), file);
    let part_path = part.path().to_path_buf();
    assert!(promote_verified_part(&part_path, part.file_mut(), &staged, 3, &expected).is_err());
    drop(part);
    assert!(!path.exists());
    assert!(!staged.exists());

    let (path, mut file) = create_unique_part(&staged).unwrap();
    file.write_all(b"abc").unwrap();
    let mut part = PartFile::new(path, file);
    let part_path = part.path().to_path_buf();
    promote_verified_part(&part_path, part.file_mut(), &staged, 3, &expected).unwrap();
    part.disarm();
    assert_eq!(fs::read(&staged).unwrap(), b"abc");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

/// Loopback HTTP script retaining every request as a wire-level test oracle.
struct ScriptedServer {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    worker: thread::JoinHandle<()>,
}

impl ScriptedServer {
    /// Start one loopback server that consumes exactly the supplied response script.
    fn start(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let worker = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 1024];
                while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                }
                recorded
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(bytes).unwrap().to_ascii_lowercase());
                stream.write_all(response.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });
        Self {
            url: format!("http://{address}/releases"),
            requests,
            worker,
        }
    }

    /// Join the script and return every recorded request in arrival order.
    fn finish(self) -> Vec<String> {
        self.worker.join().unwrap();
        Arc::try_unwrap(self.requests)
            .unwrap()
            .into_inner()
            .unwrap()
    }
}

/// Build one complete HTTP/1.1 response with an exact body length.
fn http_response(status: u16, headers: &[(&str, &str)], body: &str) -> String {
    let reason = match status {
        200 => "OK",
        304 => "Not Modified",
        403 => "Forbidden",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Test Status",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    response.push_str(body);
    response
}

/// Build a JSON release-list page from exact stable tags.
fn release_page_json(tags: &[&str]) -> String {
    format!(
        "[{}]",
        tags.iter()
            .map(|tag| release_json(tag))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Build a full JSON page by repeating one independently valid release fixture.
fn repeated_release_page(tag: &str, count: usize) -> String {
    format!(
        "[{}]",
        std::iter::repeat_with(|| release_json(tag))
            .take(count)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Serialize the minimal immutable GitHub release fixture used by the loopback server.
fn release_json(tag: &str) -> String {
    format!(
        r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"immutable":true,"assets":[{{"name":"MoonTerminal.exe","size":3,"digest":"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","browser_download_url":"https://github.com/Moonbot-Tech/MoonTerminal/releases/download/{tag}/MoonTerminal.exe"}}]}}"#
    )
}

/// Build an independent immutable-release response fixture for eligibility tests.
fn fixture_release(tag: &str) -> GitHubRelease {
    GitHubRelease {
        tag_name: tag.to_owned(),
        draft: false,
        prerelease: false,
        immutable: true,
        assets: vec![GitHubAsset {
            name: "MoonTerminal.exe".to_owned(),
            size: 3,
            digest: Some(
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                    .to_owned(),
            ),
            browser_download_url: format!(
                "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/{tag}/MoonTerminal.exe"
            ),
        }],
    }
}

/// Allocate a process-local temporary directory outside every production path constructor.
fn test_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "moonterminal-update-{label}-{}",
        std::process::id()
    ))
}
