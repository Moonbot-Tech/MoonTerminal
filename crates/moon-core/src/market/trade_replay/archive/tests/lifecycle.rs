use super::*;

/// Resolved unlatched failures and expired cooldowns must release their process-global path,
/// while a definitive missing day stays latched instead of being downloaded repeatedly.
#[test]
fn day_failure_entries_leave_on_success_and_expiry() {
    let root = std::env::temp_dir().join(format!("archive-lifecycle-{}", std::process::id()));
    let success = root.join("success");
    let failure = Err(ArchiveError::Transient("synthetic".into()));
    note_day_result(success.clone(), &failure, 1, Some(u64::MAX), Instant::now());
    note_day_result(
        success.clone(),
        &Ok(Coverage::none()),
        1,
        Some(u64::MAX),
        Instant::now(),
    );
    assert!(
        DAY_FAILURES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .days
            .contains_key(&success)
    );
    assert!(
        day_refusal(
            &success,
            &|| Some(u64::MAX),
            Instant::now() + Duration::from_secs(86_400)
        )
        .is_none()
    );
    assert!(
        !DAY_FAILURES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .days
            .contains_key(&success)
    );

    let expired = root.join("expired");
    let now = Instant::now();
    for _ in 0..3 {
        note_day_result(expired.clone(), &failure, 1, Some(u64::MAX), now);
    }
    assert!(
        day_refusal(
            &expired,
            &|| Some(u64::MAX),
            now + Duration::from_secs(86_400)
        )
        .is_none()
    );
    assert!(
        !DAY_FAILURES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .days
            .contains_key(&expired)
    );

    let missing = root.join("missing");
    note_day_result(
        missing.clone(),
        &Err(ArchiveError::UnknownSymbol),
        0,
        Some(u64::MAX),
        now,
    );
    assert_eq!(
        day_refusal(
            &missing,
            &|| Some(u64::MAX),
            now + Duration::from_secs(86_400)
        ),
        Some(ArchiveError::UnknownSymbol)
    );
}

/// The final same-day lock user must remove the map entry without unlocking earlier users.
#[test]
fn day_lock_leaves_with_its_last_user() {
    let path = std::env::temp_dir().join(format!("archive-lock-{}", std::process::id()));
    let first = day_cache::in_flight(&path);
    let second = day_cache::in_flight(&path);
    assert!(day_cache::contains_key(&path));
    drop(first);
    assert!(day_cache::contains_key(&path));
    drop(second);
    assert!(!day_cache::contains_key(&path));
}

/// Concurrent final users must all finish before the single map entry disappears, without a
/// strong-count race leaving an unused archive path behind.
#[test]
fn day_lock_leaves_after_concurrent_users() {
    let path = std::env::temp_dir().join(format!("archive-lock-concurrent-{}", std::process::id()));
    std::thread::scope(|scope| {
        for _ in 0..16 {
            scope.spawn(|| drop(day_cache::in_flight(&path)));
        }
    });
    assert!(!day_cache::contains_key(&path));
}

/// Releasing a disk latch must preserve the failed-body allowance exactly as transfer recovery
/// does; resetting it would let a repeatedly damaged archive bypass the three-attempt limit.
#[test]
fn disk_recovery_preserves_failed_attempts() {
    let path = std::env::temp_dir().join(format!("archive-disk-attempts-{}", std::process::id()));
    let now = Instant::now();
    let failure = Err(ArchiveError::Transient("synthetic".into()));
    for _ in 0..2 {
        note_day_result(path.clone(), &failure, 1, Some(1), now);
    }
    note_day_result(path.clone(), &Err(ArchiveError::NoRoom), 1, Some(1), now);
    assert!(day_refusal(&path, &|| Some(2), now).is_none());
    note_day_result(path.clone(), &failure, 1, Some(2), now);
    let book = DAY_FAILURES.get_or_init(Mutex::default).lock().unwrap();
    let day = book.days.get(&path).unwrap();
    assert_eq!(day.attempts, 3);
    assert!(matches!(
        day.latched.as_ref(),
        Some(DayLatch::Transfer { .. })
    ));
}
