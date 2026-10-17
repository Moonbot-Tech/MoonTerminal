//! Exercise recorder switch generations and pass transitions without starting a fetch thread.

use super::*;
use crate::market::tape_recorder::catchup::{self, tests::SWITCH_LOCK};
use crate::venue::{Brand, MarketKind, Venue};
use std::sync::Arc;

/// Retrying NotShrinkable every minute would rewrite tape forever without changing its file mode.
#[test]
fn review_only_transient_compaction_failures_retry() {
    use super::super::super::retention::Phase;
    use crate::market::trade_replay::trade_cache::CompactFail;
    let _lock = SWITCH_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(0, true);
    catchup::take_purge_wanted();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    for failure in [
        CompactFail::NotShrinkable,
        CompactFail::NoRoom,
        CompactFail::Deferred,
        CompactFail::Other,
    ] {
        let mut state = RecorderCatchup::default();
        state.retention.phase = Phase::Recount {
            reads: vec![],
            complete: HashSet::new(),
            failure: Some(failure),
            freed: 0,
        };
        assert!(!state.retention_tick(
            &cache,
            NOW,
            &HashMap::new(),
            &catchup::Shape {
                margin_ms: crate::market::trade_replay::margin_ms(),
                long_ms: crate::market::trade_replay::long_position_ms(),
                tail_ms: crate::market::trade_replay::margin_ms()
            }
        ));
        if failure == CompactFail::NotShrinkable {
            assert_eq!(state.retention.retry_at, None);
            assert!(!state.retention_tick(
                &cache,
                NOW + 60_000,
                &HashMap::new(),
                &catchup::Shape {
                    margin_ms: crate::market::trade_replay::margin_ms(),
                    long_ms: crate::market::trade_replay::long_position_ms(),
                    tail_ms: crate::market::trade_replay::margin_ms()
                }
            ));
            catchup::set_keep(0, false);
            assert!(
                state.retention_tick(
                    &cache,
                    NOW + 60_001,
                    &HashMap::new(),
                    &catchup::Shape {
                        margin_ms: crate::market::trade_replay::margin_ms(),
                        long_ms: crate::market::trade_replay::long_position_ms(),
                        tail_ms: crate::market::trade_replay::margin_ms()
                    }
                ),
                "a new purge must still run"
            );
            catchup::set_keep(0, true);
            catchup::take_purge_wanted();
        } else {
            assert_eq!(state.retention.retry_at, Some(NOW + 60_000));
            assert!(!state.retention_tick(
                &cache,
                NOW + 59_999,
                &HashMap::new(),
                &catchup::Shape {
                    margin_ms: crate::market::trade_replay::margin_ms(),
                    long_ms: crate::market::trade_replay::long_position_ms(),
                    tail_ms: crate::market::trade_replay::margin_ms()
                }
            ));
            assert!(state.retention_tick(
                &cache,
                NOW + 60_000,
                &HashMap::new(),
                &catchup::Shape {
                    margin_ms: crate::market::trade_replay::margin_ms(),
                    long_ms: crate::market::trade_replay::long_position_ms(),
                    tail_ms: crate::market::trade_replay::margin_ms()
                }
            ));
        }
    }
}

/// Without the station guard, every terminal close accumulates a permanent protection row.
#[test]
fn review_terminal_closes_do_not_accumulate_retention_rows() {
    use crate::market::tape_recorder::{Cmd, Recorder, Stats, peek};
    use std::sync::atomic::Ordering;
    let _lock = SWITCH_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    let now = std::time::Instant::now();
    let mut recorder = Recorder {
        cache: TradeCache::open_evictable(":memory:".into()).unwrap(),
        catchup: RecorderCatchup::default(),
        tasks: HashMap::new(),
        donors: HashMap::new(),
        shunned: HashMap::new(),
        stats: Stats::default(),
        last_summary: now,
        next_drain: now,
        replies: Vec::new(),
        ticks: Vec::new(),
        compares: Vec::new(),
        closed: peek::ClosedTrades::default(),
        pending_bytes: Arc::default(),
    };
    let trade = row();
    for enabled in [false, true] {
        catchup::ENABLED.store(enabled, Ordering::Relaxed);
        recorder.apply(
            Cmd::Closed {
                exchange: trade.exchange.clone(),
                market: trade.market.clone(),
                venue: trade.venue,
                trade: trade.id,
                open_ms: trade.open_ms,
                close_ms: trade.close_ms,
                emulator: Some(false),
            },
            NOW,
        );
        assert_eq!(recorder.catchup.known.contains_key(&trade.id), enabled);
        assert!(
            recorder.peek(trade.id, 0, 0).is_ok(),
            "terminal peek tracking must survive"
        );
    }
}

/// Dropping a request in Listing or Inventory would strand emulator tape under keep forever.
#[test]
fn review_purge_requests_survive_every_busy_phase() {
    use super::super::super::retention::Phase;
    use crate::market::trade_replay::trade_cache::Inventory;
    use std::sync::atomic::Ordering;
    let _lock = SWITCH_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    for phase in 0..4 {
        catchup::set_keep(0, true);
        catchup::take_purge_wanted();
        let (inventory_tx, inventory_rx) = mpsc::channel::<rusqlite::Result<Inventory>>();
        let (trim_tx, trim_rx) = mpsc::channel();
        let (read_tx, read_rx) = mpsc::channel();
        let mut state = RecorderCatchup {
            active: true,
            ..Default::default()
        };
        state.retention.phase = match phase {
            0 => Phase::Listing {
                since: NOW,
                generation: catchup::relist_gen(),
            },
            1 => Phase::Inventory {
                rx: inventory_rx,
                generation: catchup::relist_gen(),
                complete: true,
            },
            2 => Phase::Trim {
                rx: trim_rx,
                progress: Arc::default(),
            },
            _ => Phase::Recount {
                reads: vec![(row(), read_rx)],
                complete: HashSet::new(),
                failure: None,
                freed: 0,
            },
        };
        catchup::set_keep(0, false);
        assert!(state.retention_tick(
            &cache,
            NOW,
            &HashMap::new(),
            &catchup::Shape {
                margin_ms: crate::market::trade_replay::margin_ms(),
                long_ms: crate::market::trade_replay::long_position_ms(),
                tail_ms: crate::market::trade_replay::margin_ms()
            }
        ));
        assert!(
            catchup::PURGE_WANTED.load(Ordering::Relaxed),
            "phase {phase} lost the request"
        );
        // A later busy tick must replace even a stale terminal status with Running.
        STATUS.lock().unwrap().purge = Some(crate::station_api::TapePurge::Done {
            at_ms: NOW - 1,
            freed_bytes: 0,
        });
        assert!(state.retention_tick(
            &cache,
            NOW + 1,
            &HashMap::new(),
            &catchup::Shape {
                margin_ms: crate::market::trade_replay::margin_ms(),
                long_ms: crate::market::trade_replay::long_position_ms(),
                tail_ms: crate::market::trade_replay::margin_ms()
            }
        ));
        assert!(matches!(
            catchup::status().purge,
            Some(crate::station_api::TapePurge::Running { .. })
        ));
        drop((inventory_tx, trim_tx, read_tx));
    }
}

