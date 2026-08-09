use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use super::*;

/// Test sink whose first layout write can be blocked while dispatch behavior is inspected.
struct RecordingSink {
    writes: Arc<Mutex<Vec<String>>>,
    entered: Option<Arc<Barrier>>,
    release: Option<Arc<Barrier>>,
    fail_layout_once: Arc<AtomicBool>,
}

impl PersistenceSink for RecordingSink {
    /// Record and optionally block one layout write.
    ///
    /// Args:
    ///     layout: Snapshot whose rail width identifies the request.
    ///
    /// Returns:
    ///     `false` once when failure injection is armed, otherwise `true`.
    fn save_layout(&mut self, layout: &WindowLayout) -> bool {
        self.writes
            .lock()
            .unwrap()
            .push(format!("layout:{:?}", layout.auto_workspace_rail_width));
        if let Some(entered) = self.entered.take() {
            entered.wait();
        }
        if let Some(release) = self.release.take() {
            release.wait();
        }
        !self.fail_layout_once.swap(false, Ordering::SeqCst)
    }

    /// Record one paired Classic write.
    ///
    /// Args:
    ///     docks: Complete Classic dock map, unused by the recorder.
    ///     detached: Detached list whose first panel identifies the request.
    ///
    /// Returns:
    ///     Always `true` for these coordinator-order tests.
    fn save_classic(&mut self, _docks: &DockMap, detached: &[DetachedSpec]) -> bool {
        self.writes.lock().unwrap().push(format!(
            "classic:{}",
            detached.first().map_or("empty", |spec| spec.panel.as_str())
        ));
        true
    }

    /// Record one shared Auto topology write.
    ///
    /// Args:
    ///     topology: Topology whose stable panel names identify the request.
    ///
    /// Returns:
    ///     Always `true` for these coordinator-order tests.
    fn save_auto(&mut self, topology: &DockTopologyByName) -> bool {
        self.writes
            .lock()
            .unwrap()
            .push(format!("auto:{}", topology.panel_names().join(",")));
        true
    }
}

/// Build one layout snapshot identified by its Auto rail width.
///
/// Args:
///     width: Marker stored in the otherwise default layout.
///
/// Returns:
///     Complete immutable layout snapshot.
fn layout(width: f32) -> WindowLayout {
    WindowLayout {
        auto_workspace_rail_width: Some(width),
        ..WindowLayout::default()
    }
}

/// Build one detached specification identified by its panel name.
///
/// Args:
///     panel: Marker used by the recording sink.
///
/// Returns:
///     Complete detached specification.
fn detached(panel: &str) -> DetachedSpec {
    DetachedSpec::new("group".to_string(), panel.to_string())
}

/// Build one coordinator around shared recording and optional first-write barriers.
///
/// Args:
///     writes: Ordered sink call log.
///     entered: Optional barrier reached after the first write begins.
///     release: Optional barrier that holds the first write until the test releases it.
///     fail_layout_once: One-shot layout failure switch.
///
/// Returns:
///     Test coordinator using the injected sink.
fn coordinator(
    writes: Arc<Mutex<Vec<String>>>,
    entered: Option<Arc<Barrier>>,
    release: Option<Arc<Barrier>>,
    fail_layout_once: Arc<AtomicBool>,
) -> PersistenceCoordinator {
    let fallback_writes = writes.clone();
    let fallback_failure = fail_layout_once.clone();
    PersistenceCoordinator::with_sinks(
        Box::new(RecordingSink {
            writes,
            entered,
            release,
            fail_layout_once,
        }),
        Box::new(RecordingSink {
            writes: fallback_writes,
            entered: None,
            release: None,
            fail_layout_once: fallback_failure,
        }),
    )
}

/// Wait a bounded interval for one worker acknowledgement.
///
/// Args:
///     coordinator: Dispatch side with exactly one request in flight.
///
/// Returns:
///     Completed acknowledgement before the two-second test deadline.
fn wait_for_ack(coordinator: &mut PersistenceCoordinator) -> PersistenceAck {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(acknowledgement) = coordinator.poll() {
            return acknowledgement;
        }
        assert!(
            Instant::now() < deadline,
            "persistence worker did not acknowledge within the test deadline"
        );
        std::thread::yield_now();
    }
}

/// `persistence/coordinator.rs:PersistenceCoordinator::dispatch` must only enqueue; moving
/// `persist_snapshot` into that method makes this assertion exceed the barrier delay and proves the
/// user-visible live-loop stall has returned.
#[test]
fn dispatch_returns_while_the_worker_sink_is_blocked() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut coordinator = coordinator(
        writes,
        Some(entered.clone()),
        Some(release.clone()),
        Arc::new(AtomicBool::new(false)),
    );

    let releaser = std::thread::spawn(move || {
        entered.wait();
        std::thread::sleep(Duration::from_millis(250));
        release.wait();
    });
    let started_at = Instant::now();
    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_layout(layout(100.0))));
    let dispatch_elapsed = started_at.elapsed();
    assert!(
        dispatch_elapsed < Duration::from_millis(200),
        "dispatch blocked on the sink for {dispatch_elapsed:?}"
    );
    assert!(coordinator.is_in_flight());
    assert_eq!(coordinator.poll(), None);
    releaser.join().unwrap();
    wait_for_ack(&mut coordinator);
}

