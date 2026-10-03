use super::*;

/// A connected core with both halves reported on this connection.
fn ready(trading: Option<bool>, auto_detect: Option<bool>) -> CoreRunState {
    CoreRunState {
        online: true,
        started: Some(true),
        started_confirmed: true,
        auto_detect,
        trading,
        trading_confirmed: true,
    }
}

/// An unreachable core is never commanded, whatever it last reported: a queued command would be
/// replayed whenever it comes back.
///
/// Mutation: drop the `online` check. Stop then reaches a disconnected core's channel and fires
/// on reconnect.
#[test]
fn offline_core_is_skipped_for_both_switches() {
    let offline = CoreRunState {
        online: false,
        ..ready(Some(true), Some(true))
    };
    for on in [true, false] {
        assert_eq!(RunSwitch::Trading.target(&offline, on), RunTarget::Offline);
        assert_eq!(
            RunSwitch::AutoDetect.target(&offline, on),
            RunTarget::Offline
        );
    }
}

/// A core already in the asked state on this connection is skipped; the opposite state is sent.
#[test]
fn confirmed_same_state_is_already() {
    let state = ready(Some(true), Some(false));
    assert_eq!(RunSwitch::Trading.target(&state, true), RunTarget::Already);
    assert_eq!(RunSwitch::Trading.target(&state, false), RunTarget::Send);
    assert_eq!(
        RunSwitch::AutoDetect.target(&state, false),
        RunTarget::Already
    );
    assert_eq!(RunSwitch::AutoDetect.target(&state, true), RunTarget::Send);
}

/// A value carried over from an earlier connection is not proof: the command is sent.
///
/// Mutation: drop the `*_confirmed` term. A reconnected core that has not re-reported would then
/// be skipped on the strength of a stale value.
#[test]
fn unconfirmed_same_state_is_sent() {
    let trading_stale = CoreRunState {
        trading_confirmed: false,
        ..ready(Some(true), None)
    };
    assert_eq!(
        RunSwitch::Trading.target(&trading_stale, true),
        RunTarget::Send
    );
    let runtime_stale = CoreRunState {
        started_confirmed: false,
        ..ready(None, Some(true))
    };
    assert_eq!(
        RunSwitch::AutoDetect.target(&runtime_stale, true),
        RunTarget::Send
    );
}

/// An unknown half is sent rather than taken as the asked state.
#[test]
fn unknown_state_is_sent() {
    let state = ready(None, None);
    assert_eq!(RunSwitch::Trading.target(&state, false), RunTarget::Send);
    assert_eq!(RunSwitch::AutoDetect.target(&state, false), RunTarget::Send);
}

/// Refused is what needed the command but did not get it through.
#[test]
fn refused_counts_needed_minus_sent() {
    let outcome = RunDispatch {
        sent: vec![1],
        needed: 3,
        offline: 2,
        already: 1,
    };
    assert_eq!(outcome.refused(), 2);
    assert_eq!(RunDispatch::default().refused(), 0);
}