/// A queued narrowing must start another pass after an older recount and delete emulator tape.
#[test]
fn review_queued_purge_runs_after_the_current_pass() {
    use super::super::super::retention::Phase;
    let _lock = SWITCH_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(0, true);
    catchup::take_purge_wanted();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let mut trade = row();
    trade.emulator = true;
    let focus = catchup::needed(
        &trade,
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms(),
        },
    );
    let (from, to) = focus.hull().unwrap();
    cache.insert(
        &trade.exchange,
        &trade.market,
        from,
        to,
        vec![],
        crate::market::trade_replay::tick_tiles::TileSource::Core,
    );
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    state.retention.phase = Phase::Recount {
        reads: vec![],
        complete: HashSet::new(),
        failure: None,
        freed: 0,
    };
    catchup::set_keep(0, false);
    assert!(
        state.retention_tick(
            &cache,
            NOW,
            &HashMap::new(),
            &catchup::Shape {
                margin_ms: crate::market::trade_replay::margin_ms(),
                long_ms: crate::market::trade_replay::long_position_ms(),
                tail_ms: crate::market::trade_replay::margin_ms()
            }
        ),
        "queued maintenance must keep catch-up paused"
    );
    assert!(matches!(
        catchup::status().purge,
        Some(crate::station_api::TapePurge::Running { .. })
    ));
    settle(&mut state, &cache, NOW + 1);
    assert!(
        cache
            .held_spans(&trade.exchange, &trade.market, from, to)
            .unwrap()
            .is_empty()
    );
}

/// A close received between listings must protect shared tape after its live task settles.
#[test]
fn real_trade_closed_after_listing_protects_shared_emulator_tape() {
    use crate::market::tape_recorder::{Cmd, Recorder, Stats, peek};
    use std::time::Instant;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(0, false);
    let real = row();
    let mut emu = real.clone();
    emu.id.rec_id += 1;
    emu.emulator = true;
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let focus = catchup::needed(
        &real,
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms(),
        },
    );
    let (from, to) = focus.hull().unwrap();
    cache.insert(
        &real.exchange,
        &real.market,
        from,
        to,
        vec![],
        crate::market::trade_replay::tick_tiles::TileSource::Core,
    );
    let now = Instant::now();
    let mut recorder = Recorder {
        cache,
        catchup: RecorderCatchup::default(),
        tasks: HashMap::new(),
        donors: HashMap::new(),
        shunned: HashMap::new(),
        stats: Stats::default(),
        last_summary: now,
        next_drain: now,
        replies: Vec::new(),
        ticks: Vec::new(),
        compares: Vec::new(),
        closed: peek::ClosedTrades::default(),
        pending_bytes: Arc::default(),
    };
    recorder.catchup.submit(vec![emu]);
    recorder.apply(
        Cmd::Closed {
            exchange: real.exchange.clone(),
            market: real.market.clone(),
            venue: real.venue,
            trade: real.id,
            open_ms: real.open_ms,
            close_ms: real.close_ms,
            emulator: Some(false),
        },
        NOW,
    );
    // The protection must outlive the live task's removal after its recording settles.
    recorder.tasks.clear();
    settle(&mut recorder.catchup, &recorder.cache, NOW);
    assert!(
        Coverage::from_spans(
            recorder
                .cache
                .held_spans(&real.exchange, &real.market, from, to)
                .unwrap()
        )
        .covers(&focus)
    );
}

/// A trim rollback caused by a full disk must report room pressure and avoid minute retries.
#[test]
fn disk_full_during_trim_reports_no_room() {
    use super::super::super::retention::Phase;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(0, true);
    catchup::take_purge_wanted();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(Err(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(13),
        None,
    )))
    .unwrap();
    let mut state = RecorderCatchup::default();
    state.retention.phase = Phase::Trim {
        rx,
        progress: Arc::default(),
    };
    assert!(!state.retention_tick(
        &cache,
        NOW,
        &HashMap::new(),
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms()
        }
    ));
    assert!(matches!(
        catchup::status().purge,
        Some(crate::station_api::TapePurge::Failed {
            reason: crate::station_api::PurgeFail::DeleteNoRoom,
            ..
        })
    ));
    assert_eq!(state.retention.retry_at, Some(NOW + HOUR_MS));
    catchup::set_keep(0, false);
    catchup::take_purge_wanted();
    state.retention.attributed_deferred = true;
    assert!(
        !state.retention_tick(
            &cache,
            NOW + 60_000,
            &HashMap::new(),
            &catchup::Shape {
                margin_ms: crate::market::trade_replay::margin_ms(),
                long_ms: crate::market::trade_replay::long_position_ms(),
                tail_ms: crate::market::trade_replay::margin_ms()
            }
        ),
        "a completed listing must not bypass the disk-full backoff"
    );
}

/// The first status read after a narrowing write must already show maintenance during listing.
#[test]
fn narrowing_publishes_running_while_listing() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(0, true);
    catchup::take_purge_wanted();
    catchup::require_listings(vec![90_001]);
    catchup::set_keep(30, true);
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let mut state = RecorderCatchup::default();
    assert!(state.retention_tick(
        &cache,
        NOW,
        &HashMap::new(),
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms()
        }
    ));
    assert!(matches!(
        catchup::status().purge,
        Some(crate::station_api::TapePurge::Running { done: 0, total: 0 })
    ));
}

