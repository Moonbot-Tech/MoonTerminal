use super::*;

/// A permanently busy machine must still admit one recovery operation every thirty seconds.
#[test]
fn pacing_wait_gives_way_after_the_max_pause() {
    let elapsed = std::cell::Cell::new(Duration::ZERO);
    assert!(wait_with(
        false,
        || 100.0,
        || elapsed.get(),
        || elapsed.set(elapsed.get() + Duration::from_millis(250)),
        || false
    ));
    assert_eq!(elapsed.get(), Duration::from_secs(30));
    wait_with(
        true,
        || panic!("shutdown sampled CPU"),
        || Duration::ZERO,
        || panic!("shutdown slept"),
        || true,
    );
}

/// Removing hysteresis would resume recovery in the noisy band just under its CPU ceiling.
#[test]
fn pacing_pauses_above_75_and_resumes_below() {
    assert!(!paused(false, 75.0));
    assert!(paused(false, 75.1));
    assert!(paused(true, 70.0));
    assert!(paused(true, 65.0));
    assert!(!paused(true, 64.9));
}
