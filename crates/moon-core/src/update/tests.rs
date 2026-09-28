//! Regression tests for the shared GitHub release-client facade.

use super::*;

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

/// The saved image carries its release version, so an older image left in Downloads is never
/// reopened as the newer update, and it carries the `.dmg` extension macOS `open` mounts.
#[test]
fn installer_image_is_named_per_release_version() {
    let version = ReleaseVersion {
        major: 0,
        minor: 24,
        patch: 1,
    };
    assert_eq!(
        installer_image_file_name(version),
        "MoonTerminal-0.24.1.dmg"
    );
    let path = installer_image_download_path(version).unwrap();
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("MoonTerminal-0.24.1.dmg")
    );
}
