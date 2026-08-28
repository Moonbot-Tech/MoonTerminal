use super::*;

/// Build a run state with both halves reported and confirmed.
fn state(started: Option<bool>, trading: Option<bool>) -> CoreRunState {
    CoreRunState {
        online: true,
        started,
        started_confirmed: started.is_some(),
        auto_detect: None,
        trading,
        trading_confirmed: trading.is_some(),
    }
}

/// An intent is outstanding until the core reports the state it asked for.
#[test]
fn an_intent_waits_for_the_state_it_asked_for() {
    let now = Instant::now();
    let mut pending = RunPending::default();
    pending.arm(7, RunAsk::Trading(true), false, now);

    assert_eq!(
        pending
            .active(7, RunHalf::Trading, state(None, Some(false)), now)
            .map(|ask| ask.kind),
        Some(RunAsk::Trading(true)),
        "the core still reports the old value"
    );
    assert_eq!(
        pending.active(7, RunHalf::Trading, state(None, Some(true)), now),
        None,
        "the core reached the asked-for state"
    );
}

/// A value the current connection has not re-reported cannot answer for a command sent after it.
#[test]
fn an_unconfirmed_value_does_not_answer() {
    let now = Instant::now();
    let mut pending = RunPending::default();
    pending.arm(7, RunAsk::Trading(true), false, now);

    let mut stale = state(None, Some(true));
    stale.trading_confirmed = false;
    assert_eq!(
        pending
            .active(7, RunHalf::Trading, stale, now)
            .map(|ask| ask.kind),
        Some(RunAsk::Trading(true)),
        "a carried-over value describes the connection before the press"
    );
}

/// The two halves are reported over different commands, so neither answers for the other.
#[test]
fn the_halves_are_independent() {
    let now = Instant::now();
    let mut pending = RunPending::default();
    pending.arm(7, RunAsk::Restart, false, now);
    pending.arm(7, RunAsk::Trading(false), false, now);

    // The strategy engine answered; the runtime has not.
    let answered_trading = state(Some(false), Some(false));
    assert_eq!(
        pending
            .active(7, RunHalf::Runtime, answered_trading, now)
            .map(|ask| ask.kind),
        Some(RunAsk::Restart)
    );
    assert_eq!(
        pending.active(7, RunHalf::Trading, answered_trading, now),
        None
    );
}

/// An unanswered intent gives the control back after the timeout.
#[test]
fn unanswered_intent_expires() {
    let now = Instant::now();
    let mut pending = RunPending::default();
    pending.arm(7, RunAsk::Restart, false, now);
    let waiting = state(Some(false), None);

    let almost = now + PENDING_TIMEOUT - Duration::from_millis(1);
    assert_eq!(
        pending
            .active(7, RunHalf::Runtime, waiting, almost)
            .map(|ask| ask.kind),
        Some(RunAsk::Restart)
    );
    assert_eq!(
        pending.active(7, RunHalf::Runtime, waiting, now + PENDING_TIMEOUT),
        None
    );
}

/// A core nobody asked anything of is never waiting.
#[test]
fn unknown_core_is_not_pending() {
    let pending = RunPending::default();
    assert_eq!(
        pending.active(1, RunHalf::Trading, state(None, None), Instant::now()),
        None
    );
}

/// Re-arming the SAME half replaces its ask rather than stacking a second one.
#[test]
fn rearming_replaces_the_ask() {
    let now = Instant::now();
    let mut pending = RunPending::default();
    pending.arm(7, RunAsk::Trading(true), false, now);
    pending.arm(7, RunAsk::Trading(false), false, now);
    assert_eq!(
        pending
            .active(7, RunHalf::Trading, state(None, Some(true)), now)
            .map(|ask| ask.kind),
        Some(RunAsk::Trading(false)),
        "the newer ask is the one still waiting, and the old value satisfies neither"
    );
}

/// The token moves on arming and on a sweep that actually dropped something.
#[test]
fn revision_tracks_register_changes() {
    let now = Instant::now();
    let mut pending = RunPending::default();
    let start = pending.rev();

    pending.arm(7, RunAsk::Restart, false, now);
    let armed = pending.rev();
    assert_ne!(armed, start);

    assert!(!pending.sweep(now), "nothing expired yet");
    assert_eq!(pending.rev(), armed, "a sweep that drops nothing is silent");

    assert!(pending.sweep(now + PENDING_TIMEOUT));
    assert_ne!(pending.rev(), armed);
    assert!(pending.is_empty());
}

/// Only one expiry sweep is scheduled at a time, and a sweep that leaves entries behind must let
/// the caller schedule the next one — otherwise a press made under a running timer never expires.
#[test]
fn a_sweep_that_leaves_entries_releases_its_claim() {
    let now = Instant::now();
    let mut pending = RunPending::default();
    pending.arm(1, RunAsk::Restart, false, now);
    assert!(pending.claim_sweep(), "the first press schedules the sweep");
    assert!(
        !pending.claim_sweep(),
        "a second press must not add a timer"
    );

    // A later press, still outstanding when the first press's timer fires.
    pending.arm(
        2,
        RunAsk::Trading(true),
        true,
        now + PENDING_TIMEOUT - Duration::from_secs(2),
    );

    assert!(
        pending.sweep(now + PENDING_TIMEOUT),
        "the first ask expired"
    );
    assert!(!pending.is_empty(), "the later ask is still waiting");
    assert!(
        pending.claim_sweep(),
        "the sweep must release its claim so the later ask gets a timer of its own"
    );
}

/// The register carries WHICH control pressed, so a caption and the rows under it do not share a
/// waiting face.
///
/// Breakage: without it, one row's press blanks the group control nobody touched, and a group press
/// that skipped the cores already in the asked-for state leaves that control looking idle.
#[test]
fn an_ask_remembers_whether_a_group_control_sent_it() {
    let now = Instant::now();
    let mut pending = RunPending::default();
    pending.arm(7, RunAsk::Trading(true), false, now);
    assert_eq!(
        pending
            .active(7, RunHalf::Trading, state(None, Some(false)), now)
            .map(|ask| ask.from_group),
        Some(false)
    );

    pending.arm(7, RunAsk::Trading(true), true, now);
    assert_eq!(
        pending
            .active(7, RunHalf::Trading, state(None, Some(false)), now)
            .map(|ask| ask.from_group),
        Some(true)
    );
}
