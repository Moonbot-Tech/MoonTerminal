use super::*;

/// A live action queued while the connection is down is not delivered.
///
/// Mutation: deliver when `ready_since` is `None`. A Stop pressed during an outage then waits in
/// MoonProto and fires on the next Ready.
#[test]
fn not_ready_drops() {
    let now = Instant::now();
    assert_eq!(staleness(now, None, now), Some(Stale::NotReady));
}

/// A live action queued before the connection last became operational waited out an outage.
///
/// Mutation: drop the `queued_at < since` check. A command queued an hour ago, during the outage,
/// is delivered the moment the core is back.
#[test]
fn queued_before_ready_drops() {
    let queued = Instant::now();
    let since = queued + Duration::from_secs(1);
    assert_eq!(
        staleness(queued, Some(since), since),
        Some(Stale::BeforeReady)
    );
}

/// A fresh action on an operational connection is delivered; one past the limit is not.
#[test]
fn fresh_delivers_and_old_drops() {
    let since = Instant::now();
    let queued = since + Duration::from_millis(10);
    assert_eq!(staleness(queued, Some(since), queued), None);
    assert_eq!(
        staleness(queued, Some(since), queued + LIVE_TTL),
        None,
        "exactly the limit is still delivered"
    );
    assert_eq!(
        staleness(
            queued,
            Some(since),
            queued + LIVE_TTL + Duration::from_millis(1)
        ),
        Some(Stale::TooOld)
    );
}

/// Trading actions are live; settings and strategy edits are desired state.
#[test]
fn classification_separates_actions_from_state() {
    assert!(is_live_action(&CoreCmd::CancelAllOrders));
    assert!(is_live_action(&CoreCmd::SetAutoDetect(false)));
    assert!(is_live_action(&CoreCmd::PanicSellMarket {
        market: "BTCUSDT".into(),
        on: true,
    }));
    assert!(is_live_action(&CoreCmd::StrategiesAction {
        checks: Vec::new(),
        start_stop: Some(false),
    }));
    assert!(!is_live_action(&CoreCmd::StrategiesAction {
        checks: vec![(1, true)],
        start_stop: None,
    }));
    assert!(!is_live_action(&CoreCmd::SetBlacklist {
        on: true,
        text: "BTC".into(),
    }));
    assert!(!is_live_action(&CoreCmd::RefreshProblems));
}

/// A stale Start/Stop is dropped, but the checkbox edits it carried are still delivered.
///
/// Mutation: drop the whole action. The strategy checkboxes the user changed then never reach the
/// core, and the terminal's view diverges from it.
#[test]
fn stale_strategies_action_keeps_its_checks() {
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(QueuedCmd {
        at: Instant::now(),
        cmd: CoreCmd::StrategiesAction {
            checks: vec![(7, true)],
            start_stop: Some(true),
        },
    })
    .unwrap();
    match recv_fresh(&rx, None, 1) {
        Ok(CoreCmd::StrategiesAction { checks, start_stop }) => {
            assert_eq!(checks, vec![(7, true)]);
            assert_eq!(start_stop, None);
        }
        other => panic!("expected the checks alone, got {other:?}"),
    }
}

/// The queue drops stale live actions and hands over everything else in order.
///
/// Mutation: return the first command unconditionally. The Stop queued during the outage is then
/// delivered after the connection came back.
#[test]
fn recv_fresh_skips_stale_live_actions_only() {
    let (tx, rx) = std::sync::mpsc::channel();
    let outage = Instant::now();
    let since = outage + Duration::from_secs(5);
    tx.send(QueuedCmd {
        at: outage,
        cmd: CoreCmd::SetAutoDetect(false),
    })
    .unwrap();
    tx.send(QueuedCmd {
        at: outage,
        cmd: CoreCmd::RefreshProblems,
    })
    .unwrap();
    tx.send(QueuedCmd {
        at: Instant::now().max(since),
        cmd: CoreCmd::CancelAllOrders,
    })
    .unwrap();
    assert!(matches!(
        recv_fresh(&rx, Some(since), 1),
        Ok(CoreCmd::RefreshProblems)
    ));
    assert!(matches!(
        recv_fresh(&rx, Some(since), 1),
        Ok(CoreCmd::CancelAllOrders)
    ));
    assert!(matches!(
        recv_fresh(&rx, Some(since), 1),
        Err(TryRecvError::Empty)
    ));
}
