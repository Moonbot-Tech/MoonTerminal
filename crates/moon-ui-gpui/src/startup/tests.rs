//! Report-writer coordination tests.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{ReportRevisionDecision, ReportRevisionGate, TickEdges, consume_report_commit};

/// Build one tick's edges from the four flags, in declaration order.
///
/// Args:
///     immediate_report: Live report data committed.
///     background_report: Historical catch-up data committed.
///     valuation: The valuation worker published committed values.
///     valuation_status: Published valuation health changed shape.
///
/// Returns:
///     Edges for one coordination tick.
fn edges(
    immediate_report: bool,
    background_report: bool,
    valuation: bool,
    valuation_status: bool,
) -> TickEdges {
    TickEdges {
        immediate_report,
        background_report,
        valuation,
        valuation_status,
    }
}

/// `startup.rs:consume_report_commit` must invoke the dedicated report-revision notification for
/// a set edge; removing that call leaves an open Analytics window stale until an unrelated UI wake.
#[test]
fn committed_report_edge_notifies_once() {
    let dirty = AtomicBool::new(true);
    let notifications = Cell::new(0);

    consume_report_commit(Some(&dirty), || {
        notifications.set(notifications.get() + 1);
    });
    consume_report_commit(Some(&dirty), || {
        notifications.set(notifications.get() + 1);
    });

    assert_eq!(notifications.get(), 1);
    assert!(!dirty.load(Ordering::Acquire));
}

/// `startup.rs:ReportRevisionGate::observe` must retain a valuation-only edge until its one-minute
/// boundary; publishing every observed edge recreates the terminal-wide Analytics stutter, while
/// clearing the pending bit before the boundary loses the final historical valuation refresh.
#[test]
fn valuation_revision_is_coalesced_and_published_at_the_boundary() {
    let start = Instant::now();
    let mut gate = ReportRevisionGate::new(start);

    assert_eq!(
        gate.observe(
            edges(false, false, true, false),
            start + Duration::from_secs(1)
        ),
        ReportRevisionDecision::default()
    );
    assert_eq!(
        gate.observe(
            edges(false, false, true, false),
            start + Duration::from_secs(59)
        ),
        ReportRevisionDecision::default()
    );
    assert_eq!(
        gate.observe(
            edges(false, false, false, false),
            start + Duration::from_secs(60)
        ),
        ReportRevisionDecision {
            notify: true,
            wake_valuation: false,
        }
    );

    assert_eq!(
        gate.observe(
            edges(false, false, true, false),
            start + Duration::from_secs(61)
        ),
        ReportRevisionDecision::default()
    );
    assert_eq!(
        gate.observe(
            edges(false, false, false, false),
            start + Duration::from_secs(120)
        ),
        ReportRevisionDecision {
            notify: true,
            wake_valuation: false,
        }
    );
}

/// `startup.rs:ReportRevisionGate::observe` must prioritize a report edge over pending valuation
/// work; removing this branch delays live trades, omits `valuation.wake()`, or emits a duplicate
/// valuation notification one interval after the report already covered its generation.
#[test]
fn report_revision_publishes_immediately_and_absorbs_pending_valuation() {
    let start = Instant::now();
    let mut gate = ReportRevisionGate::new(start);

    assert_eq!(
        gate.observe(
            edges(false, false, true, false),
            start + Duration::from_secs(1)
        ),
        ReportRevisionDecision::default()
    );
    assert_eq!(
        gate.observe(
            edges(true, true, true, false),
            start + Duration::from_secs(2)
        ),
        ReportRevisionDecision {
            notify: true,
            wake_valuation: true,
        }
    );
    assert_eq!(
        gate.observe(
            edges(false, false, false, false),
            start + Duration::from_secs(62)
        ),
        ReportRevisionDecision::default()
    );
    assert_eq!(
        gate.observe(
            edges(true, false, false, false),
            start + Duration::from_secs(63)
        ),
        ReportRevisionDecision {
            notify: true,
            wake_valuation: true,
        }
    );
}