/// Settlement must not synchronously read held spans while maintenance owns the cache queue.
#[test]
fn pass_settling_during_a_trim_does_not_block_the_tick() {
    use super::super::super::retention::Phase;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let (_tx, rx) = mpsc::channel();
    let mut state = RecorderCatchup {
        active: true,
        ..RecorderCatchup::default()
    };
    state.retention.phase = Phase::Trim {
        rx,
        progress: Arc::default(),
    };
    state.finish(
        &cache,
        PassResult {
            finished: vec![],
            archive_bytes: 321,
        },
        NOW,
    );
    assert!(state.active);
    assert_eq!(state.archive_bytes, 0);
    state.retention.phase = Phase::Idle;
    let (result, at) = state.deferred_finish.take().unwrap();
    state.finish(&cache, result, at);
    assert!(!state.active);
    assert_eq!(state.archive_bytes, 321);
}

const DAY: i64 = 86_400_000;
const NOW: i64 = 500 * DAY;

/// Settling a recovered recent exit must latch an expired entry without falsely counting full tape.
#[test]
fn unlisted_mixed_age_settlement_never_replans_expired_entry() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let mut trade = row();
    trade.venue = None;
    trade.exchange = crate::fixture::unlisted::EXCHANGE_KEY.into();
    trade.open_ms = NOW - 49 * HOUR_MS;
    trade.close_ms = NOW - HOUR_MS;
    trade.core = Some(
        crate::fixture::unlisted::source(trade.id.core)
            .replay_address(trade.id.core)
            .unwrap(),
    );
    let shape = catchup::Shape {
        margin_ms: 1_000,
        long_ms: HOUR_MS,
        tail_ms: 2_000,
    };
    let focus = catchup::needed(&trade, &shape);
    let recent = *focus.spans().last().unwrap();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    let batch = state.prepare(|_| Coverage::none(), NOW, &shape, true);
    assert_eq!(batch.jobs.len(), 1);
    let result = executor::run_pass_with_core(
        batch.jobs,
        || NOW,
        &gate::ReplayGate::new(),
        (
            |_, _, _, _, _| panic!("unlisted REST"),
            |_, _, _, _| panic!("unlisted archive"),
            Some(Arc::new(move |ask| {
                assert_eq!((ask.from_ms, ask.to_ms), recent);
                (
                    Some(crate::market::source::CoreReplayTicks {
                        ticks: vec![],
                        covered: recent,
                        archive_answered: true,
                    }),
                    false,
                )
            })),
        ),
        0,
        executor::run_job_at,
        |job, answer| {
            for &(from, to) in answer.file.spans() {
                cache.insert(
                    &job.trade.exchange,
                    &job.trade.market,
                    from,
                    to,
                    vec![],
                    crate::market::trade_replay::TileSource::Core,
                );
            }
        },
    );
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    state.finish(&cache, result, NOW);
    assert_eq!(state.latched.get(&trade.id), Some(&Latch::Permanent));
    assert!(!state.tape_complete.contains(&trade.id));
    assert!(
        state
            .prepare(|_| Coverage::one(recent), NOW, &shape, true)
            .jobs
            .is_empty()
    );
}

/// Preserve global retention switches and barriers around asynchronous recorder fixtures.
struct RetentionSettings {
    enabled: bool,
    days: u16,
    emulator: bool,
    wanted: bool,
    required: Option<Vec<u64>>,
}

impl RetentionSettings {
    /// An empty required set models a station with no protecting remote cores.
    fn new() -> Self {
        use std::sync::atomic::Ordering;
        let saved = Self {
            enabled: catchup::enabled(),
            days: catchup::max_age_days(),
            emulator: catchup::record_emulator(),
            wanted: catchup::PURGE_WANTED.load(Ordering::Relaxed),
            required: catchup::REQUIRED_CORES.lock().unwrap().clone(),
        };
        catchup::enable();
        catchup::require_listings(vec![]);
        saved
    }
}

impl Drop for RetentionSettings {
    /// Avoid leaking finite horizons or a purge request into unrelated planner fixtures.
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        catchup::set_keep(self.days, self.emulator);
        catchup::ENABLED.store(self.enabled, Ordering::Relaxed);
        catchup::PURGE_WANTED.store(self.wanted, Ordering::Relaxed);
        *catchup::REQUIRED_CORES.lock().unwrap() = self.required.clone();
    }
}

/// Poll fixture maintenance to its outcome with a bounded deadline and no production fetches.
fn settle(state: &mut RecorderCatchup, cache: &TradeCache, now: i64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.retention_tick(
        cache,
        now,
        &HashMap::new(),
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms(),
        },
    ) {
        assert!(
            std::time::Instant::now() < deadline,
            "maintenance did not settle"
        );
        std::thread::yield_now();
    }
}

/// A finite horizon must remove tape again when time advances without a settings change.
#[test]
fn retention_drops_tape_that_ages_out() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(1, true);
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    cache.insert(
        "synthetic",
        "SYNUSDT",
        NOW - DAY + 1000,
        NOW - DAY + 2000,
        vec![],
        crate::market::trade_replay::tick_tiles::TileSource::Core,
    );
    let mut state = RecorderCatchup::default();
    settle(&mut state, &cache, NOW);
    assert_eq!(
        cache
            .held_spans("synthetic", "SYNUSDT", i64::MIN, i64::MAX)
            .unwrap()
            .len(),
        1
    );
    settle(&mut state, &cache, NOW + HOUR_MS);
    assert!(
        cache
            .held_spans("synthetic", "SYNUSDT", i64::MIN, i64::MAX)
            .unwrap()
            .is_empty()
    );
}

