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
