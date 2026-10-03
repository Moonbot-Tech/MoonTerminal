//! Pins for the down/back machine. Each test names the edit that would break it.

use std::collections::BTreeMap;

use moon_core::feed::ConnStatus;
use moon_core::telegram::notify::{DownRule, NotifyLedger};

use super::*;

fn rule() -> DownRule {
    DownRule {
        on: true,
        after_minutes: 5,
    }
}

/// Resetting the loss timer on every sample, or deleting it on `Up`, would
/// either spam a flapping core or announce a down that never lasted the delay.
#[test]
fn flapping_under_the_delay_says_nothing() {
    let rule = rule();
    let mut tracker = DownTracker::default();
    let mut ledger = NotifyLedger::default();
    let core = 7;
    let t0 = 5_000_000_i64;
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Up)], t0)
            .is_empty()
    );
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Lost)], t0)
            .is_empty()
    );
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Lost)], t0 + 200)
            .is_empty()
    );
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Up)], t0 + 200)
            .is_empty()
    );
    assert!(tracker.lost_since.is_empty());
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Lost)], t0 + 250)
            .is_empty()
    );
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Lost)], t0 + 250 + 299)
            .is_empty()
    );
    assert!(ledger.down_announced.is_empty());
    assert_eq!(tracker.lost_since.get(&core).copied(), Some(t0 + 250));
}

/// Emitting `Down` again while the core stays lost, or `Back` without a prior
/// down, would double-notify one outage or invent a recovery.
#[test]
fn one_outage_announces_down_then_back_once() {
    let rule = rule();
    let mut tracker = DownTracker::default();
    let mut ledger = NotifyLedger::default();
    let core = 7;
    let t0 = 1_700_000_000_i64;
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Lost)], t0)
            .is_empty()
    );
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Lost)], t0 + 299)
            .is_empty()
    );
    assert_eq!(
        tracker.step(&rule, &mut ledger, &[(core, Link::Lost)], t0 + 300),
        vec![DownEvent::Down {
            core,
            since_utc: t0
        }]
    );
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Lost)], t0 + 301)
            .is_empty()
    );
    assert_eq!(
        tracker.step(&rule, &mut ledger, &[(core, Link::Up)], t0 + 400),
        vec![DownEvent::Back {
            core,
            down_for_secs: 400,
        }]
    );
    assert!(ledger.down_announced.is_empty());
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(core, Link::Up)], t0 + 401)
            .is_empty()
    );
}

/// Starting the timer in the past after a restart would send a second down
/// notice immediately. Coming back must still send the one recovery, with a
/// zero duration when the loss time was not kept.
#[test]
fn restart_emits_back_without_a_second_down() {
    let rule = rule();
    let now = 8_000_000_i64;

    let mut ledger = NotifyLedger::default();
    ledger.down_announced.insert(7);
    let mut tracker = DownTracker::default();
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(7, Link::Lost)], now)
            .is_empty()
    );
    assert!(ledger.down_announced.contains(&7));
    assert_eq!(tracker.lost_since.get(&7).copied(), Some(now));

    let mut ledger = NotifyLedger::default();
    ledger.down_announced.insert(7);
    let mut tracker = DownTracker::default();
    assert_eq!(
        tracker.step(&rule, &mut ledger, &[(7, Link::Up)], now),
        vec![DownEvent::Back {
            core: 7,
            down_for_secs: 0,
        }]
    );
    assert!(ledger.down_announced.is_empty());
}

/// Treating `Connecting` as a down notice, or as an up link, would either page
/// the chat during startup or forget that the core is not up yet.
#[test]
fn startup_connecting_shorter_than_the_delay_says_nothing() {
    let link = link_of(&ConnStatus::Connecting).expect("connecting maps to a link");
    assert_eq!(link, Link::Lost);
    let rule = rule();
    let mut tracker = DownTracker::default();
    let mut ledger = NotifyLedger::default();
    let now = 1_700_000_000_i64;
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(3, link)], now)
            .is_empty()
    );
    assert_eq!(tracker.lost_since.get(&3).copied(), Some(now));
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(3, link)], now + 5 * 60 - 1)
            .is_empty()
    );
    assert!(!ledger.down_announced.contains(&3));
}

