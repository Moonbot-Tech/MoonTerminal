//! Regression tests for stable-release eligibility and verified download bytes.

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
    let candidate = eligible_release(release).unwrap().unwrap();
    assert_eq!(
        candidate.version,
        ReleaseVersion {
            major: 0,
            minor: 22,
            patch: 1,
        }
    );
    assert_eq!(candidate.release_tag(), "v0.22.1");
    let legacy = eligible_release(fixture_release("v0.21")).unwrap().unwrap();
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
    assert_eq!(eligible_release(mutable).unwrap(), None);
    let mut draft = fixture_release("v0.22.1");
    draft.draft = true;
    assert_eq!(eligible_release(draft).unwrap(), None);
    let mut prerelease = fixture_release("v0.22.1");
    prerelease.prerelease = true;
    assert_eq!(eligible_release(prerelease).unwrap(), None);
}

/// Accepting a duplicate exact executable would make asset choice depend on API ordering rather
/// than the release contract.
#[test]
fn eligibility_rejects_duplicate_windows_assets() {
    let mut release = fixture_release("v0.22.1");
    release.assets.push(release.assets[0].clone());
    let error = eligible_release(release).unwrap_err();
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
    assert!(
        validate_download_url(
            "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.21/MoonTerminal.exe",
            "v0.21"
        )
        .is_ok()
    );
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
        vec![
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
    assert!(
        greatest_eligible(
            vec![fixture_release("v0.21")],
            baseline,
            None,
            &mut BTreeMap::new(),
        )
        .unwrap()
        .is_none()
    );
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
        vec![fixture_release("v0.24"), fixture_release("v0.24.0")],
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
        vec![fixture_release("v0.25.0"), fixture_release("v0.24")],
        baseline,
        None,
        &mut tags_by_version,
    )
    .unwrap();
    let error = greatest_eligible(
        vec![fixture_release("v0.24.0")],
        baseline,
        greatest,
        &mut tags_by_version,
    )
    .unwrap_err();
    assert!(error.to_string().contains("ambiguous tags"));
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
