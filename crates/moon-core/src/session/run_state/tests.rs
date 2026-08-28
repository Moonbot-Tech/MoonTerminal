use super::*;

use crate::feed::{ConnStatus, RuntimeState};
use crate::session::store::CoreData;

/// Build a core in the given connection state with the given reported halves.
fn core(status: ConnStatus, runtime: Option<(bool, bool)>, trading: Option<bool>) -> CoreData {
    let mut data = CoreData::new();
    data.status = status;
    data.runtime_state = runtime.map(|(is_started, auto_detect_active)| RuntimeState {
        is_started,
        auto_detect_active,
    });
    data.runtime_state_confirmed = runtime.is_some();
    data.strategies_running = trading;
    data.strategies_running_confirmed = trading.is_some();
    data
}

/// A core that has reported both halves projects them unchanged.
#[test]
fn ready_core_reports_both_halves() {
    let state = CoreRunState::from_core(&core(ConnStatus::Ready, Some((true, false)), Some(true)));
    assert_eq!(
        state,
        CoreRunState {
            online: true,
            started: Some(true),
            started_confirmed: true,
            auto_detect: Some(false),
            trading: Some(true),
            trading_confirmed: true,
        }
    );
}

/// The two halves arrive over different commands, so one may be known while the other is not.
#[test]
fn halves_are_independent() {
    let only_runtime = CoreRunState::from_core(&core(ConnStatus::Ready, Some((true, true)), None));
    assert_eq!(only_runtime.started, Some(true));
    assert_eq!(only_runtime.trading, None);

    let only_trading = CoreRunState::from_core(&core(ConnStatus::Ready, None, Some(false)));
    assert_eq!(only_trading.started, None);
    assert_eq!(only_trading.trading, Some(false));
}

/// An unreachable core reports nothing, however much the store still retains for it.
#[test]
fn offline_core_reports_nothing() {
    for status in [
        ConnStatus::Connecting,
        ConnStatus::Stage("init".into()),
        ConnStatus::Disconnected,
        ConnStatus::Failed("boom".into()),
    ] {
        let state = CoreRunState::from_core(&core(status, Some((true, true)), Some(true)));
        assert_eq!(state, CoreRunState::default(), "stale state must not leak");
        assert!(!state.needs_restart());
    }
}

/// Restart is offered for a reported-stopped runtime only, never for an unknown one.
#[test]
fn restart_needs_a_reported_stop() {
    let stopped = CoreRunState::from_core(&core(ConnStatus::Ready, Some((false, false)), None));
    assert!(stopped.needs_restart());

    let unknown = CoreRunState::from_core(&core(ConnStatus::Ready, None, None));
    assert!(!unknown.needs_restart());

    let running = CoreRunState::from_core(&core(ConnStatus::Ready, Some((true, true)), None));
    assert!(!running.needs_restart());
}

/// Fold a set of already-projected states.
fn summary(states: &[(bool, Option<bool>, Option<bool>)]) -> RunSummary {
    RunSummary::fold(
        states
            .iter()
            .map(|(online, started, trading)| CoreRunState {
                online: *online,
                started: *started,
                started_confirmed: started.is_some(),
                auto_detect: None,
                trading: *trading,
                trading_confirmed: trading.is_some(),
            }),
    )
}

/// Everything trading offers Stop; nothing trading offers Start.
#[test]
fn uniform_scopes_offer_the_opposite_action() {
    let all_on = summary(&[
        (true, Some(true), Some(true)),
        (true, Some(true), Some(true)),
    ]);
    assert_eq!(all_on.trading_action(), TradingAction::Stop);
    assert!(!all_on.trading_mixed());

    let all_off = summary(&[
        (true, Some(true), Some(false)),
        (true, Some(true), Some(false)),
    ]);
    assert_eq!(all_off.trading_action(), TradingAction::Start);
}

/// A mixed scope offers Start: it finishes the job, while Stop would already be true for half.
#[test]
fn mixed_scope_offers_start() {
    let mixed = summary(&[
        (true, Some(true), Some(true)),
        (true, Some(true), Some(false)),
    ]);
    assert_eq!(mixed.trading_action(), TradingAction::Start);
    assert!(mixed.trading_mixed());
}

/// A core that reported nothing does not vote, but it also does not force Unknown on the scope.
#[test]
fn unknown_members_do_not_vote() {
    let with_unknown = summary(&[(true, Some(true), Some(true)), (true, None, None)]);
    assert_eq!(with_unknown.trading_action(), TradingAction::Stop);
    assert!(!with_unknown.trading_mixed());
}