/// `Ready` is the up variant. The feed enum names it `Ready`, not `Connected`.
/// Every other variant is a lost link. There is no inactive status to map to `None`.
#[test]
fn link_of_maps_ready_to_up_and_the_rest_to_lost() {
    assert_eq!(link_of(&ConnStatus::Ready), Some(Link::Up));
    assert_eq!(link_of(&ConnStatus::Connecting), Some(Link::Lost));
    assert_eq!(
        link_of(&ConnStatus::Stage("sync".to_string())),
        Some(Link::Lost)
    );
    assert_eq!(
        link_of(&ConnStatus::Failed("down".to_string())),
        Some(Link::Lost)
    );
    assert_eq!(link_of(&ConnStatus::Disconnected), Some(Link::Lost));
}

/// Leaving the announced set or the timer in place while the rule is off would
/// send a stale recovery the next time the user turns notices on.
#[test]
fn a_disabled_rule_clears_down_state_and_sends_nothing() {
    let rule = DownRule {
        on: false,
        after_minutes: 5,
    };
    let mut tracker = DownTracker {
        lost_since: BTreeMap::from([(7, 1)]),
    };
    let mut ledger = NotifyLedger::default();
    ledger.down_announced.insert(7);
    let events = tracker.step(&rule, &mut ledger, &[(7, Link::Lost)], 10_000);
    assert!(events.is_empty());
    assert!(ledger.down_announced.is_empty());
    assert!(tracker.lost_since.is_empty());
}

/// Keeping `down_announced` for a core that left the observation would suppress
/// its next down forever, and emitting `Back` would invent a recovery. Coming
/// back as lost must start the delay from that new observation.
#[test]
fn an_announced_core_that_leaves_the_links_is_forgotten() {
    let rule = rule();
    let mut tracker = DownTracker::default();
    let mut ledger = NotifyLedger::default();
    let gone = 7_u64;
    let stays = 9_u64;
    let t0 = 1_700_000_000_i64;

    assert!(
        tracker
            .step(&rule, &mut ledger, &[(gone, Link::Lost)], t0)
            .is_empty()
    );
    assert_eq!(
        tracker.step(&rule, &mut ledger, &[(gone, Link::Lost)], t0 + 300),
        vec![DownEvent::Down {
            core: gone,
            since_utc: t0,
        }]
    );
    assert!(
        tracker
            .step(
                &rule,
                &mut ledger,
                &[(gone, Link::Lost), (stays, Link::Lost)],
                t0 + 300,
            )
            .is_empty()
    );
    assert!(ledger.down_announced.contains(&gone));
    assert!(!ledger.down_announced.contains(&stays));
    assert_eq!(tracker.lost_since.get(&stays).copied(), Some(t0 + 300));

    let left = tracker.step(&rule, &mut ledger, &[(stays, Link::Lost)], t0 + 310);
    assert!(
        left.is_empty(),
        "a core that disappeared must not emit Back"
    );
    assert!(!ledger.down_announced.contains(&gone));
    assert!(!tracker.lost_since.contains_key(&gone));
    assert_eq!(tracker.lost_since.get(&stays).copied(), Some(t0 + 300));

    let again = tracker.step(&rule, &mut ledger, &[(gone, Link::Lost)], t0 + 310);
    assert!(
        again.is_empty(),
        "a fresh loss timer must not announce immediately"
    );
    assert_eq!(tracker.lost_since.get(&gone).copied(), Some(t0 + 310));
    assert!(!ledger.down_announced.contains(&gone));
    assert!(
        tracker
            .step(&rule, &mut ledger, &[(gone, Link::Lost)], t0 + 310 + 299)
            .is_empty()
    );
    assert_eq!(
        tracker.step(&rule, &mut ledger, &[(gone, Link::Lost)], t0 + 610),
        vec![DownEvent::Down {
            core: gone,
            since_utc: t0 + 310,
        }]
    );
}