/// A venue-less core with a successfully enumerated empty report must not hold unrelated aged tape.
#[test]
fn a_venue_less_core_does_not_hold_aged_tape() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(1, true);
    let core = 70_001;
    catchup::require_listings(vec![core]);
    catchup::listing_started(core, catchup::relist_gen());
    catchup::listing_applied(core, catchup::relist_gen());
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    cache.insert(
        "synthetic",
        "SYNUSDT",
        NOW - 3 * DAY,
        NOW - 2 * DAY,
        vec![],
        crate::market::trade_replay::tick_tiles::TileSource::Core,
    );
    let mut state = RecorderCatchup::default();
    settle(&mut state, &cache, NOW);
    assert!(
        cache
            .held_spans("synthetic", "SYNUSDT", i64::MIN, i64::MAX)
            .unwrap()
            .is_empty()
    );
}

/// Without a deferred request, forever retention would never revisit an emulator purge vetoed by an offline report.
#[test]
fn incomplete_listing_defers_emulator_purge() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(0, false);
    catchup::require_listings(vec![40_001]);
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let mut trade = row();
    trade.emulator = true;
    let focus = catchup::needed(
        &trade,
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms(),
        },
    );
    let (from, to) = focus.hull().unwrap();
    cache.insert(
        &trade.exchange,
        &trade.market,
        from,
        to,
        vec![],
        crate::market::trade_replay::tick_tiles::TileSource::Core,
    );
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    assert!(state.retention_tick(
        &cache,
        NOW,
        &HashMap::new(),
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms()
        }
    ));
    settle(&mut state, &cache, NOW + 600_000);
    assert_eq!(
        cache
            .held_spans(&trade.exchange, &trade.market, from, to)
            .unwrap(),
        vec![(from, to)]
    );
    assert!(state.retention.attributed_deferred);
    catchup::listing_applied(40_001, catchup::relist_gen());
    settle(&mut state, &cache, NOW + 600_001);
    assert!(
        cache
            .held_spans(&trade.exchange, &trade.market, from, to)
            .unwrap()
            .is_empty()
    );
    assert!(!state.retention.attributed_deferred);
}

/// An older pass can file its final answer only before retention enters its cache queue.
#[test]
fn narrowing_during_a_fetch_never_refiles_purged_tape() {
    use super::super::super::retention::Phase;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(1, true);
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let mut state = RecorderCatchup {
        active: true,
        ..Default::default()
    };
    assert!(state.retention_tick(
        &cache,
        NOW,
        &HashMap::new(),
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms()
        }
    ));
    assert!(matches!(state.retention.phase, Phase::Listing { .. }));
    cache.insert(
        "synthetic",
        "SYNUSDT",
        NOW - 3 * DAY,
        NOW - 2 * DAY,
        vec![],
        crate::market::trade_replay::tick_tiles::TileSource::Core,
    );
    state.finish(
        &cache,
        PassResult {
            finished: vec![],
            archive_bytes: 0,
        },
        NOW,
    );
    settle(&mut state, &cache, NOW);
    assert!(
        cache
            .held_spans("synthetic", "SYNUSDT", i64::MIN, i64::MAX)
            .unwrap()
            .is_empty()
    );
}

/// A cache worker held in maintenance must never block another recorder tick.
#[test]
fn ticks_continue_while_a_trim_runs() {
    use super::super::super::retention::Phase;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut state = RecorderCatchup::default();
    state.retention.phase = Phase::Trim {
        rx,
        progress: std::sync::Arc::default(),
    };
    let before = std::time::Instant::now();
    for _ in 0..10 {
        assert!(state.retention_tick(
            &cache,
            NOW,
            &HashMap::new(),
            &catchup::Shape {
                margin_ms: crate::market::trade_replay::margin_ms(),
                long_ms: crate::market::trade_replay::long_position_ms(),
                tail_ms: crate::market::trade_replay::margin_ms()
            }
        ));
    }
    assert!(before.elapsed() < std::time::Duration::from_secs(1));
    assert!(!state.active);
}

/// Done must wait for the explicit coverage answers and updated pending count.
#[test]
fn done_is_published_after_the_queue_recount() {
    use super::super::super::retention::Phase;
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let _settings = RetentionSettings::new();
    catchup::set_keep(0, true);
    catchup::take_purge_wanted();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let trade = row();
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    let mut unsupported = row();
    unsupported.id.rec_id = 999;
    state.known.insert(unsupported.id, unsupported.clone());
    state.latched.insert(unsupported.id, Latch::Permanent);
    let (unsupported_tx, unsupported_rx) = std::sync::mpsc::channel();
    unsupported_tx.send(Ok(Vec::new())).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    state.retention.phase = Phase::Recount {
        reads: vec![(trade.clone(), rx), (unsupported, unsupported_rx)],
        complete: HashSet::new(),
        failure: None,
        freed: 123,
    };
    STATUS.lock().unwrap().purge =
        Some(crate::station_api::TapePurge::Running { done: 1, total: 1 });
    assert!(state.retention_tick(
        &cache,
        NOW,
        &HashMap::new(),
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms()
        }
    ));
    assert_eq!(catchup::status().pending, 1);
    assert!(matches!(
        catchup::status().purge,
        Some(crate::station_api::TapePurge::Running { .. })
    ));
    tx.send(Ok(catchup::needed(
        &trade,
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms(),
        },
    )
    .spans()
    .to_vec()))
        .unwrap();
    assert!(!state.retention_tick(
        &cache,
        NOW,
        &HashMap::new(),
        &catchup::Shape {
            margin_ms: crate::market::trade_replay::margin_ms(),
            long_ms: crate::market::trade_replay::long_position_ms(),
            tail_ms: crate::market::trade_replay::margin_ms()
        }
    ));
    assert_eq!(catchup::status().pending, 0);
    assert!(matches!(
        catchup::status().purge,
        Some(crate::station_api::TapePurge::Done {
            freed_bytes: 123,
            ..
        })
    ));
}

