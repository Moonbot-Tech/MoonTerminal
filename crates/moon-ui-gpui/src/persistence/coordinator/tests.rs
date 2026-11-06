use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use super::*;

/// Complete synthetic settings authority identified by the edited card width.
fn detects_file(width: u16) -> moon_core::config::DetectViewFile {
    let mut file = moon_core::config::DetectViewFile::default();
    let mut cfg = moon_core::config::DetectViewCfg::default();
    cfg.medium.w = width;
    file.set_group("UNIT_01", cfg);
    file
}

/// Build a detection-only sink with deterministic blocking and failure injection.
fn detects_coordinator(
    writes: Arc<Mutex<Vec<String>>>,
    entered: Option<Arc<Barrier>>,
    release: Option<Arc<Barrier>>,
    fail_detects_once: bool,
) -> PersistenceCoordinator {
    let sink = RecordingSink {
        writes: writes.clone(),
        entered,
        release,
        fail_write_once: Arc::new(AtomicBool::new(false)),
        fail_detects_once,
    };
    let fallback = RecordingSink {
        writes,
        entered: None,
        release: None,
        fail_write_once: Arc::new(AtomicBool::new(false)),
        fail_detects_once: false,
    };
    PersistenceCoordinator::with_sinks(Box::new(sink), Box::new(fallback))
}

/// A blocked disk save must neither block dispatch nor lose the final slider value on quit.
#[test]
fn detects_coalesces_in_flight_mutations_and_flushes_latest_value_on_quit() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut coordinator = detects_coordinator(
        writes.clone(),
        Some(entered.clone()),
        Some(release.clone()),
        false,
    );
    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_detects(detects_file(100))));
    entered.wait();
    let rejected =
        !coordinator.dispatch(PersistenceSnapshot::empty().with_detects(detects_file(200)));
    let shutdown = std::thread::spawn(move || {
        coordinator.shutdown(PersistenceSnapshot::empty().with_detects(detects_file(300)))
    });
    release.wait();
    let acknowledgement = shutdown.join().unwrap();
    assert!(rejected);
    assert_eq!(acknowledgement.failed(), PersistenceClasses::default());
    assert_eq!(*writes.lock().unwrap(), ["detects:100", "detects:300"]);
}

/// A failed detection save must select that authority for retry, including the quit fallback.
#[test]
fn detects_failure_is_acknowledged_and_latest_authority_can_be_retried() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let mut coordinator = detects_coordinator(writes.clone(), None, None, true);
    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_detects(detects_file(100))));
    let acknowledgement = wait_for_ack(&mut coordinator);
    assert!(acknowledgement.failed().detects);
    assert!(!acknowledgement.failed().layout);
    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_detects(detects_file(200))));
    assert_eq!(
        wait_for_ack(&mut coordinator).failed(),
        PersistenceClasses::default()
    );
    assert_eq!(*writes.lock().unwrap(), ["detects:100", "detects:200"]);
    let writes = Arc::new(Mutex::new(Vec::new()));
    let mut coordinator = detects_coordinator(writes.clone(), None, None, true);
    assert_eq!(
        coordinator
            .shutdown(PersistenceSnapshot::empty().with_detects(detects_file(250)))
            .failed(),
        PersistenceClasses::default()
    );
    assert_eq!(*writes.lock().unwrap(), ["detects:250", "detects:250"]);
}

/// Test sink whose first layout write can be blocked while dispatch behavior is inspected.
struct RecordingSink {
    writes: Arc<Mutex<Vec<String>>>,
    entered: Option<Arc<Barrier>>,
    release: Option<Arc<Barrier>>,
    fail_write_once: Arc<AtomicBool>,
    fail_detects_once: bool,
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
        !self.fail_write_once.swap(false, Ordering::SeqCst)
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

    /// Record the exact detection preference and optionally fail its first save.
    fn save_detects(&mut self, detects: &moon_core::config::DetectViewFile) -> bool {
        self.writes
            .lock()
            .unwrap()
            .push(format!("detects:{}", detects.group("UNIT_01").medium.w));
        if let Some(entered) = self.entered.take() {
            entered.wait();
        }
        if let Some(release) = self.release.take() {
            release.wait();
        }
        !std::mem::take(&mut self.fail_detects_once)
    }

