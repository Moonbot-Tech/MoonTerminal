//! Regression tests for immutable release eligibility policy.

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
    let candidate = eligible_exe(&release).unwrap().unwrap();
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
    let legacy = eligible_exe(&legacy_release).unwrap().unwrap();
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
    assert_eq!(eligible_exe(&mutable).unwrap(), None);
    let mut draft = fixture_release("v0.22.1");
    draft.draft = true;
    assert_eq!(eligible_exe(&draft).unwrap(), None);
    let mut prerelease = fixture_release("v0.22.1");
    prerelease.prerelease = true;
    assert_eq!(eligible_exe(&prerelease).unwrap(), None);
}

/// Removing either side of the absolute executable-size range would admit an empty or oversized
/// asset, while an off-by-one comparison would reject the maximum valid release.
#[test]
fn eligibility_enforces_every_executable_size_boundary() {
    for (size, expected_eligible) in [
        (0, false),
        (1, true),
        (MAX_EXECUTABLE_BYTES, true),
        (MAX_EXECUTABLE_BYTES + 1, false),
    ] {
        let mut release = fixture_release("v0.22.1");
        release.assets[0].size = size;
        assert_eq!(
            eligible_exe(&release).unwrap().is_some(),
            expected_eligible,
            "unexpected eligibility for executable size {size}"
        );
    }
}

/// Accepting a duplicate exact executable would make asset choice depend on API ordering rather
/// than the release contract.
#[test]
fn eligibility_rejects_duplicate_windows_assets() {
    let mut release = fixture_release("v0.22.1");
    release.assets.push(release.assets[0].clone());
    let error = eligible_exe(&release).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("duplicate MoonTerminal.exe assets")
    );
}

/// Trusting a URL from another repository or tag would disconnect the selected version from the
/// bytes that are downloaded.
#[test]
fn asset_url_is_bound_to_the_repository_tag_and_exact_name() {
    assert!(
        validate_download_url(
            "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.21/MoonTerminal.exe",
            "v0.21",
            WINDOWS_ASSET_NAME
        )
        .is_ok()
    );
    assert!(validate_download_url(
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.24.1/MoonTerminal.exe",
        "v0.24.1",
        WINDOWS_ASSET_NAME
    )
    .is_ok());
    for rejected in [
        "https://github.com/other/MoonTerminal/releases/download/v0.24.1/MoonTerminal.exe",
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.24.0/MoonTerminal.exe",
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.24.1/other.exe",
        "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.24.1/EvilMoonTerminal.exe",
    ] {
        assert!(
            validate_download_url(rejected, "v0.24.1", WINDOWS_ASSET_NAME).is_err(),
            "accepted {rejected}"
        );
    }
}

/// The asset choice is a plain function of the target OS, so this Windows-run suite also pins
/// what a macOS build selects; swapping or collapsing the two names would offer macOS users an
/// executable they cannot run, or Windows users a disk image.
#[test]
fn asset_name_is_chosen_per_target_os() {
    assert_eq!(asset_name_for_os("macos"), "MoonTerminal.dmg");
    assert_eq!(asset_name_for_os("windows"), "MoonTerminal.exe");
    assert_eq!(
        platform_asset_name(),
        asset_name_for_os(std::env::consts::OS)
    );
}

/// The station's asset is named by the CPU the release builds it for (STATION.md §6): a new
/// architecture, or a name the release does not publish, must find nothing rather than a binary
/// for another CPU.
#[test]
fn station_asset_name_is_chosen_per_target_arch() {
    assert_eq!(
        station_asset_name_for_arch("x86_64"),
        Some("moon-station-x86_64")
    );
    assert_eq!(
        station_asset_name_for_arch("aarch64"),
        Some("moon-station-aarch64")
    );
    assert_eq!(station_asset_name_for_arch("riscv64"), None);
    assert_eq!(
        station_asset_name(),
        station_asset_name_for_arch(std::env::consts::ARCH)
    );
}

/// A macOS build must be offered only the `.dmg`, validated as strictly as the executable: a
/// release carrying just the Windows asset is not an update for it, and vice versa.
#[test]
fn eligibility_offers_only_the_platform_asset() {
    let exe_only = fixture_release("v0.22.1");
    assert_eq!(
        eligible_release_for(&exe_only, MACOS_ASSET_NAME).unwrap(),
        None
    );

    let dmg_only = fixture_release_with("v0.22.1", MACOS_ASSET_NAME);
    assert_eq!(eligible_exe(&dmg_only).unwrap(), None);
    let dmg = eligible_release_for(&dmg_only, MACOS_ASSET_NAME)
        .unwrap()
        .unwrap();
    assert_eq!(dmg.asset_name(), "MoonTerminal.dmg");
    assert!(dmg.download_url().ends_with("/v0.22.1/MoonTerminal.dmg"));

    let mut both = fixture_release("v0.22.1");
    both.assets
        .extend(fixture_release_with("v0.22.1", MACOS_ASSET_NAME).assets);
    assert_eq!(
        eligible_release_for(&both, MACOS_ASSET_NAME)
            .unwrap()
            .unwrap()
            .asset_name(),
        "MoonTerminal.dmg"
    );
    assert_eq!(
        eligible_exe(&both).unwrap().unwrap().asset_name(),
        "MoonTerminal.exe"
    );

    let mut undigested = fixture_release_with("v0.22.1", MACOS_ASSET_NAME);
    undigested.assets[0].digest = None;
    assert_eq!(
        eligible_release_for(&undigested, MACOS_ASSET_NAME).unwrap(),
        None
    );
    let mut foreign = fixture_release_with("v0.22.1", MACOS_ASSET_NAME);
    foreign.assets[0].browser_download_url =
        "https://github.com/other/MoonTerminal/releases/download/v0.22.1/MoonTerminal.dmg"
            .to_owned();
    assert!(eligible_release_for(&foreign, MACOS_ASSET_NAME).is_err());
    assert!(
        validate_download_url(
            "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/v0.22.1/MoonTerminal.exe",
            "v0.22.1",
            MACOS_ASSET_NAME
        )
        .is_err()
    );
}

/// Select the Windows executable regardless of the host the suite runs on.
fn eligible_exe(release: &GitHubRelease) -> anyhow::Result<Option<AvailableRelease>> {
    eligible_release_for(release, WINDOWS_ASSET_NAME)
}

/// Build an independent immutable-release response fixture for eligibility tests.
fn fixture_release(tag: &str) -> GitHubRelease {
    fixture_release_with(tag, WINDOWS_ASSET_NAME)
}

/// Build an immutable-release fixture carrying one asset with the given exact name.
fn fixture_release_with(tag: &str, asset_name: &str) -> GitHubRelease {
    GitHubRelease {
        tag_name: tag.to_owned(),
        draft: false,
        prerelease: false,
        immutable: true,
        assets: vec![GitHubAsset {
            name: asset_name.to_owned(),
            size: 3,
            digest: Some(
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                    .to_owned(),
            ),
            browser_download_url: format!(
                "https://github.com/Moonbot-Tech/MoonTerminal/releases/download/{tag}/{asset_name}"
            ),
        }],
    }
}
