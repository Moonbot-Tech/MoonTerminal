//! Exercise recorder switch generations and pass transitions without starting a fetch thread.

use super::*;
use crate::market::tape_recorder::catchup::{self, tests::SWITCH_LOCK};
use crate::venue::{Brand, MarketKind, Venue};

const DAY: i64 = 86_400_000;
const NOW: i64 = 500 * DAY;

/// Restore global choices and progress even when an assertion unwinds.
struct Restore {
    status: crate::station_api::TapeCatchup,
    margin_s: u32,
    long_min: u32,
}

impl Restore {
    /// Capture the prior process-wide configuration under the test serialization lock.
    fn new() -> Self {
        Self {
            status: catchup::status(),
            margin_s: (crate::market::trade_replay::margin_ms() / 1000) as u32,
            long_min: (crate::market::trade_replay::long_position_ms() / 60_000) as u32,
        }
    }
}

impl Drop for Restore {
    /// Keep later recorder tests independent of switch and status changes in this fixture.
    fn drop(&mut self) {
        catchup::set_switches(self.status.auto.unwrap(), self.status.archives.unwrap());
        crate::market::trade_replay::set_margin_s(self.margin_s);
        crate::market::trade_replay::set_long_position_min(self.long_min);
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

/// Widening must fetch only the newly missing margin, identical snapped values must retain
/// settlement, and shrinking must preserve held tape without requesting anything new.
#[test]
fn margin_changes_replan_settled_rows_without_deleting_held_tape() {
    use crate::market::trade_replay::{margin_ms, set_margin_s};
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    set_margin_s(30);
    let trade = row();
    let held = Coverage::one((trade.open_ms - 30_000, trade.close_ms + 30_000));
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    assert_eq!(
        state
            .prepare(|_| held.clone(), NOW, margin_ms(), DAY, true)
            .complete,
        1
    );
    set_margin_s(31);
    state.submit(vec![trade.clone()]);
    assert!(
        state
            .prepare(
                |_| panic!("same snapped value rechecked disk"),
                NOW,
                margin_ms(),
                DAY,
                true
            )
            .jobs
            .is_empty()
    );
    set_margin_s(60);
    state.submit(vec![trade.clone()]);
    let plan = state.prepare(|_| held.clone(), NOW, margin_ms(), DAY, true);
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(
        plan.jobs[0].missing,
        Coverage::from_spans([
            (trade.open_ms - 60_000, trade.open_ms - 30_001),
            (trade.close_ms + 30_001, trade.close_ms + 60_000),
        ])
    );
    let wider = Coverage::one((trade.open_ms - 60_000, trade.close_ms + 60_000));
    assert_eq!(
        state
            .prepare(|_| wider.clone(), NOW, margin_ms(), DAY, true)
            .complete,
        1
    );
    set_margin_s(30);
    state.submit(vec![trade]);
    let plan = state.prepare(|_| wider.clone(), NOW, margin_ms(), DAY, true);
    assert_eq!(plan.complete, 1);
    assert!(plan.jobs.is_empty());
}

/// Permanent request failures must leave the queue and remain latched on later enumeration.
#[test]
fn settled_request_fault_is_not_retried_on_the_next_hour() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let trade = row();
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    let job = state
        .prepare(|_| Coverage::none(), NOW, 0, DAY, true)
        .jobs
        .remove(0);
    let cache = TradeCache::open_evictable(std::path::PathBuf::from(":memory:")).unwrap();
    state.finish(
        &cache,
        PassResult {
            finished: vec![executor::FinishedJob {
                job,
                failure: Some(Failure::Permanent),
                attempted: true,
                fault: Some(Fault {
                    reason: CatchupFailReason::RequestFault,
                    detail: "request fault: HTTP 400".into(),
                }),
            }],
            archive_bytes: 0,
        },
        NOW,
    );
    assert_eq!(catchup::status().failed, 1);
    assert_eq!(
        catchup::status().failed_by,
        vec![(CatchupFailReason::RequestFault, 1)]
    );
    assert_eq!(catchup::status().pending, 0);
    state.submit(vec![trade]);
    assert!(
        state
            .prepare(
                |_| panic!("permanent request fault retried"),
                NOW + HOUR_MS,
                0,
                DAY,
                true
            )
            .jobs
            .is_empty()
    );
}

/// Raising the long-position threshold must revisit a settled pair of ends and fetch the
/// middle; reloading the same clamped threshold must not revisit it again.
#[test]
fn long_threshold_changes_replan_only_missing_middle() {
    use crate::market::trade_replay::{long_position_ms, set_long_position_min};
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    set_long_position_min(60);
    let mut trade = row();
    trade.open_ms = trade.close_ms - 90 * 60_000;
    let held = Coverage::from_spans([
        (trade.open_ms, trade.open_ms),
        (trade.close_ms, trade.close_ms),
    ]);
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    assert_eq!(
        state
            .prepare(|_| held.clone(), NOW, 0, long_position_ms(), true)
            .complete,
        1
    );
    set_long_position_min(60);
    state.submit(vec![trade.clone()]);
    assert!(
        state
            .prepare(
                |_| panic!("same threshold rechecked disk"),
                NOW,
                0,
                long_position_ms(),
                true
            )
            .jobs
            .is_empty()
    );
    set_long_position_min(120);
    state.submit(vec![trade.clone()]);
    let plan = state.prepare(|_| held.clone(), NOW, 0, long_position_ms(), true);
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(
        plan.jobs[0].missing,
        Coverage::one((trade.open_ms + 1, trade.close_ms - 1))
    );
    assert_eq!(
        state
            .prepare(
                |_| Coverage::one((trade.open_ms, trade.close_ms)),
                NOW,
                0,
                long_position_ms(),
                true
            )
            .complete,
        1
    );
    set_long_position_min(999);
    state.submit(vec![trade]);
    assert!(
        state
            .prepare(
                |_| panic!("same clamped threshold rechecked disk"),
                NOW,
                0,
                long_position_ms(),
                true
            )
            .jobs
            .is_empty()
    );
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
                    fault: None,
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

/// Thousands of failures write one summary and a few warn lines per reason, never one per job.
#[test]
fn pass_report_counts_every_failure_but_logs_a_bounded_few() {
    use crate::station_api::CatchupFailReason as R;
    let trade = row();
    let job = CatchupJob {
        missing: Coverage::one((trade.open_ms, trade.close_ms)),
        expires_at_ms: i64::MAX,
        source: catchup::Source::Archive(
            crate::market::trade_replay::venue_caps::ArchiveRoute::BybitTradingDaily,
        ),
        trade,
    };
    let fault = |reason, detail: &str| {
        Some(Fault {
            reason,
            detail: detail.into(),
        })
    };
    let mut failures = vec![(job.clone(), fault(R::Http, "binance HTTP 503")); 1000];
    failures.extend(vec![
        (job.clone(), fault(R::Network, "timed out\u{2014}x"));
        3
    ]);
    failures.push((job, None));
    let report = pass_report(7, 40, &failures);
    assert_eq!(
        report.counts,
        vec![(R::Http, 1000), (R::Network, 3), (R::Other, 1)]
    );
    assert_eq!(
        report.summary,
        "catch-up pass: filled 7, failed 1004 (http: 1000, network: 3, other: 1), pending 40"
    );
    assert_eq!(report.warns.len(), WARN_PER_REASON + 3 + 1);
    assert!(report.warns.iter().all(|line| line.is_ascii()));
    assert!(report.warns[0].contains("synthetic SYNUSDT"));
    assert!(report.warns[0].ends_with("binance HTTP 503"));
    assert_eq!(
        pass_report(2, 0, &[]).summary,
        "catch-up pass: filled 2, failed 0, pending 0"
    );
}

/// Tape fetched in full but refused by the disk reserve at filing must count as `no_room`.
#[test]
fn reserve_refused_filing_counts_as_no_room() {
    use crate::station_api::CatchupFailReason as R;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let cache = TradeCache::open_evictable(std::path::PathBuf::from(":memory:")).unwrap();
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    let trade = row();
    let job = CatchupJob {
        missing: Coverage::one((trade.open_ms, trade.close_ms)),
        expires_at_ms: i64::MAX,
        source: catchup::Source::Archive(
            crate::market::trade_replay::venue_caps::ArchiveRoute::BybitTradingDaily,
        ),
        trade: trade.clone(),
    };
    let mut state = RecorderCatchup::default();
    state.filing_refused(trade.exchange.clone(), trade.market.clone());
    state.finish(
        &cache,
        PassResult {
            finished: vec![executor::FinishedJob {
                job,
                failure: None,
                fault: None,
                attempted: true,
            }],
            archive_bytes: 0,
        },
        NOW,
    );
    assert_eq!(catchup::status().failed_by, vec![(R::NoRoom, 1)]);
    assert!(state.refused.is_empty());
}

/// A backlog larger than one pass drains in bounded chunks, plan order first, untaken rows free.
#[test]
fn backlog_drains_in_bounded_passes_that_each_move_the_status() {
    use crate::station_api::CatchupFailReason as R;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    catchup::set_switches(true, true);
    let cache = TradeCache::open_evictable(std::path::PathBuf::from(":memory:")).unwrap();
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    let (sender, receiver) = mpsc::channel();
    let mut state = RecorderCatchup {
        worker: Some(sender),
        ..RecorderCatchup::default()
    };
    let total = PASS_JOBS * 2 + 50;
    let rows: Vec<_> = (0..total)
        .map(|i| {
            let mut trade = row();
            trade.id.rec_id = i as i64;
            trade.close_ms -= i as i64 * 60_000;
            trade.open_ms = trade.close_ms - 60_000;
            trade
        })
        .collect();
    state.submit(rows);
    let mut taken = Vec::new();
    for (chunk, (expected, fault)) in [
        (PASS_JOBS, None),
        (
            PASS_JOBS,
            Some(Fault {
                reason: R::Http,
                detail: "synthetic HTTP 503".into(),
            }),
        ),
        (50, None),
    ]
    .into_iter()
    .enumerate()
    {
        let now = NOW + chunk as i64;
        state.tick(&cache, now, 0, DAY);
        let pass = receiver
            .try_recv()
            .expect("pending work starts the next pass at once");
        assert_eq!(pass.jobs.len(), expected);
        assert!(catchup::status().running);
        // Rows not taken stay pending and unlatched, so the next tick may take them.
        for trade in state.pending.values() {
            let in_pass = pass.jobs.iter().any(|job| job.trade.id == trade.id);
            let settled = taken.contains(&trade.id.rec_id);
            assert!(in_pass || settled || !state.latched.contains_key(&trade.id));
        }
        taken.extend(pass.jobs.iter().map(|job| job.trade.id.rec_id));
        let failures: Vec<_> = pass
            .jobs
            .iter()
            .map(|job| (job.clone(), fault.clone()))
            .collect();
        assert!(pass_report(0, 0, &failures).warns.len() <= WARN_PER_REASON);
        state.finish(
            &cache,
            PassResult {
                finished: pass
                    .jobs
                    .into_iter()
                    .map(|job| executor::FinishedJob {
                        job,
                        failure: None,
                        fault: fault.clone(),
                        attempted: true,
                    })
                    .collect(),
                archive_bytes: 0,
            },
            now,
        );
        let status = catchup::status();
        assert!(!status.running);
        assert_eq!(
            status.failed, expected as u32,
            "failed describes the last chunk"
        );
        let reason = fault.as_ref().map_or(R::Other, |f| f.reason);
        assert_eq!(status.failed_by, vec![(reason, expected as u32)]);
        assert_eq!(status.pending, total as u32);
    }
    // Archive-only rows tie on expiry, so the plan takes the newest close first.
    assert_eq!(taken, (0..total as i64).collect::<Vec<_>>());
    state.tick(&cache, NOW + 10, 0, DAY);
    assert!(
        receiver.try_recv().is_err(),
        "retries wait for the next hour"
    );
}

/// A spent hourly archive budget keeps archive jobs out of passes until the hour rolls, while
/// REST-able jobs still go out and nothing spins on work no pass could do.
#[test]
fn spent_archive_budget_holds_archive_jobs_until_the_hour_rolls() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    catchup::set_switches(true, true);
    let cache = TradeCache::open_evictable(std::path::PathBuf::from(":memory:")).unwrap();
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    let (sender, receiver) = mpsc::channel();
    let mut state = RecorderCatchup {
        worker: Some(sender),
        hour: Some(NOW.div_euclid(HOUR_MS)),
        archive_bytes: executor::ARCHIVE_BYTES_PER_PASS,
        ..RecorderCatchup::default()
    };
    let archive_row = row();
    let mut rest_row = row();
    rest_row.id.rec_id = 92;
    rest_row.venue = Venue {
        brand: Brand::Gate,
        kind: MarketKind::Spot,
    };
    state.submit(vec![archive_row.clone(), rest_row.clone()]);

    state.tick(&cache, NOW, 0, DAY);
    let pass = receiver.try_recv().expect("REST work still starts a pass");
    let ids: Vec<_> = pass.jobs.iter().map(|job| job.trade.id).collect();
    assert_eq!(ids, vec![rest_row.id], "archive job left out of the pass");
    assert!(!state.latched.contains_key(&archive_row.id));
    state.finish(
        &cache,
        PassResult {
            finished: pass
                .jobs
                .into_iter()
                .map(|job| executor::FinishedJob {
                    job,
                    failure: Some(executor::Failure::Retry),
                    fault: None,
                    attempted: true,
                })
                .collect(),
            archive_bytes: executor::ARCHIVE_BYTES_PER_PASS,
        },
        NOW,
    );

    state.tick(&cache, NOW + 1, 0, DAY);
    assert!(
        receiver.try_recv().is_err(),
        "no pass starts for archive work alone while the budget is spent"
    );
    assert!(state.pending.contains_key(&archive_row.id));
    assert!(
        !state.latched.contains_key(&archive_row.id),
        "a held job is not failed"
    );
    assert!(!state.active);

    let next_hour = (NOW.div_euclid(HOUR_MS) + 1) * HOUR_MS;
    state.tick(&cache, next_hour, 0, DAY);
    let pass = receiver
        .try_recv()
        .expect("the hour roll re-admits archive jobs");
    assert!(pass.jobs.iter().any(|job| job.trade.id == archive_row.id));
    assert_eq!(state.archive_bytes, 0);
}

/// Selection keeps plan order and the pass size, filtering archive work only when spent.
#[test]
fn admissible_filters_archive_jobs_only_once_the_budget_is_spent() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let rows: Vec<_> = (0..PASS_JOBS + 5)
        .map(|i| {
            let mut trade = row();
            trade.id.rec_id = i as i64;
            trade
        })
        .collect();
    let jobs = catchup::plan(&rows, |_| Coverage::none(), &HashMap::new(), NOW, 0, DAY).jobs;
    assert_eq!(admissible(jobs.clone(), 0, NOW).len(), PASS_JOBS);
    assert!(admissible(jobs, executor::ARCHIVE_BYTES_PER_PASS, NOW).is_empty());
}