/// `persistence/coordinator.rs:PersistenceCoordinator::dispatch` must report an in-flight request
/// as rejected; returning `true` makes the live loop clear the later dirty state even though its
/// 200 px resize was never queued.
#[test]
fn one_in_flight_request_coalesces_later_mutations() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut coordinator = coordinator(
        writes.clone(),
        Some(entered.clone()),
        Some(release.clone()),
        Arc::new(AtomicBool::new(false)),
    );

    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_layout(layout(100.0))));
    entered.wait();
    let accepted_later =
        coordinator.dispatch(PersistenceSnapshot::empty().with_layout(layout(200.0)));
    release.wait();
    wait_for_ack(&mut coordinator);
    assert!(
        !accepted_later,
        "an in-flight worker must reject rather than falsely accept a later snapshot"
    );
    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_layout(layout(300.0))));
    wait_for_ack(&mut coordinator);

    assert_eq!(
        *writes.lock().unwrap(),
        vec!["layout:Some(100.0)", "layout:Some(300.0)"]
    );
}

/// `startup.rs:dispatch_live_persistence` must not clear dirty state from a success acknowledgement;
/// adding that clear loses the later layout mutation and restores stale state after restart.
#[test]
fn in_flight_success_does_not_claim_a_later_mutation() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let mut coordinator = coordinator(writes, None, None, Arc::new(AtomicBool::new(false)));
    let request = PersistenceSnapshot::empty().with_layout(layout(100.0));
    assert!(coordinator.dispatch(request));
    // Dispatch cleared the dirty flag; this is the later mutation the old acknowledgement must
    // never clear.
    let mut layout_dirty = true;
    let acknowledgement = wait_for_ack(&mut coordinator);
    if acknowledgement.failed().layout {
        layout_dirty = true;
    }
    assert!(layout_dirty);
}

/// `persistence/coordinator.rs:PersistenceAck::failed` must report a failed selected class;
/// returning an empty mask prevents the live loop from retrying the latest user layout.
#[test]
fn failure_acknowledgement_selects_the_class_for_retry() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let mut coordinator = coordinator(writes, None, None, Arc::new(AtomicBool::new(true)));
    assert!(
        coordinator.dispatch(
            PersistenceSnapshot::empty()
                .with_layout(layout(100.0))
                .with_classic(DockMap::new(), vec![detached("succeeds")])
        )
    );
    let acknowledgement = wait_for_ack(&mut coordinator);
    assert_eq!(
        acknowledgement.failed(),
        PersistenceClasses {
            layout: true,
            classic: false,
            auto: false,
        }
    );
    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_layout(layout(200.0))));
    let retry = wait_for_ack(&mut coordinator);
    assert_eq!(retry.failed(), PersistenceClasses::default());
}

/// `persistence/coordinator.rs:PersistenceCoordinator::shutdown` must queue its command through the
/// worker; calling the sink directly reverses this order and races identical temp paths on quit.
#[test]
fn shutdown_writes_the_final_full_snapshot_after_in_flight_work() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut coordinator = coordinator(
        writes.clone(),
        Some(entered.clone()),
        Some(release.clone()),
        Arc::new(AtomicBool::new(false)),
    );
    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_layout(layout(100.0))));
    entered.wait();
    let shutdown = std::thread::spawn(move || {
        coordinator.shutdown(
            PersistenceSnapshot::empty()
                .with_layout(layout(300.0))
                .with_classic(DockMap::new(), vec![detached("final")])
                .with_auto(DockTopologyByName::tab_preset(["Report", "Log"])),
        )
    });
    release.wait();
    let acknowledgement = shutdown.join().unwrap();

    assert_eq!(acknowledgement.failed(), PersistenceClasses::default());
    assert_eq!(
        *writes.lock().unwrap(),
        vec![
            "layout:Some(100.0)",
            "layout:Some(300.0)",
            "classic:final",
            "auto:Report,Log"
        ]
    );
}

/// `persistence/coordinator.rs:PersistenceCoordinator::shutdown` must retry a failed final worker
/// snapshot through its quit-only fallback; returning the failed acknowledgement loses the latest
/// layout even though the application waited for shutdown.
#[test]
fn shutdown_retries_failed_final_classes_through_the_fallback_sink() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let mut coordinator = coordinator(writes.clone(), None, None, Arc::new(AtomicBool::new(true)));

    let acknowledgement =
        coordinator.shutdown(PersistenceSnapshot::empty().with_layout(layout(500.0)));

    assert_eq!(acknowledgement.failed(), PersistenceClasses::default());
    assert_eq!(
        *writes.lock().unwrap(),
        vec!["layout:Some(500.0)", "layout:Some(500.0)"]
    );
}
