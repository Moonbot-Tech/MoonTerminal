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

/// A slow link that keeps delivering parts is never cut by a clock over the whole transfer
/// (#1419: the move stopped a download still receiving parts at 180s).
#[test]
fn a_transfer_still_receiving_parts_keeps_going() {
    let start = Instant::now();
    let mut progress = Progress::new(start);
    let mut received = 0;
    for minute in 1..=20 {
        received += 128 * 1024;
        let now = start + Duration::from_secs(minute * 60);
        assert!(
            progress.check(received, now).is_ok(),
            "cut at minute {minute}"
        );
    }
}

/// A transfer that stops receiving parts is given up after the library's own five minutes,
/// counted from the last part, not from the start.
#[test]
fn a_stalled_transfer_ends_after_five_minutes_without_a_part() {
    let start = Instant::now();
    let mut progress = Progress::new(start);
    // A part every two seconds for four minutes, then nothing.
    let mut received = 0;
    for second in (2..=240).step_by(2) {
        received += 128 * 1024;
        let now = start + Duration::from_secs(second);
        assert!(progress.check(received, now).is_ok(), "cut at {second}s");
    }
    let last_part = start + Duration::from_secs(240);
    for second in (2..300).step_by(2) {
        let now = last_part + Duration::from_secs(second);
        assert!(
            progress.check(received, now).is_ok(),
            "cut {second}s after the last part"
        );
    }
    let error = progress
        .check(received, last_part + Duration::from_secs(300))
        .unwrap_err()
        .to_string();
    assert!(error.contains("no new part"), "{error}");
    assert!(error.contains(&format!("{received} bytes")), "{error}");
}

/// A core that never sends a first part is still told apart within a minute.
#[test]
fn no_first_part_ends_after_a_minute() {
    let start = Instant::now();
    let mut progress = Progress::new(start);
    assert!(progress.check(0, start + Duration::from_secs(59)).is_ok());
    let error = progress
        .check(0, start + Duration::from_secs(60))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("no part of its settings snapshot"),
        "{error}"
    );
}