/// Counts must use complete known IDs, and disk usage must include the independently sized WAL.
#[test]
fn status_reports_db_size_and_age_histogram() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let path = std::env::temp_dir().join(format!(
        "synthetic-retention-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let wal = path.with_extension("sqlite-wal");
    std::fs::write(&path, vec![0; 4096]).unwrap();
    std::fs::write(&wal, vec![0; 127]).unwrap();
    let mut state = RecorderCatchup::default();
    let real = row();
    let mut emulator = row();
    emulator.id.rec_id = 92;
    emulator.emulator = true;
    emulator.close_ms = NOW - 2 * DAY;
    state.known.insert(real.id, real.clone());
    state.known.insert(emulator.id, emulator.clone());
    state.tape_complete.extend([
        real.id,
        emulator.id,
        TradeId {
            core: 99,
            rec_id: 99,
        },
    ]);
    state.publish_storage_at(NOW, &path);
    let status = catchup::status();
    std::fs::remove_file(&wal).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(status.db_bytes, Some(4223));
    assert_eq!(status.tape_trades, Some(2));
    assert_eq!(
        status.tape_ages,
        vec![
            crate::station_api::TapeAge {
                days: 1,
                plain: 1,
                emulator: 0
            },
            crate::station_api::TapeAge {
                days: 2,
                plain: 0,
                emulator: 1
            }
        ]
    );
}

/// Throughput must use completed trades over elapsed recovery time rather than queued jobs.
#[test]
fn status_reports_catchup_rate() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let mut state = RecorderCatchup {
        rate_started: Some(NOW - HOUR_MS / 2),
        ..Default::default()
    };
    state.completions.push_back((NOW - 1000, 10));
    state.finish(
        &cache,
        PassResult {
            finished: vec![],
            archive_bytes: 0,
        },
        NOW,
    );
    assert_eq!(catchup::status().catchup_rate_per_hour, Some(20));
    let encoded = serde_json::to_value(catchup::status()).unwrap();
    assert_eq!(encoded["catchup_rate_per_hour"], 20);
}

/// Restore global choices and progress even when an assertion unwinds.
struct Restore {
    status: crate::station_api::TapeCatchup,
    margin_s: u32,
    long_min: u32,
    min_tail_s: u32,
    tail_final: u32,
}

impl Restore {
    /// Capture the prior process-wide configuration under the test serialization lock.
    fn new() -> Self {
        Self {
            status: catchup::status(),
            margin_s: (crate::market::trade_replay::margin_ms() / 1000) as u32,
            long_min: (crate::market::trade_replay::long_position_ms() / 60_000) as u32,
            min_tail_s: crate::market::trade_replay::min_tail_s(),
            tail_final: catchup::tail_final(),
        }
    }
}

impl Drop for Restore {
    /// Keep later recorder tests independent of switch and status changes in this fixture.
    fn drop(&mut self) {
        catchup::set_switches(self.status.auto.unwrap(), self.status.archives.unwrap());
        crate::market::trade_replay::set_margin_s(self.margin_s);
        crate::market::trade_replay::set_long_position_min(self.long_min);
        crate::market::trade_replay::set_min_tail_s(self.min_tail_s);
        catchup::TAIL_FINAL.store(self.tail_final, std::sync::atomic::Ordering::Relaxed);
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
        venue: Some(Venue {
            brand: Brand::Bybit,
            kind: MarketKind::Futures,
        }),
        open_ms: NOW - DAY - 60_000,
        core: None,
        close_ms: NOW - DAY,
        emulator: false,
        tunable: true,
    }
}

/// Reapplying a floor must preserve settlement; widening must expose just the missing tail.
#[test]
fn min_tail_changes_replan_settled_rows_once() {
    use crate::market::trade_replay::{min_tail_s, record_tail_ms, set_margin_s, set_min_tail_s};
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    set_margin_s(30);
    set_min_tail_s(60);
    let trade = row();
    let held = Coverage::one((trade.open_ms - 30_000, trade.close_ms + 60_000));
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    let shape = || catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: record_tail_ms(),
    };
    assert_eq!(
        state
            .prepare(|_| held.clone(), NOW, &shape(), true)
            .complete,
        1
    );
    let generation = catchup::switches().0;
    set_min_tail_s(60);
    assert_eq!(catchup::switches().0, generation);
    state.submit(vec![trade.clone()]);
    assert!(
        state
            .prepare(
                |_| panic!("unchanged floor rechecked tape"),
                NOW,
                &shape(),
                true
            )
            .jobs
            .is_empty()
    );
    set_min_tail_s(120);
    assert_eq!(min_tail_s(), 120);
    assert_eq!(record_tail_ms(), 120_000);
    state.submit(vec![trade.clone()]);
    let batch = state.prepare(|_| held.clone(), NOW, &shape(), true);
    assert_eq!(
        batch.jobs[0].missing.spans(),
        &[(trade.close_ms + 60_001, trade.close_ms + 120_000)]
    );
}

/// Changing a floor hidden by the recording margin must preserve settled catch-up windows.
#[test]
fn floor_below_recording_margin_preserves_generation() {
    use crate::market::trade_replay::{set_margin_s, set_min_tail_s};
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    set_margin_s(180);
    set_min_tail_s(60);
    let generation = catchup::switches().0;
    set_min_tail_s(90);
    assert_eq!(catchup::switches().0, generation);
}

/// Only two answered, successfully filed, tail-only passes may finalise an incomplete tail.
#[test]
fn empty_archive_tail_finalises_after_two_answers() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    let trade = row();
    let held = Coverage::one((trade.open_ms - 30_000, trade.close_ms + 30_000));
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    cache.insert(
        &trade.exchange,
        &trade.market,
        held.hull().unwrap().0,
        held.hull().unwrap().1,
        vec![],
        crate::market::trade_replay::TileSource::Core,
    );
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    for pass in 0..2 {
        let now = NOW + pass * HOUR_MS;
        let batch = state.prepare(|_| held.clone(), now, &shape, true);
        let result = executor::run_pass_with_core(
            batch.jobs,
            || now,
            &gate::ReplayGate::new(),
            (
                |_, _, _, _, _| panic!("archive-only job"),
                |_, _, _, _| {
                    (
                        Ok(crate::market::trade_replay::archive::ArchiveFill {
                            covered: Coverage::none(),
                            ticks: vec![],
                            deferred: None,
                        }),
                        0,
                    )
                },
                None,
            ),
            0,
            executor::run_job_at,
            |_, _| {},
        );
        assert!(result.finished[0].attempted);
        assert_eq!(result.finished[0].failure, None);
        state.finish(&cache, result, now);
        assert_eq!(
            state.latched.get(&trade.id) == Some(&Latch::Permanent),
            pass == 1
        );
    }
    assert_eq!(catchup::tail_final(), 1);
}

