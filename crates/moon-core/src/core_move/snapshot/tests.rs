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

/// A fake transfer that grows on disk every poll: the first part is reported at once, then at
/// most once a second and only when the count moved — never on every 200 ms poll (#1419: the
/// settings row showed nothing until the download ended).
#[test]
fn received_bytes_are_reported_once_a_second_while_they_grow() {
    let dir = std::env::temp_dir().join(format!("moon-core-move-report-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("transfer.zip");
    std::fs::write(&file, b"").unwrap();

    let start = Instant::now();
    let mut progress = Progress::new(start);
    let mut reports = Vec::new();
    // Ten seconds of polls; a 1 KiB part lands every poll for the first five seconds only.
    for poll in 1..=50u64 {
        if poll <= 25 {
            let mut bytes = std::fs::read(&file).unwrap();
            bytes.extend_from_slice(&[0u8; 1024]);
            std::fs::write(&file, &bytes).unwrap();
        }
        let now = start + POLL * poll as u32;
        let on_disk = std::fs::metadata(&file).unwrap().len();
        progress.check(on_disk, now).unwrap();
        if let Some(bytes) = progress.report(now) {
            reports.push((poll, bytes));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);

    // Poll 1 (0.2 s) at once, then every fifth poll (1 s) while it grows; the last growth
    // (poll 25) a second after the report before it, then nothing once the count stands still.
    assert_eq!(
        reports,
        vec![
            (1, 1024),
            (6, 6 * 1024),
            (11, 11 * 1024),
            (16, 16 * 1024),
            (21, 21 * 1024),
            (26, 25 * 1024),
        ],
    );
    // A growth inside the second is held, not dropped: it goes out once the second is up.
    let mut tail = Progress::new(start);
    tail.check(1024, start).unwrap();
    assert_eq!(tail.report(start), Some(1024));
    tail.check(2048, start + Duration::from_millis(400))
        .unwrap();
    assert_eq!(tail.report(start + Duration::from_millis(400)), None);
    assert_eq!(tail.report(start + Duration::from_secs(1)), Some(2048));
}

/// Nothing is reported before the first part: a core that never answers shows no count of zero.
#[test]
fn nothing_is_reported_before_the_first_part() {
    let start = Instant::now();
    let mut progress = Progress::new(start);
    for second in 1..59 {
        let now = start + Duration::from_secs(second);
        progress.check(0, now).unwrap();
        assert_eq!(progress.report(now), None);
    }
}

/// A transfer failure says the reason alone and stays recognisable through the context a caller
/// adds, so the command line can add the ZIP pointer while the window, which cannot take a ZIP,
/// does not tell its user to pass one (#1419).
#[test]
fn a_transfer_failure_is_marked_and_carries_no_zip_pointer() {
    use anyhow::Context as _;
    let start = Instant::now();
    let error = Progress::new(start)
        .check(0, start + FIRST_PART)
        .context("take the settings snapshot from 203.0.113.5:4510")
        .unwrap_err();
    assert!(is_transfer_failure(&error), "{error:#}");
    let text = format!("{error:#}");
    assert!(!text.contains("ZIP"), "{text}");
    assert!(!text.contains("Get settings"), "{text}");

    let other = anyhow::anyhow!("connect: timed out").context("take the settings snapshot");
    assert!(!is_transfer_failure(&other));
}