/// A scope nobody reported for offers nothing to press — and neither does one nobody can reach.
#[test]
fn silent_scope_is_unknown() {
    let silent = summary(&[(true, None, None), (false, None, None)]);
    assert_eq!(silent.trading_action(), TradingAction::Unknown);
    assert_eq!(silent.online, 1);

    // Offline cores cannot report, so an unreachable scope reduces to the same answer.
    let offline = summary(&[(false, None, None), (false, None, None)]);
    assert_eq!(offline.trading_action(), TradingAction::Unknown);
    assert_eq!(offline.online, 0);
}

/// The stopped-runtime count is what a caption uses to offer a restart at all.
#[test]
fn stopped_counts_reported_stops_only() {
    let scope = summary(&[
        (true, Some(false), Some(false)),
        (true, Some(true), Some(true)),
        (true, None, None),
        (false, None, None),
    ]);
    assert_eq!(scope.online, 3);
    assert_eq!(scope.stopped, 1);
    assert_eq!(scope.started_on, 1);
    assert_eq!(scope.trading_on, 1);
    assert_eq!(scope.trading_off, 1);
}

/// A reconnected core keeps its values but stops claiming them, and the fold counts that.
///
/// Breakage: dropping the values instead leaves every core that survived a link blip reading as
/// "never reported" — the regression this pair of flags exists to prevent; counting them as fresh
/// instead makes a control state as fact something no live connection has confirmed.
#[test]
fn a_reconnected_core_reports_its_values_as_unconfirmed() {
    let mut data = core(ConnStatus::Ready, Some((true, true)), Some(true));
    data.runtime_state_confirmed = false;
    data.strategies_running_confirmed = false;

    let state = CoreRunState::from_core(&data);
    assert_eq!(state.started, Some(true));
    assert_eq!(state.trading, Some(true));
    assert!(!state.started_confirmed && !state.trading_confirmed);

    let folded = RunSummary::fold([state]);
    assert_eq!(
        folded.started_on, 1,
        "the value still counts as a reported start"
    );
    assert_eq!(folded.trading_on, 1);
    assert_eq!(
        folded.started_on_stale, 1,
        "but both halves are known to be stale"
    );
    assert_eq!(folded.trading_on_stale, 1);
    assert_eq!(
        folded.trading_action(),
        TradingAction::Stop,
        "an unconfirmed value still decides what the button offers — the alternative is a control that goes blank on every reconnect"
    );
}

/// An offline core reports nothing at all, confirmed or not.
#[test]
fn an_offline_core_carries_no_stale_values_either() {
    let mut data = core(ConnStatus::Disconnected, Some((true, true)), Some(true));
    data.runtime_state_confirmed = false;
    data.strategies_running_confirmed = false;

    let folded = RunSummary::fold([CoreRunState::from_core(&data)]);
    assert_eq!(folded.started_on_stale, 0);
    assert_eq!(folded.trading_on_stale, 0);
    assert_eq!(folded.trading_action(), TradingAction::Unknown);
}

/// The stale counters are split by VALUE, because a control fades the state it is drawing.
///
/// Breakage: one counter per half dims a confirmed "stopped" dot because an unrelated core in the
/// same group has a stale "started" — the fade would then describe a different core than the dot.
#[test]
fn staleness_is_counted_per_value_not_per_half() {
    let confirmed_stop = CoreRunState {
        online: true,
        started: Some(false),
        started_confirmed: true,
        auto_detect: None,
        trading: Some(false),
        trading_confirmed: true,
    };
    let stale_start = CoreRunState {
        online: true,
        started: Some(true),
        started_confirmed: false,
        auto_detect: None,
        trading: Some(true),
        trading_confirmed: false,
    };

    let folded = RunSummary::fold([confirmed_stop, stale_start]);
    assert_eq!(folded.stopped_stale, 0, "the stop was confirmed");
    assert_eq!(folded.started_on_stale, 1);
    assert_eq!(folded.trading_off_stale, 0);
    assert_eq!(folded.trading_on_stale, 1);
}

/// The per-direction counters name what a press would actually reach.
///
/// Breakage: showing `trading_off` on a Start button under-reports the cores commanded, because a
/// core that reported nothing is still connected and is still sent the command.
#[test]
fn the_needing_counters_count_what_a_press_reaches() {
    let scope = summary(&[
        (true, None, Some(false)),  // connected, stopped: start reaches it
        (true, None, Some(true)),   // connected, already trading: start skips it
        (true, None, None),         // connected, never reported: start still reaches it
        (false, None, Some(false)), // offline: neither reaches it
    ]);
    assert_eq!(scope.needing_start, 2);
    assert_eq!(scope.needing_stop, 2);
    assert_eq!(scope.online, 3);
}
