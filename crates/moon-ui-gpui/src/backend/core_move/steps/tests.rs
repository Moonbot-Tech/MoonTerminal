//! The fold of a move's step events into the six rows the wizard draws.

use std::time::Duration;

use super::*;

fn s(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

fn mark(id: StepId, state: StepState, note: StepNote) -> StepEvent {
    StepEvent::Mark { id, state, note }
}

fn states(steps: &Steps) -> Vec<StepState> {
    steps.rows().iter().map(|r| r.state).collect()
}

/// The first three steps of a move that reached the old core's stop.
fn first_three() -> Vec<(StepEvent, Duration)> {
    vec![
        (
            mark(
                StepId::Settings,
                StepState::Running,
                StepNote::TakingSettings,
            ),
            s(0),
        ),
        (
            mark(
                StepId::Settings,
                StepState::Done,
                StepNote::SettingsTaken {
                    strategies: Some(42),
                    history: true,
                },
            ),
            s(12),
        ),
        (
            mark(StepId::Server, StepState::Running, StepNote::CheckingServer),
            s(12),
        ),
        (
            mark(
                StepId::Server,
                StepState::Done,
                StepNote::ServerFits { port: 5160 },
            ),
            s(20),
        ),
        (
            mark(StepId::Install, StepState::Running, StepNote::Installing),
            s(20),
        ),
        (
            mark(
                StepId::Install,
                StepState::Done,
                StepNote::Installed {
                    dir: "mbcorea".into(),
                },
            ),
            s(65),
        ),
    ]
}

/// A move mid-way shows its finished steps ticked with their times, the current one running
/// and the rest waiting; a fold that lost the time or the note would blank the list.
#[test]
fn running_mid_way() {
    let mut events = first_three();
    events.push((
        mark(StepId::StopOld, StepState::Running, StepNote::Stopping),
        s(66),
    ));
    let steps = fold(events);
    use StepState::*;
    assert_eq!(
        states(&steps),
        [Done, Done, Done, Running, Waiting, Waiting]
    );
    assert_eq!(steps.rows()[2].at, Some(s(65)));
    assert_eq!(
        steps.rows()[3].at,
        None,
        "a running step has no finish time"
    );
    assert_eq!(
        steps.rows()[1].note,
        Some(StepNote::ServerFits { port: 5160 })
    );
    assert_eq!(steps.current(), StepId::StopOld);
}

/// A refusing old core holds step 4 with its open-deal count; later steps still wait.
#[test]
fn refused_with_open_deals() {
    let mut events = first_three();
    events.push((
        mark(StepId::StopOld, StepState::Running, StepNote::Stopping),
        s(66),
    ));
    events.push((
        mark(
            StepId::StopOld,
            StepState::Hold,
            StepNote::Refused {
                open_deals: Some(3),
            },
        ),
        s(67),
    ));
    let steps = fold(events);
    use StepState::*;
    assert_eq!(states(&steps), [Done, Done, Done, Hold, Waiting, Waiting]);
    assert_eq!(
        steps.rows()[3].note,
        Some(StepNote::Refused {
            open_deals: Some(3)
        })
    );
    assert_eq!(steps.current(), StepId::StopOld);
}

/// A finished move ticks all six steps.
#[test]
fn done() {
    let mut events = first_three();
    for (i, (id, note)) in [
        (StepId::StopOld, StepNote::OldStopped),
        (
            StepId::StartNew,
            StepNote::Started {
                logged_in: true,
                strategies: true,
            },
        ),
        (StepId::Switch, StepNote::Switched),
    ]
    .into_iter()
    .enumerate()
    {
        events.push((
            mark(id, StepState::Running, StepNote::Stopping),
            s(70 + i as u64),
        ));
        events.push((mark(id, StepState::Done, note), s(80 + i as u64)));
    }
    let steps = fold(events);
    assert!(steps.rows().iter().all(|r| r.state == StepState::Done));
    assert_eq!(steps.rows()[5].note, Some(StepNote::Switched));
    assert_eq!(steps.rows()[5].at, Some(s(82)));
}

/// A failure in the install shows its reason there and skips every later step — a later step
/// left "waiting" would read as a move still going on.
#[test]
fn failed_at_install_skips_the_rest() {
    let mut events = first_three();
    events.truncate(5);
    events.push((
        StepEvent::Failed {
            step: StepId::Install,
            reason: "no space left".into(),
        },
        s(41),
    ));
    let steps = fold(events);
    use StepState::*;
    assert_eq!(
        states(&steps),
        [Done, Done, Failed, Skipped, Skipped, Skipped]
    );
    assert_eq!(
        steps.rows()[2].note,
        Some(StepNote::Reason("no space left".into()))
    );
    assert_eq!(steps.rows()[2].at, Some(s(41)));
    assert!(
        steps.rows()[3..]
            .iter()
            .all(|r| r.note == Some(StepNote::NotNeeded))
    );
}

/// A cancel on the held stop marks that step cancelled and the later ones not needed; the
/// removed new core shows on the install step.
#[test]
fn cancelled_on_the_held_stop() {
    let mut events = first_three();
    events.push((
        mark(
            StepId::StopOld,
            StepState::Hold,
            StepNote::Refused { open_deals: None },
        ),
        s(67),
    ));
    events.push((
        mark(StepId::Install, StepState::Done, StepNote::Removed),
        s(90),
    ));
    events.push((StepEvent::Cancelled, s(91)));
    let steps = fold(events);
    use StepState::*;
    assert_eq!(
        states(&steps),
        [Done, Done, Done, Skipped, Skipped, Skipped]
    );
    assert_eq!(steps.rows()[2].note, Some(StepNote::Removed));
    assert_eq!(steps.rows()[3].note, Some(StepNote::Cancelled));
    assert_eq!(steps.rows()[4].note, Some(StepNote::NotNeeded));
}