/// Shared day grouping must classify empty ordinary gaps independently of the first tail job.
#[test]
fn shared_empty_archive_day_preserves_each_jobs_attempt_state() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    for tail_first in [true, false] {
        let tail = row();
        let mut gap = tail.clone();
        gap.id.rec_id += 1;
        let mut state = RecorderCatchup::default();
        state.submit(vec![tail.clone(), gap.clone()]);
        let mut batch = state.prepare(
            |trade| {
                Coverage::one((
                    trade.open_ms - 30_000,
                    trade.close_ms + if trade.id == tail.id { 30_000 } else { 29_999 },
                ))
            },
            NOW,
            &shape,
            true,
        );
        batch
            .jobs
            .sort_by_key(|job| (job.trade.id == tail.id) != tail_first);
        assert_eq!(batch.jobs.len(), 2);
        let result = executor::run_pass_with_core(
            batch.jobs,
            || NOW,
            &gate::ReplayGate::new(),
            (
                |_, _, _, _, _| panic!("archive-only fixture"),
                |_, _, _, _| {
                    (
                        Ok(crate::market::trade_replay::archive::ArchiveFill {
                            covered: Coverage::none(),
                            ticks: vec![],
                            deferred: None,
                        }),
                        0,
                    )
                },
                None,
            ),
            0,
            executor::run_job_at,
            |_, _| {},
        );
        assert_eq!(result.finished.len(), 2);
        for finished in result.finished {
            assert_eq!(finished.attempted, finished.job.trade.id == tail.id);
            assert!(finished.failure.is_none());
        }
    }
}

/// Ring-only answers after a timed-out archive wait must never consume clean tail attempts.
#[test]
fn timed_out_partial_core_answers_do_not_finalise_a_tail() {
    use crate::market::{MarketDataSource, MarketStore};
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    let mut trade = row();
    trade.venue = None;
    trade.core = Some(crate::market::source::ReplayAddress {
        history: MarketDataSource::new(MarketStore::shared(0.0)),
        venue: None,
        exchange_key: trade.exchange.clone(),
        cache: None,
    });
    let held = Coverage::one((trade.open_ms - 30_000, trade.close_ms + 30_000));
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    cache.insert(
        &trade.exchange,
        &trade.market,
        held.hull().unwrap().0,
        held.hull().unwrap().1,
        vec![],
        crate::market::trade_replay::TileSource::Core,
    );
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    for pass in 0..2 {
        let now = NOW + pass * HOUR_MS;
        let batch = state.prepare(|_| held.clone(), now, &shape, true);
        assert_eq!(batch.jobs.len(), 1);
        let result = executor::run_pass_with_core(
            batch.jobs,
            || now,
            &gate::ReplayGate::new(),
            (
                |_, _, _, _, _| panic!("core-only fixture"),
                |_, _, _, _| panic!("core-only fixture"),
                Some(Arc::new(|ask| {
                    (
                        Some(crate::market::source::CoreReplayTicks {
                            ticks: vec![],
                            covered: (ask.from_ms, ask.from_ms + 100),
                            archive_answered: false,
                        }),
                        false,
                    )
                })),
            ),
            0,
            executor::run_job_at,
            |job, answer| {
                for &(from, to) in answer.file.spans() {
                    cache.insert(
                        &job.trade.exchange,
                        &job.trade.market,
                        from,
                        to,
                        vec![],
                        crate::market::trade_replay::TileSource::Core,
                    );
                }
            },
        );
        assert_eq!(result.finished[0].failure, Some(Failure::Retry));
        assert_eq!(
            result.finished[0].fault.as_ref().unwrap().detail,
            "core archive wait did not answer"
        );
        assert!(cache.sync(std::time::Duration::from_secs(2)));
        state.finish(&cache, result, now);
        assert_ne!(state.latched.get(&trade.id), Some(&Latch::Permanent));
        assert!(!state.core_gap_tries.contains_key(&trade.id));
    }
}

/// Faults and unfiled answers must not consume the two clean attempts allowed for a tail.
#[test]
fn incomplete_tail_finalises_only_after_two_clean_passes() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    for mode in [
        "clean",
        "no_room",
        "archive_pending",
        "fault",
        "unfiled",
        "non_tail",
    ] {
        let trade = row();
        let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
        let held_end = trade.close_ms + if mode == "non_tail" { 29_999 } else { 30_000 };
        cache.insert(
            &trade.exchange,
            &trade.market,
            trade.open_ms - 30_000,
            held_end,
            vec![],
            crate::market::trade_replay::TileSource::Core,
        );
        let mut state = RecorderCatchup::default();
        state.submit(vec![trade.clone()]);
        for pass in 0..2 {
            let now = NOW + pass * HOUR_MS;
            let batch = state.prepare(
                |_| Coverage::one((trade.open_ms - 30_000, held_end)),
                now,
                &shape,
                true,
            );
            let job = batch.jobs.into_iter().next().unwrap();
            if mode == "no_room" {
                state.filing_refused(trade.exchange.clone(), trade.market.clone());
            }
            let filed = if mode == "unfiled" {
                job.missing.clone()
            } else {
                Coverage::none()
            };
            state.finish(
                &cache,
                PassResult {
                    finished: vec![executor::FinishedJob {
                        job,
                        failure: (mode == "archive_pending").then_some(Failure::Retry),
                        fault: (mode == "fault").then(|| Fault {
                            reason: CatchupFailReason::Network,
                            detail: "fixture fault".into(),
                        }),
                        attempted: mode != "archive_pending",
                        filed,
                    }],
                    archive_bytes: 0,
                },
                now,
            );
            assert_eq!(
                state.latched.get(&trade.id) == Some(&Latch::Permanent),
                mode == "clean" && pass == 1,
                "{mode} pass {pass}"
            );
        }
        assert_eq!(catchup::tail_final(), u32::from(mode == "clean"), "{mode}");
        if mode == "clean" {
            state.submit(vec![trade.clone()]);
            assert!(
                state
                    .prepare(
                        |_| panic!("final tail re-read"),
                        NOW + 2 * HOUR_MS,
                        &shape,
                        true
                    )
                    .jobs
                    .is_empty()
            );
        } else {
            assert!(!state.pending.is_empty(), "{mode}");
            assert!(!state.core_gap_tries.contains_key(&trade.id), "{mode}");
        }
    }
}

