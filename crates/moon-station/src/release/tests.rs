//! Synthetic update-request fixtures; no installed updater or real server is touched.

use super::{STALE_REQUEST, UPDATE_REQUEST, request_update_file};
use moon_tg::UpdateRefusal;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Remove the synthetic directory even when an assertion fails.
struct Fixture(PathBuf);

impl Fixture {
    /// A unique empty data root, isolated from the station's real files.
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "station-update-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    /// Only remove the fixture directory created by this test.
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Removing create_new or flattening refusals loses pending requests or localized recovery.
#[test]
fn update_requests_preserve_pending_and_stale_reasons() {
    let fixture = Fixture::new();
    request_update_file(&fixture.0).unwrap();
    let request = fixture.0.join(UPDATE_REQUEST);
    assert!(request.exists());
    assert_eq!(
        request_update_file(&fixture.0),
        Err(UpdateRefusal::AlreadyRunning)
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(&request)
        .unwrap()
        .set_modified(SystemTime::now() - STALE_REQUEST - std::time::Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        request_update_file(&fixture.0),
        Err(UpdateRefusal::RequestStale)
    );
    assert!(request.exists());
    assert!(matches!(
        request_update_file(&fixture.0.join("missing")),
        Err(UpdateRefusal::WriteFailed(_))
    ));
}
