//! Background discovery regressions with a blocked provider and injected UTC clock.

use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A slow first lookup must not stall a status request or admit duplicate discovery threads.
#[test]
fn first_lookup_answers_immediately_and_deduplicates_refreshes() {
    let mut watch = ReleaseWatch::default();
    let (release, blocked) = mpsc::channel();
    let (entered, active) = mpsc::channel();
    let now = UNIX_EPOCH + Duration::from_secs(1);
    watch.refresh_with(now, move || {
        entered.send(()).unwrap();
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        ReleaseCheck::Newer("v0.24.1".into())
    });
    active.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        watch.latest(),
        ReleaseCheck::Failed(ReleaseFailure::Timeout)
    );
    watch.refresh_with(now, || panic!("duplicate upstream lookup"));
    release.send(()).unwrap();
    watch.worker.take().unwrap().join().unwrap();
    assert_eq!(watch.latest(), ReleaseCheck::Newer("v0.24.1".into()));
}

/// A status before the next UTC slot keeps the cached result; one at the boundary starts a
/// refresh and still serves the old result while that refresh is blocked.
#[test]
fn utc_slots_refresh_without_erasing_the_previous_result() {
    let mut watch = ReleaseWatch::default();
    let now = UNIX_EPOCH + Duration::from_secs(1);
    watch.refresh_with(now, || ReleaseCheck::Current);
    watch.worker.take().unwrap().join().unwrap();
    let slot = watch.next_refresh.unwrap();
    assert_eq!(slot, UNIX_EPOCH + Duration::from_secs(6 * 3600));
    watch.refresh_with(slot - Duration::from_secs(1), || panic!("early refresh"));
    assert!(watch.worker.is_none());
    let (release, blocked) = mpsc::channel();
    watch.refresh_with(slot, move || {
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        ReleaseCheck::Failed(ReleaseFailure::Unavailable(
            "fixture upstream failure".into(),
        ))
    });
    assert_eq!(watch.latest(), ReleaseCheck::Current);
    release.send(()).unwrap();
    watch.worker.take().unwrap().join().unwrap();
    assert_eq!(
        watch.latest(),
        ReleaseCheck::Failed(ReleaseFailure::Unavailable(
            "fixture upstream failure".into()
        ))
    );
}

/// Time the actual ten-core status renderer while release discovery remains unresolved. The old
/// fresh-discovery path waited 109.159 ms with a 100 ms injected timeout (10 s in production).
#[test]
#[ignore = "status upstream latency probe"]
fn probe_status_reply() {
    moon_tg::warm_locales();
    let status: moon_core::station_api::Status = serde_json::from_value(serde_json::json!({
        "station_version": "v0.24.0", "cores_ready": 10, "cores_total": 10, "bot": null
    }))
    .unwrap();
    let navigation = moon_tg::station_owner_navigation(&Default::default());
    let mut watch = ReleaseWatch::default();
    let (release, blocked) = mpsc::channel();
    let (entered, active) = mpsc::channel();
    watch.refresh_with(SystemTime::now(), move || {
        entered.send(()).unwrap();
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        ReleaseCheck::Current
    });
    active.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut timings = Vec::new();
    for _ in 0..100 {
        let start = Instant::now();
        let check = watch.latest();
        let response = moon_tg::station_status_reply(&status, &check, navigation.clone(), false);
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(check, ReleaseCheck::Failed(ReleaseFailure::Timeout));
        assert!(matches!(
            response,
            moon_core::telegram::runtime::Response::Rich { .. }
        ));
    }
    release.send(()).unwrap();
    watch.worker.take().unwrap().join().unwrap();
    timings.sort_by(f64::total_cmp);
    eprintln!(
        "status cached discovery + ten-core render, upstream blocked: p50={:.3} p95={:.3} ms",
        timings[49], timings[94]
    );
    assert!(
        timings[94] < 100.0,
        "status must not wait for release discovery"
    );
}