/// Answered empty rings must not stop another job on the core, and must cap gaps after two passes.
#[test]
fn unlisted_answered_misses_do_not_stop_core_and_finalise_after_two_passes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let mut trade = row();
    trade.venue = None;
    trade.exchange = crate::fixture::unlisted::EXCHANGE_KEY.into();
    trade.core = Some(
        crate::fixture::unlisted::source(trade.id.core)
            .replay_address(trade.id.core)
            .unwrap(),
    );
    let mut second = trade.clone();
    second.id.rec_id += 1;
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone(), second.clone()]);
    for pass in 0..2 {
        let now = NOW + pass * HOUR_MS;
        let batch = state.prepare(|_| Coverage::none(), now, &shape, true);
        assert_eq!(batch.jobs.len(), 2);
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let result = executor::run_pass_with_core(
            batch.jobs,
            || now,
            &gate::ReplayGate::new(),
            (
                |_, _, _, _, _| panic!("core-only fixture"),
                |_, _, _, _| panic!("core-only fixture"),
                Some(Arc::new(move |_| {
                    counted.fetch_add(1, Ordering::Relaxed);
                    (None, true)
                })),
            ),
            0,
            executor::run_job_at,
            |_, _| {},
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert!(result.finished.iter().all(|job| job.attempted
            && job.failure.is_none()
            && job.fault.is_none()
            && job.filed.is_empty()));
        state.finish(&cache, result, now);
        for id in [trade.id, second.id] {
            assert_eq!(state.latched.get(&id) == Some(&Latch::Permanent), pass == 1);
        }
    }
    state.submit(vec![trade, second]);
    assert!(
        state
            .prepare(
                |_| panic!("final gap re-read"),
                NOW + 2 * HOUR_MS,
                &shape,
                true
            )
            .jobs
            .is_empty()
    );
}

/// Unlisted in-trade holes must stop spending core requests after two clean partial answers.
#[test]
fn unlisted_non_tail_gap_finalises_after_two_clean_answers() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let mut trade = row();
    trade.venue = None;
    trade.exchange = crate::fixture::unlisted::EXCHANGE_KEY.into();
    trade.core = Some(
        crate::fixture::unlisted::source(trade.id.core)
            .replay_address(trade.id.core)
            .unwrap(),
    );
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    let mut held = Coverage::one((trade.open_ms - 30_000, trade.open_ms + 1_000));
    cache.insert(
        &trade.exchange,
        &trade.market,
        trade.open_ms - 30_000,
        trade.open_ms + 1_000,
        vec![],
        crate::market::trade_replay::TileSource::Core,
    );
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    for pass in 0..2 {
        let now = NOW + pass * HOUR_MS;
        let batch = state.prepare(|_| held.clone(), now, &shape, true);
        assert_eq!(batch.jobs.len(), 1);
        let job = batch.jobs.into_iter().next().unwrap();
        assert!(!tail_only(&job.missing, job.tail_start_ms));
        let from = trade.open_ms + 1_001 + pass * 1_000;
        let to = from + 999;
        let filed = Coverage::one((from, to));
        assert!(job.missing.covers(&filed));
        cache.insert(
            &trade.exchange,
            &trade.market,
            from,
            to,
            vec![],
            crate::market::trade_replay::TileSource::Core,
        );
        assert!(cache.sync(std::time::Duration::from_secs(2)));
        held.add((from, to));
        state.finish(
            &cache,
            PassResult {
                finished: vec![executor::FinishedJob {
                    job,
                    failure: None,
                    fault: None,
                    attempted: true,
                    filed,
                }],
                archive_bytes: 0,
            },
            now,
        );
        assert_eq!(
            state.latched.get(&trade.id) == Some(&Latch::Permanent),
            pass == 1
        );
    }
    assert!(!state.tail_final.contains(&trade.id));
    state.submit(vec![trade]);
    assert!(
        state
            .prepare(
                |_| panic!("final gap re-read"),
                NOW + 2 * HOUR_MS,
                &shape,
                true
            )
            .jobs
            .is_empty()
    );
}

/// A failed coverage read must never be interpreted as a clean empty tail response.
#[test]
fn unavailable_held_read_does_not_finalise_tail() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    // A directory cannot be opened as SQLite; the cache worker exits without answering reads.
    let cache = TradeCache::open_evictable(std::env::temp_dir()).unwrap();
    let trade = row();
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    for pass in 0..2 {
        let now = NOW + pass * HOUR_MS;
        let job = state
            .prepare(
                |_| Coverage::one((trade.open_ms - 30_000, trade.close_ms + 30_000)),
                now,
                &shape,
                true,
            )
            .jobs
            .remove(0);
        state.finish(
            &cache,
            PassResult {
                finished: vec![executor::FinishedJob {
                    job,
                    failure: None,
                    fault: None,
                    attempted: true,
                    filed: Coverage::none(),
                }],
                archive_bytes: 0,
            },
            now,
        );
    }
    assert!(!state.core_gap_tries.contains_key(&trade.id));
    assert!(matches!(
        state.latched.get(&trade.id),
        Some(Latch::RetryAfter(_))
    ));
}

