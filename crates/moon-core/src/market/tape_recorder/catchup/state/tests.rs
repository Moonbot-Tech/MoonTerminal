//! Exercise recorder switch generations and pass transitions without starting a fetch thread.

use super::*;
use crate::market::tape_recorder::catchup::{self, tests::SWITCH_LOCK};
use crate::venue::{Brand, MarketKind, Venue};

const DAY: i64 = 86_400_000;
const NOW: i64 = 500 * DAY;

/// Restore global choices and progress even when an assertion unwinds.
struct Restore {
    status: crate::station_api::TapeCatchup,
}

impl Restore {
    /// Capture the prior process-wide configuration under the test serialization lock.
    fn new() -> Self {
        Self {
            status: catchup::status(),
        }
    }
}

impl Drop for Restore {
    /// Keep later recorder tests independent of switch and status changes in this fixture.
    fn drop(&mut self) {
        catchup::set_switches(self.status.auto.unwrap(), self.status.archives.unwrap());
        *STATUS.lock().unwrap() = self.status.clone();
    }
}

/// A synthetic archive-only closed trade is old enough to leave the live recorder tail.
fn row() -> CatchupTrade {
    CatchupTrade {
        id: TradeId {
            core: 7,
            rec_id: 91,
        },
        exchange: "synthetic".into(),
        market: "SYNUSDT".into(),
        venue: Venue {
            brand: Brand::Bybit,
            kind: MarketKind::Futures,
        },
        open_ms: NOW - DAY - 60_000,
        close_ms: NOW - DAY,
    }
}

/// Leaving permanent or complete identities latched on a switch change would hide eligible tape.
#[test]
fn archive_switch_change_replans_refused_and_completed_rows() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    catchup::set_switches(true, false);
    let trade = row();
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    let refused = state.prepare(|_| Coverage::none(), NOW, 0, DAY, false);
    assert_eq!(refused.no_route, 1);
    assert!(refused.jobs.is_empty());
    state.submit(vec![trade.clone()]);
    assert!(
        state
            .prepare(|_| panic!("latched row asked disk"), NOW, 0, DAY, false)
            .jobs
            .is_empty()
    );
    catchup::set_switches(true, true);
    state.submit(vec![trade.clone()]);
    let planned = state.prepare(|_| Coverage::none(), NOW, 0, DAY, true);
    assert_eq!(planned.jobs.len(), 1);
    assert!(matches!(
        planned.jobs[0].source,
        catchup::Source::Archive(_)
    ));
    assert_eq!(
        state
            .prepare(|trade| catchup::needed(trade, 0, DAY), NOW, 0, DAY, true)
            .complete,
        1
    );
    // An identical reload must not invalidate completed tape.
    catchup::set_switches(true, true);
    state.submit(vec![trade.clone()]);
    assert!(
        state
            .prepare(
                |_| panic!("unchanged generation rechecked tape"),
                NOW,
                0,
                DAY,
                true
            )
            .jobs
            .is_empty()
    );
    catchup::set_switches(false, true);
    state.submit(vec![trade]);
    assert_eq!(
        state
            .prepare(|_| Coverage::none(), NOW, 0, DAY, true)
            .jobs
            .len(),
        1
    );
}

/// A tick's archive choice reaches planning as given, even when the live switches say otherwise.
#[test]
fn prepare_plans_with_the_archive_choice_it_is_handed() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    catchup::set_switches(true, true);
    let mut state = RecorderCatchup::default();
    state.submit(vec![row()]);
    let plan = state.prepare(|_| Coverage::none(), NOW, 0, DAY, false);
    assert_eq!(
        plan.no_route, 1,
        "archives off in the snapshot leaves no source"
    );
    assert!(plan.jobs.is_empty());
}

/// Automatic recovery off must keep queued rows; a dispatched pass stays running until settlement.
#[test]
fn auto_off_queues_without_dispatch_and_running_tracks_pass_settlement() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    catchup::set_switches(false, true);
    let cache = TradeCache::open_evictable(std::path::PathBuf::from(":memory:")).unwrap();
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    let (sender, receiver) = mpsc::channel();
    let mut state = RecorderCatchup {
        worker: Some(sender),
        ..RecorderCatchup::default()
    };
    state.submit(vec![row()]);
    state.tick(&cache, NOW, 0, DAY);
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(!catchup::status().running);
    assert_eq!(catchup::status().pending, 1);
    catchup::set_switches(true, true);
    state.tick(&cache, NOW, 0, DAY);
    let pass = receiver
        .try_recv()
        .expect("queued trade dispatched after enabling auto");
    assert_eq!(pass.jobs.len(), 1);
    assert!(catchup::status().running);
    assert_eq!(catchup::status().last_pass_ms, Some(NOW));
    state.tick(&cache, NOW + 1, 0, DAY);
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    // Reload while running: the old pass may settle, but its permanent refusal must not survive.
    catchup::set_switches(false, false);
    state.tick(&cache, NOW + 2, 0, DAY);
    assert!(catchup::status().running);
    state.finish(
        &cache,
        PassResult {
            finished: pass
                .jobs
                .into_iter()
                .map(|job| executor::FinishedJob {
                    job,
                    failure: Some(Failure::Permanent),
                    attempted: true,
                })
                .collect(),
            archive_bytes: 0,
        },
        NOW + 3,
    );
    assert!(!catchup::status().running);
    assert!(state.latched.is_empty());
}