    /// Record the immutable chart list instead of touching the user's files.
    fn save_charts(&mut self, charts: &[ChartTabSpec]) -> bool {
        self.writes
            .lock()
            .unwrap()
            .push(format!("charts:{}", charts.len()));
        !self.fail_write_once.swap(false, Ordering::SeqCst)
    }

    /// Record drawing persistence without accessing the user's figure file.
    fn save_figures(&mut self, _figures: &FigureSnapshot) -> bool {
        self.writes.lock().unwrap().push("figures".into());
        !self.fail_write_once.swap(false, Ordering::SeqCst)
    }

    /// Record the badge revision to distinguish edits made during an in-flight save.
    fn save_badges(&mut self, badges: &TabBadgeSettings) -> bool {
        self.writes
            .lock()
            .unwrap()
            .push(format!("badges:{}", badges.rev()));
        !self.fail_write_once.swap(false, Ordering::SeqCst)
    }

    /// Record the captured history length.
    fn save_updates(&mut self, updates: &CoreUpdateHistory) -> bool {
        self.writes
            .lock()
            .unwrap()
            .push(format!("updates:{}", updates.records.len()));
        !self.fail_write_once.swap(false, Ordering::SeqCst)
    }

    /// Record config ordering without reading or replacing real credentials.
    fn save_config(&mut self, _config: &ConfigSnapshot) -> bool {
        self.writes.lock().unwrap().push("config".into());
        !self.fail_write_once.swap(false, Ordering::SeqCst)
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
    let mut layout = WindowLayout::default();
    layout.auto_workspace_rail_width = Some(width);
    layout
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
///     fail_write_once: One-shot layout failure switch.
///
/// Returns:
///     Test coordinator using the injected sink.
fn coordinator(
    writes: Arc<Mutex<Vec<String>>>,
    entered: Option<Arc<Barrier>>,
    release: Option<Arc<Barrier>>,
    fail_write_once: Arc<AtomicBool>,
) -> PersistenceCoordinator {
    let fallback_writes = writes.clone();
    let fallback_failure = fail_write_once.clone();
    PersistenceCoordinator::with_sinks(
        Box::new(RecordingSink {
            writes,
            entered,
            release,
            fail_write_once,
            fail_detects_once: false,
        }),
        Box::new(RecordingSink {
            writes: fallback_writes,
            entered: None,
            release: None,
            fail_write_once: fallback_failure,
            fail_detects_once: false,
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
            ..Default::default()
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

/// Build all regular autosave classes with a chart count that identifies the captured generation.
fn workspace_snapshot(count: u32) -> PersistenceSnapshot {
    let charts = (0..count)
        .map(|num| ChartTabSpec::new("test".into(), num, moon_core::config::ChartBucket::Shared))
        .collect();
    PersistenceSnapshot::empty()
        .with_charts(charts)
        .with_figures(moon_core::figures::FigureStore::default().snapshot())
        .with_badges(TabBadgeSettings::default())
        .with_updates(CoreUpdateHistory::default())
        .with_config(
            moon_core::config::AppConfig::headless(Vec::new())
                .persistence_snapshot()
                .unwrap(),
        )
}

/// A stalled save cannot block dispatch, grow a queue, or overwrite the final charts on quit.
#[test]
fn workspace_autosave_coalesces_and_quit_writes_latest_after_blocked_io() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut coordinator = coordinator(
        writes.clone(),
        Some(entered.clone()),
        Some(release.clone()),
        Arc::new(AtomicBool::new(false)),
    );
    // The first sink call remains blocked while the application submits a later edit.
    assert!(coordinator.dispatch(workspace_snapshot(1).with_layout(layout(100.0))));
    entered.wait();
    assert!(!coordinator.dispatch(workspace_snapshot(2)));
    assert!(coordinator.poll().is_none());
    let shutdown = std::thread::spawn(move || coordinator.shutdown(workspace_snapshot(3)));
    release.wait();
    assert_eq!(
        shutdown.join().unwrap().failed(),
        PersistenceClasses::default()
    );
    assert_eq!(
        *writes.lock().unwrap(),
        [
            "layout:Some(100.0)",
            "charts:1",
            "figures",
            "badges:0",
            "updates:0",
            "config",
            "charts:3",
            "figures",
            "badges:0",
            "updates:0",
            "config",
        ]
    );
}

/// Every newly asynchronous class must acknowledge errors and allow retry without another edit.
#[test]
fn every_workspace_class_reports_failure_and_retries() {
    let all = workspace_snapshot(1);
    for selected in [
        PersistenceClasses {
            charts: true,
            ..Default::default()
        },
        PersistenceClasses {
            figures: true,
            ..Default::default()
        },
        PersistenceClasses {
            badges: true,
            ..Default::default()
        },
        PersistenceClasses {
            updates: true,
            ..Default::default()
        },
        PersistenceClasses {
            config: true,
            ..Default::default()
        },
    ] {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut coordinator =
            coordinator(writes.clone(), None, None, Arc::new(AtomicBool::new(true)));
        let snapshot = all.clone().retain_classes(selected);
        assert!(coordinator.dispatch(snapshot.clone()));
        assert_eq!(wait_for_ack(&mut coordinator).failed(), selected);
        assert!(coordinator.dispatch(snapshot));
        assert_eq!(
            wait_for_ack(&mut coordinator).failed(),
            PersistenceClasses::default()
        );
        assert_eq!(writes.lock().unwrap().len(), 2);
    }
}

/// Layout changes must not starve drawings, and a closed autosave window must keep edits pending.
#[test]
fn workspace_admission_is_independent_of_layout_and_limited_to_one_second() {
    let mut coordinator = coordinator(
        Arc::new(Mutex::new(Vec::new())),
        None,
        None,
        Arc::new(AtomicBool::new(false)),
    );
    assert!(coordinator.dispatch(PersistenceSnapshot::empty().with_layout(layout(100.0))));
    wait_for_ack(&mut coordinator);
    assert!(coordinator.admit_autosave(Instant::now()));
    assert!(coordinator.dispatch(workspace_snapshot(1)));
    wait_for_ack(&mut coordinator);
    let accepted = coordinator.last_autosave.unwrap();
    assert!(!coordinator.admit_autosave(accepted + Duration::from_millis(999)));
    assert!(coordinator.admit_autosave(accepted + Duration::from_secs(1)));
}

/// Explicit config/key changes cannot overtake an older automatic snapshot and be rolled back by it.
#[test]
fn deliberate_write_follows_the_pending_autosave_and_does_not_consume_future_acks() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut coordinator = coordinator(
        writes.clone(),
        Some(entered.clone()),
        Some(release.clone()),
        Arc::new(AtomicBool::new(false)),
    );
    assert!(coordinator.dispatch(workspace_snapshot(1).with_layout(layout(100.0))));
    entered.wait();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let manual_writes = writes.clone();
    let manual = std::thread::spawn(move || {
        let ack = coordinator.finish_pending().unwrap();
        assert_eq!(ack.failed(), PersistenceClasses::default());
        manual_writes.lock().unwrap().push("manual".into());
        done_tx.send(()).unwrap();
        assert!(!coordinator.has_pending_config());
        assert!(coordinator.poll().is_none());
        assert!(coordinator.dispatch(workspace_snapshot(2)));
        assert_eq!(
            wait_for_ack(&mut coordinator).failed(),
            PersistenceClasses::default()
        );
    });
    let completed_while_blocked = done_rx.recv_timeout(Duration::from_millis(30)).is_ok();
    release.wait();
    manual.join().unwrap();
    assert!(!completed_while_blocked);
    let writes = writes.lock().unwrap();
    let manual = writes.iter().position(|write| write == "manual").unwrap();
    assert_eq!(writes[manual - 1], "config");
    assert_eq!(writes[manual + 1], "charts:2");
}
