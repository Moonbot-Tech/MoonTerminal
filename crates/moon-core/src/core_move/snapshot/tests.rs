use super::*;

/// A leftover of a killed run is removed; a live run's target and anything else in the folder are
/// not — deleting a fresh target would break another move running on this machine.
#[test]
fn only_old_targets_are_removed() {
    let dir = std::env::temp_dir().join(format!("moon-core-move-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join(format!("{TEMP_PREFIX}1.zip"));
    let fresh = dir.join(format!("{TEMP_PREFIX}2.zip"));
    let other = dir.join("unrelated.zip");
    for path in [&old, &fresh, &other] {
        std::fs::write(path, b"x").unwrap();
    }
    let long_ago = std::time::SystemTime::now() - STALE - Duration::from_secs(60);
    for path in [&old, &other] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
    }

    remove_stale(&dir);

    assert!(!old.exists());
    assert!(fresh.exists());
    assert!(other.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
