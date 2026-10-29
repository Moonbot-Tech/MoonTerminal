use super::*;

/// Measure resolved archive-day bookkeeping and report retained process-global entries.
#[test]
#[ignore = "synthetic tape scale baseline"]
fn tape_scale_archive_result_cleanup() {
    let started = std::time::Instant::now();
    for index in 0..20_000_i64 {
        let path = std::env::temp_dir().join(format!("synthetic-archive-day-{index}"));
        let failure = Err(ArchiveError::Transient("synthetic".into()));
        note_day_result(path.clone(), &failure, 1, Some(u64::MAX), Instant::now());
        note_day_result(
            path,
            &Ok(Coverage::none()),
            1,
            Some(u64::MAX),
            Instant::now(),
        );
    }
    let expiry = Instant::now() + Duration::from_secs(86_400);
    for index in 0..20_000_i64 {
        let path = std::env::temp_dir().join(format!("synthetic-archive-day-{index}"));
        assert!(day_refusal(&path, &|| Some(u64::MAX), expiry).is_none());
    }
    let retained = DAY_FAILURES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .days
        .len();
    println!(
        "[tape_scale] archive_result_cleanup days=20000 retained={retained} elapsed_ms={:.3}",
        started.elapsed().as_secs_f64() * 1_000.0
    );
}