/// With no reachable source, a missing tail is final without spending either retry pass.
#[test]
fn venue_less_tail_without_source_is_final_immediately() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let mut trade = row();
    trade.venue = None;
    trade.open_ms -= 3 * DAY;
    trade.close_ms -= 3 * DAY;
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    let mut state = RecorderCatchup::default();
    state.submit(vec![trade.clone()]);
    let batch = state.prepare(
        |_| Coverage::one((trade.open_ms - 30_000, trade.close_ms + 30_000)),
        NOW,
        &shape,
        true,
    );
    assert!(batch.jobs.is_empty());
    assert_eq!(state.latched.get(&trade.id), Some(&Latch::Permanent));
    assert_eq!(catchup::tail_final(), 1);
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
            .prepare(
                |_| held.clone(),
                NOW,
                &catchup::Shape {
                    margin_ms: margin_ms(),
                    long_ms: DAY,
                    tail_ms: margin_ms()
                },
                true
            )
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
                &catchup::Shape {
                    margin_ms: margin_ms(),
                    long_ms: DAY,
                    tail_ms: margin_ms()
                },
                true
            )
            .jobs
            .is_empty()
    );
    set_margin_s(60);
    state.submit(vec![trade.clone()]);
    let plan = state.prepare(
        |_| held.clone(),
        NOW,
        &catchup::Shape {
            margin_ms: margin_ms(),
            long_ms: DAY,
            tail_ms: margin_ms(),
        },
        true,
    );
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
            .prepare(
                |_| wider.clone(),
                NOW,
                &catchup::Shape {
                    margin_ms: margin_ms(),
                    long_ms: DAY,
                    tail_ms: margin_ms()
                },
                true
            )
            .complete,
        1
    );
    set_margin_s(30);
    state.submit(vec![trade]);
    let plan = state.prepare(
        |_| wider.clone(),
        NOW,
        &catchup::Shape {
            margin_ms: margin_ms(),
            long_ms: DAY,
            tail_ms: margin_ms(),
        },
        true,
    );
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
        .prepare(
            |_| Coverage::none(),
            NOW,
            &catchup::Shape {
                margin_ms: 0,
                long_ms: DAY,
                tail_ms: 0,
            },
            true,
        )
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
                filed: Coverage::none(),
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
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: DAY,
                    tail_ms: 0
                },
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
            .prepare(
                |_| held.clone(),
                NOW,
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: long_position_ms(),
                    tail_ms: 0
                },
                true
            )
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
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: long_position_ms(),
                    tail_ms: 0
                },
                true
            )
            .jobs
            .is_empty()
    );
    set_long_position_min(120);
    state.submit(vec![trade.clone()]);
    let plan = state.prepare(
        |_| held.clone(),
        NOW,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: long_position_ms(),
            tail_ms: 0,
        },
        true,
    );
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
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: long_position_ms(),
                    tail_ms: 0
                },
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
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: long_position_ms(),
                    tail_ms: 0
                },
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
    let refused = state.prepare(
        |_| Coverage::none(),
        NOW,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
        false,
    );
    assert_eq!(refused.no_route, 1);
    assert!(refused.jobs.is_empty());
    state.submit(vec![trade.clone()]);
    assert!(
        state
            .prepare(
                |_| panic!("latched row asked disk"),
                NOW,
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: DAY,
                    tail_ms: 0
                },
                false
            )
            .jobs
            .is_empty()
    );
    catchup::set_switches(true, true);
    state.submit(vec![trade.clone()]);
    let planned = state.prepare(
        |_| Coverage::none(),
        NOW,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
        true,
    );
    assert_eq!(planned.jobs.len(), 1);
    assert!(matches!(
        planned.jobs[0].source,
        catchup::Source::Archive(_)
    ));
    assert_eq!(
        state
            .prepare(
                |trade| catchup::needed(
                    trade,
                    &catchup::Shape {
                        margin_ms: 0,
                        long_ms: DAY,
                        tail_ms: 0
                    }
                ),
                NOW,
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: DAY,
                    tail_ms: 0
                },
                true
            )
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
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: DAY,
                    tail_ms: 0
                },
                true
            )
            .jobs
            .is_empty()
    );
    catchup::set_switches(false, true);
    state.submit(vec![trade]);
    assert_eq!(
        state
            .prepare(
                |_| Coverage::none(),
                NOW,
                &catchup::Shape {
                    margin_ms: 0,
                    long_ms: DAY,
                    tail_ms: 0
                },
                true
            )
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
    let plan = state.prepare(
        |_| Coverage::none(),
        NOW,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
        false,
    );
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
    state.tick(
        &cache,
        NOW,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    );
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(!catchup::status().running);
    assert_eq!(catchup::status().pending, 1);
    catchup::set_switches(true, true);
    state.tick(
        &cache,
        NOW,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    );
    let pass = receiver
        .try_recv()
        .expect("queued trade dispatched after enabling auto");
    assert_eq!(pass.jobs.len(), 1);
    assert!(catchup::status().running);
    assert_eq!(catchup::status().last_pass_ms, Some(NOW));
    state.tick(
        &cache,
        NOW + 1,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    );
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    // Reload while running: the old pass may settle, but its permanent refusal must not survive.
    catchup::set_switches(false, false);
    state.tick(
        &cache,
        NOW + 2,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    );
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
                    filed: Coverage::none(),
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
        tail_start_ms: i64::MAX,
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
        tail_start_ms: i64::MAX,
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
                filed: Coverage::none(),
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
        state.tick(
            &cache,
            now,
            &catchup::Shape {
                margin_ms: 0,
                long_ms: DAY,
                tail_ms: 0,
            },
        );
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
                        filed: Coverage::none(),
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
    state.tick(
        &cache,
        NOW + 10,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    );
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
    rest_row.venue = Some(Venue {
        brand: Brand::Gate,
        kind: MarketKind::Spot,
    });
    state.submit(vec![archive_row.clone(), rest_row.clone()]);

    state.tick(
        &cache,
        NOW,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    );
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
                    filed: Coverage::none(),
                })
                .collect(),
            archive_bytes: executor::ARCHIVE_BYTES_PER_PASS,
        },
        NOW,
    );

    state.tick(
        &cache,
        NOW + 1,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    );
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
    state.tick(
        &cache,
        next_hour,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    );
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
    let jobs = catchup::plan(
        &rows,
        |_| Coverage::none(),
        &HashMap::new(),
        NOW,
        &catchup::Shape {
            margin_ms: 0,
            long_ms: DAY,
            tail_ms: 0,
        },
    )
    .jobs;
    assert_eq!(admissible(jobs.clone(), 0, NOW).len(), PASS_JOBS);
    assert!(admissible(jobs, executor::ARCHIVE_BYTES_PER_PASS, NOW).is_empty());
}
