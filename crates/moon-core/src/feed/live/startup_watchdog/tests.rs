use super::*;

use crate::feed::CoreStartupState;

/// Build a snapshot on one step with a given completed mask.
fn snap(step: Option<CoreInitStep>, completed: u16) -> CoreStartupStatus {
    CoreStartupStatus {
        state: CoreStartupState::Initializing,
        current_step: step,
        completed_mask: completed,
        ..CoreStartupStatus::default()
    }
}

/// A startup that keeps completing steps keeps its connection, however long it takes overall.
#[test]
fn advancing_startup_never_stalls() {
    let mut dog = StartupWatchdog::default();
    let mut at = Instant::now();

    for (index, step) in [
        CoreInitStep::BaseCheck,
        CoreInitStep::AuthCheck,
        CoreInitStep::GetMarketsList,
        CoreInitStep::UpdateMarketsList,
    ]
    .into_iter()
    .enumerate()
    {
        // Each step lands just short of the budget, so the TOTAL is several times past it.
        at += STARTUP_STALL - Duration::from_secs(1);
        let mask = (1u16 << index) - 1;
        assert!(
            !dog.observe(&snap(Some(step), mask), at, false),
            "progress moved, so the clock restarted"
        );
    }
}

/// A startup frozen on one step for the whole budget is reported.
#[test]
fn frozen_startup_is_reported() {
    let start = Instant::now();
    let mut dog = StartupWatchdog::default();
    let stuck = snap(Some(CoreInitStep::GetMarketsList), 0b11);

    assert!(
        !dog.observe(&stuck, start, false),
        "the first sighting only marks"
    );
    assert!(
        !dog.observe(&stuck, start + STARTUP_STALL - Duration::from_millis(1), false),
        "one poll short of the budget"
    );
    assert!(dog.observe(&stuck, start + STARTUP_STALL, false));
}

/// Nothing outside the achievement fields restarts the clock.
///
/// Breakage, per mutator: the PHASE follows MoonProto's authorization flag, so a link that
/// re-handshakes more often than the budget would hide a zero-progress init forever — the shape of
/// the incident this exists for. The SLICED counters are read from the transport slicer, below the
/// domain-ready dispatch filter, so a step that keeps timing out while its answer keeps partly
/// arriving moves them without init advancing. The RETRY counters count attempts, not achievements,
/// and MoonProto re-sends the same step forever.
#[test]
fn only_the_achievement_fields_restart_the_clock() {
    /// One named way the snapshot can churn without init achieving anything.
    type Churn = (&'static str, fn(&mut CoreStartupStatus));

    let churn: [Churn; 4] = [
        ("phase", |s| {
            s.state = CoreStartupState::Reconnecting;
            s.reconnect_count += 1;
        }),
        ("sliced transfer", |s| {
            s.received_sliced_bytes += 512 * 1024;
            s.received_sliced_blocks += 400;
            s.active_received_blocks = 12;
            s.idle_for_ms = Some(4);
        }),
        ("duplicates", |s| {
            s.duplicate_sliced_blocks += 900;
        }),
        ("retries", |s| {
            s.current_step_retries += 1;
            s.total_init_retries += 1;
            s.elapsed_ms += STARTUP_STALL.as_millis() as u64;
        }),
    ];

    for (name, churn) in churn {
        let start = Instant::now();
        let mut dog = StartupWatchdog::default();
        let mut busy = snap(Some(CoreInitStep::GetMarketsList), 0b11);

        assert!(!dog.observe(&busy, start, false), "{name}: first sighting");
        churn(&mut busy);
        assert!(
            !dog.observe(&busy, start + Duration::from_secs(30), false),
            "{name}: still inside the budget"
        );
        assert!(
            dog.observe(&busy, start + STARTUP_STALL, false),
            "{name} is not progress, so the clock kept running"
        );
    }
}

/// Progress that goes BACKWARDS — the retry path walking a step down — is movement too.
#[test]
fn regressed_progress_restarts_the_clock() {
    let start = Instant::now();
    let mut dog = StartupWatchdog::default();

    let ahead = snap(Some(CoreInitStep::StrategySchema), 0b1111);
    assert!(!dog.observe(&ahead, start, false));
    let almost = start + STARTUP_STALL - Duration::from_secs(1);
    let back = snap(Some(CoreInitStep::BaseCheck), 0);
    assert!(!dog.observe(&back, almost, false));
    assert!(!dog.observe(&back, almost + Duration::from_secs(2), false));
}

/// The watchdog retires itself once initialization has finished.
///
/// Breakage: the rule that it watches the FIRST startup only lives inside the type, not in an `&&`
/// at the call site. Past init, MoonProto publishes `Ready`/`Reconnecting` in step with
/// authorization, so a perfectly ordinary post-init blip is indistinguishable from a stall here and
/// a working core would be torn down every budget.
#[test]
fn an_initialized_client_is_no_longer_watched() {
    let start = Instant::now();
    let mut dog = StartupWatchdog::default();
    let frozen = snap(None, 0xFF);

    assert!(!dog.observe(&frozen, start, true));
    assert!(!dog.observe(&frozen, start + STARTUP_STALL * 10, true));
}

/// The reported error names the step and the progress the attempt reached.
#[test]
fn the_error_names_where_it_stopped() {
    let text = StartupStalled::of(&CoreStartupStatus {
        current_step: Some(CoreInitStep::GetMarketsList),
        completed_mask: 0b11,
        ..CoreStartupStatus::default()
    })
    .to_string();
    assert!(text.contains("GetMarketsList"), "{text}");
    assert!(text.contains("2/8"), "{text}");
}