/// `startup.rs:ReportRevisionGate::observe` must wake valuation immediately for a background
/// report page without notifying report readers; tying the wake to the minute boundary stalls
/// outbox processing, while notifying here restores the periodic Analytics freeze.
#[test]
fn background_report_wakes_valuation_without_immediate_ui_refresh() {
    let start = Instant::now();
    let mut gate = ReportRevisionGate::new(start);

    assert_eq!(
        gate.observe(
            edges(false, true, false, false),
            start + Duration::from_secs(1)
        ),
        ReportRevisionDecision {
            notify: false,
            wake_valuation: true,
        }
    );
    assert_eq!(
        gate.observe(
            edges(false, false, false, false),
            start + Duration::from_secs(60)
        ),
        ReportRevisionDecision {
            notify: true,
            wake_valuation: false,
        }
    );
}

/// `startup.rs:ReportRevisionGate::observe` must publish a valuation HEALTH change even when no
/// rows committed, and must not treat it as a report commit.
///
/// Breakage: dropping `edges.valuation_status` from the `background_pending` expression. A
/// stalled worker commits nothing by definition, so the revision would never be published and the
/// report footer's stall chip could not appear until an unrelated report write happened to arrive
/// — which on an idle terminal is never. Folding it into `report_committed` instead would also
/// unpark the worker for a UI-only health transition that has no new valuation work.
#[test]
fn a_health_change_alone_still_reaches_a_revision() {
    let start = Instant::now();
    let mut gate = ReportRevisionGate::new(start);

    assert_eq!(
        gate.observe(
            edges(false, false, false, true),
            start + Duration::from_secs(1)
        ),
        ReportRevisionDecision::default(),
        "a health change never wakes the worker"
    );
    assert_eq!(
        gate.observe(
            edges(false, false, false, false),
            start + Duration::from_secs(60)
        ),
        ReportRevisionDecision {
            notify: true,
            wake_valuation: false,
        },
        "and is published at the next boundary"
    );
}

/// Read the startup source governed by the ordering contract.
///
/// Returns:
///     UTF-8 source text from the sibling `startup.rs`.
fn startup_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("startup.rs");
    std::fs::read_to_string(path).unwrap()
}

/// Return the byte position of a required source anchor.
///
/// Args:
///     source: Complete startup source.
///     anchor: Exact architectural call-site anchor.
///
/// Returns:
///     First matching byte position.
fn position(source: &str, anchor: &str) -> usize {
    source
        .find(anchor)
        .unwrap_or_else(|| panic!("missing startup contract anchor: {anchor}"))
}

/// `startup.rs:run` must consume the gate's complete coordination decision; replacing either
/// decision field with the raw dirty flags passes the pure gate tests but restores duplicate UI
/// notifications or stops new reports from waking historical valuation.
#[test]
fn report_revision_decision_stays_wired_to_the_coordination_loop() {
    let source = startup_source();
    let immediate = position(
        &source,
        "consume_report_commit(coord_report_immediate_dirty.as_deref()",
    );
    let background = position(
        &source,
        "consume_report_commit(coord_report_background_dirty.as_deref()",
    );
    let status = position(
        &source,
        "consume_report_commit(coord_valuation_status_dirty.as_deref()",
    );
    let observe = position(&source, "let revision = report_revision_gate.observe(");
    let wake = position(&source, "if revision.wake_valuation {");
    let notify = position(&source, "if revision.notify {");

    assert!(immediate < background);
    assert!(background < status);
    assert!(status < observe);
    assert!(observe < wake);
    assert!(wake < notify);
    assert!(source[wake..notify].contains("valuation.wake();"));
    assert!(source[notify..].contains("coord_report_revision.update(cx, |_, cx| cx.notify());"));
}

/// `startup.rs:run` must read the reports uid floor before recovery and recover before the writer.
///
/// Moving `report_recovery::prepare()` above `observed_uid_floor` loses deleted-core uid history
/// when the damaged replica is replaced. Moving it below `app.run` or bypassing the private permit
/// starts a writer before the damaged main/WAL/SHM set has been safely preserved.
#[test]
fn report_recovery_stays_between_uid_floor_and_writer_start() {
    let source = startup_source();
    let uid_floor = position(&source, "let uid_floor = observed_uid_floor");
    let recovery = position(
        &source,
        "let report_write_permit = moon_core::db::report_recovery::prepare()",
    );
    let app_run = position(&source, "app.run(move |cx|");
    let writer = position(
        &source,
        "report_write_permit.and_then(moon_core::db::spawn_writer)",
    );

    assert!(uid_floor < recovery);
    assert!(recovery < app_run);
    assert!(app_run < writer);
}
