use super::*;

/// Build an isolated recorder whose reads and writes share one in-memory cache queue.
fn chart_recorder() -> Recorder {
    let now = Instant::now();
    Recorder {
        cache: TradeCache::open_evictable(":memory:".into()).unwrap(),
        catchup: catchup::RecorderCatchup::default(),
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
    }
}

/// The key hook must read both filed and unfiled prints exactly like the closed-trade hook.
#[test]
fn market_key_and_trade_reads_share_filed_and_buffered_tape() {
    let mut recorder = chart_recorder();
    let key = ("CORE_A".to_string(), "XALPHA".to_string());
    let trade = TradeId {
        core: 7,
        rec_id: 42,
    };
    let tick = |stamp| Tick {
        time_ms: stamp as f64,
        price: 10.0,
        qty: 1.0,
        side: crate::feed::Side::Buy,
    };
    recorder.cache.insert(
        &key.0,
        &key.1,
        1000,
        2000,
        vec![tick(1000), tick(1500)],
        TileSource::Core,
    );
    let mut task = KeyTask::new();
    task.seed_asked(2000, 1);
    task.seeded(vec![tick(2500)], 2000, 1);
    recorder.tasks.insert(key.clone(), task);
    recorder.closed.closed(trade, &key, 1500, 2500);
    let by_key = recorder.read_key(&key, 1500, 2500).unwrap();
    assert_eq!(
        by_key
            .ticks
            .iter()
            .map(|tick| tick.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![1500, 2500]
    );
    let by_trade = recorder.peek(trade, 0, 0).unwrap();
    let values = |ticks: &[Tick]| {
        ticks
            .iter()
            .map(|t| (t.time_ms, t.price, t.qty, t.side))
            .collect::<Vec<_>>()
    };
    assert_eq!(values(&by_key.ticks), values(&by_trade.ticks));
    assert_eq!(by_key.covered, vec![(1500, 2000)]);
    assert_eq!(by_trade.covered, by_key.covered);
    let (reply, answer) = mpsc::channel();
    recorder.apply(
        Cmd::PeekKey {
            key,
            from_ms: 1500,
            to_ms: 2500,
            reply,
        },
        3000,
    );
    assert_eq!(
        values(&answer.recv().unwrap().unwrap().ticks),
        values(&by_key.ticks)
    );
}

/// A stopped or empty key must not claim that its requested tape is complete.
#[test]
fn recorder_span_distinguishes_empty_partial_and_complete() {
    let mut recorder = chart_recorder();
    let key = ("CORE_A".to_owned(), "XALPHA".to_owned());
    let empty = recorder.read_key(&key, 1000, 4000).unwrap();
    assert!(empty.ticks.is_empty());
    assert!(empty.settled);
    assert!(empty.covered.is_empty());
    recorder
        .cache
        .insert(&key.0, &key.1, 1500, 2500, Vec::new(), TileSource::Core);
    let partial = recorder.read_key(&key, 1000, 4000).unwrap();
    assert_eq!(partial.covered, vec![(1500, 2500)]);
    assert!(partial.settled);
    recorder.watch_key(key.clone(), 5000, 3000);
    assert!(!recorder.read_key(&key, 1000, 4000).unwrap().settled);
    recorder
        .cache
        .insert(&key.0, &key.1, 1000, 4000, Vec::new(), TileSource::Core);
    assert_eq!(
        recorder.read_key(&key, 1000, 4000).unwrap().covered,
        vec![(1000, 4000)]
    );
}

/// Renewal protects an old watch while a stale watch is evicted and remaining leases expire.
#[test]
fn ninth_market_watch_evicts_earliest_expiry_and_expired_watches_drop() {
    let mut recorder = chart_recorder();
    let key = |index| ("CORE_A".to_string(), format!("XALPHA{index}"));
    for index in 0..8 {
        recorder.apply(
            Cmd::Watch {
                key: key(index),
                until_ms: 5000 + index,
            },
            1000 + index,
        );
    }
    recorder.apply(
        Cmd::Watch {
            key: key(0),
            until_ms: 6000,
        },
        2000,
    );
    recorder.apply(
        Cmd::Watch {
            key: key(8),
            until_ms: 5000,
        },
        2001,
    );
    assert_eq!(
        recorder
            .tasks
            .values()
            .filter(|task| task.watch_until().is_some())
            .count(),
        8
    );
    assert!(recorder.tasks[&key(0)].wants_pair());
    assert!(recorder.tasks[&key(1)].watch_until().is_none());
    assert!(recorder.tasks[&key(8)].wants_pair());
    recorder.expire_watches(5008);
    assert!(recorder.tasks[&key(0)].wants_pair());
    assert!(!recorder.tasks[&key(8)].wants_pair());
    recorder.expire_watches(6000);
    assert!(
        recorder
            .tasks
            .values()
            .all(|task| task.watch_until().is_none())
    );
    assert!(recorder.tasks.values().all(|task| !task.wants_pair()));
}

/// A mixed-unit or far-future lease must release the pair after sixty recorder-clock seconds.
#[test]
fn market_watch_clamps_extreme_lease_to_sixty_seconds() {
    let mut recorder = chart_recorder();
    let key = ("CORE_A".to_owned(), "XALPHA".to_owned());
    recorder.apply(
        Cmd::Watch {
            key: key.clone(),
            until_ms: i64::MAX,
        },
        1000,
    );
    recorder.expire_watches(60_999);
    assert!(recorder.tasks[&key].wants_pair());
    recorder.expire_watches(61_000);
    assert!(!recorder.tasks[&key].wants_pair());
    assert_eq!(recorder.tasks[&key].watch_until(), None);
}

/// Collapsing coverage to the fresh tail loses a fully recorded trade before a gap.
#[test]
fn recorder_span_prefers_fresh_coverage_after_a_gap() {
    let recorder = chart_recorder();
    let key = ("CORE_A".to_owned(), "XALPHA".to_owned());
    recorder
        .cache
        .insert(&key.0, &key.1, 1000, 100_000, Vec::new(), TileSource::Core);
    recorder.cache.insert(
        &key.0,
        &key.1,
        200_000,
        201_000,
        Vec::new(),
        TileSource::Core,
    );
    assert_eq!(
        recorder.read_key(&key, 1000, 201_000).unwrap().covered,
        vec![(1000, 100_000), (200_000, 201_000)]
    );
}

/// The read API must not start a recorder as a side effect of a chart request.
#[test]
fn market_tape_without_running_recorder_is_unknown() {
    assert!(TX.get().is_none_or(Option::is_none));
    assert!(matches!(
        market_tape("CORE_A", "XALPHA", 0, 1000, Duration::from_millis(1)),
        Err(TapeMiss::Unknown)
    ));
}

const MIN: i64 = 60_000;

/// A close revealing emulator metadata must discard its earlier unclassified open and buffer.
#[test]
fn emulator_known_only_at_close_is_closed_and_dropped() {
    let _guard = catchup::tests::SWITCH_LOCK.lock().unwrap();
    let previous = (catchup::max_age_days(), catchup::record_emulator());
    catchup::set_keep(0, false);
    let now = Instant::now();
    let mut recorder = Recorder {
        cache: TradeCache::open_evictable(":memory:".into()).unwrap(),
        catchup: catchup::RecorderCatchup::default(),
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
    let id = TradeId { core: 9, rec_id: 1 };
    let key = ("SYNTHETIC".to_string(), "SYNUSDT".to_string());
    recorder.apply(
        Cmd::Opened {
            exchange: key.0.clone(),
            market: key.1.clone(),
            trade: id,
            open_ms: 1000,
            emulator: None,
        },
        1000,
    );
    let task = recorder.tasks.get_mut(&key).unwrap();
    task.seed_asked(1000, 1);
    task.seeded(
        vec![Tick {
            time_ms: 1000.0,
            price: 1.0,
            qty: 1.0,
            side: crate::feed::Side::Buy,
        }],
        1000,
        1,
    );
    assert!(!task.unfiled(0, 2000).is_empty());
    recorder.apply(
        Cmd::Closed {
            exchange: key.0.clone(),
            market: key.1.clone(),
            venue: Some(crate::venue::Venue {
                brand: crate::venue::Brand::Bybit,
                kind: crate::venue::MarketKind::Futures,
            }),
            trade: id,
            open_ms: 1000,
            close_ms: 2000,
            emulator: Some(true),
        },
        2000,
    );
    assert!(!recorder.tasks.contains_key(&key));
    assert!(
        recorder
            .cache
            .held_spans(&key.0, &key.1, 0, 5000)
            .unwrap()
            .is_empty()
    );
    catchup::set_keep(previous.0, previous.1);
    catchup::take_purge_wanted();
}

/// Turning emulator recording off must discard queued seeds and buffers before any flush.
#[test]
fn recorder_drops_emulator_tasks_when_turned_off() {
    let _guard = catchup::tests::SWITCH_LOCK.lock().unwrap();
    let previous = (catchup::max_age_days(), catchup::record_emulator());
    catchup::set_keep(0, true);
    let now = Instant::now();
    let mut recorder = Recorder {
        cache: TradeCache::open_evictable(std::path::PathBuf::from(":memory:")).unwrap(),
        catchup: catchup::RecorderCatchup::default(),
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
    let emu = TradeId { core: 1, rec_id: 1 };
    let plain = TradeId { core: 1, rec_id: 2 };
    for (market, id, emulator) in [("XALPHA", emu, Some(true)), ("XBETA", plain, None)] {
        recorder.apply(
            Cmd::Opened {
                exchange: "SYNTHETIC".into(),
                market: market.into(),
                trade: id,
                open_ms: 1000,
                emulator,
            },
            1000,
        );
        let task = recorder
            .tasks
            .get_mut(&("SYNTHETIC".into(), market.into()))
            .unwrap();
        task.seed_asked(1000, 1);
        task.seeded(
            vec![Tick {
                time_ms: 1000.0,
                price: 1.0,
                qty: 1.0,
                side: crate::feed::Side::Buy,
            }],
            1000,
            1,
        );
        task.seed_asked(2000, 1);
    }
    catchup::set_keep(0, false);
    let task = recorder
        .tasks
        .get_mut(&("SYNTHETIC".into(), "XALPHA".into()))
        .unwrap();
    assert!(
        flush_to(task, Some(5000 + QUIET_TAIL_MS), true).is_none(),
        "shutdown flush must not file emulator tape"
    );
    assert!(task.seed().is_none());
    assert!(task.unfiled(0, 5000).is_empty());
    recorder.drop_emulator_tasks();
    assert!(
        !recorder
            .tasks
            .contains_key(&("SYNTHETIC".into(), "XALPHA".into()))
    );
    assert!(recorder.tasks[&("SYNTHETIC".into(), "XBETA".into())].knows(plain));
    recorder.apply(
        Cmd::Closed {
            exchange: "SYNTHETIC".into(),
            market: "XALPHA".into(),
            venue: Some(crate::venue::Venue {
                brand: crate::venue::Brand::Bybit,
                kind: crate::venue::MarketKind::Futures,
            }),
            trade: emu,
            open_ms: 1000,
            close_ms: 2000,
            emulator: Some(true),
        },
        2000,
    );
    assert!(
        !recorder
            .tasks
            .contains_key(&("SYNTHETIC".into(), "XALPHA".into()))
    );
    catchup::set_keep(previous.0, previous.1);
    catchup::take_purge_wanted();
}

/// A fresh open is an entry anywhere; a resend of a known trade changes nothing.
#[test]
fn a_fresh_open_is_recorded_everywhere() {
    for station in [false, true] {
        for known in [false, true] {
            assert_eq!(open_verdict(station, 0, known), OpenVerdict::Open);
            assert_eq!(
                open_verdict(station, FRESH_OPEN_MS, known),
                OpenVerdict::Open
            );
        }
    }
}

/// An old open is a resent row: the terminal skips it, a station takes up a trade it does not
/// know yet — one open across its own restart — and skips it once known or past the horizon.
#[test]
fn only_a_station_resumes_an_old_open_it_does_not_know() {
    let old = 30 * MIN;
    assert_eq!(open_verdict(false, old, false), OpenVerdict::Skip);
    assert_eq!(open_verdict(true, old, false), OpenVerdict::Resume);
    assert_eq!(open_verdict(true, old, true), OpenVerdict::Skip);
    assert_eq!(
        open_verdict(true, 24 * 3_600_000, false),
        OpenVerdict::Resume
    );
    assert_eq!(
        open_verdict(true, 24 * 3_600_000 + 1, false),
        OpenVerdict::Skip
    );
}

/// Ignoring queued bytes would admit two individually small writes across the disk reserve.
#[test]
fn pending_backfills_share_disk_reservation_until_confirmed() {
    let pending = Arc::new(AtomicU64::new(0));
    let first = reserve_backfill(&pending, 9, |total| total <= 15).unwrap();
    assert!(reserve_backfill(&pending, 9, |total| total <= 15).is_none());
    assert_eq!(pending.load(Ordering::Acquire), 9);
    drop(first);
    assert_eq!(pending.load(Ordering::Acquire), 0);
    let next = reserve_backfill(&pending, 9, |total| total <= 15).unwrap();
    drop(next);
    assert_eq!(pending.load(Ordering::Acquire), 0);
}

/// Requiring a public venue at close would lose venue-less trade windows from retention.
#[test]
fn a_close_on_an_unknown_venue_is_recorded_and_known_to_catchup() {
    let _guard = catchup::tests::SWITCH_LOCK.lock().unwrap();
    catchup::enable();
    let now = Instant::now();
    let mut recorder = Recorder {
        cache: TradeCache::open_evictable(":memory:".into()).unwrap(),
        catchup: catchup::RecorderCatchup::default(),
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
    let unknown = TradeId { core: 9, rec_id: 1 };
    let known = TradeId { core: 8, rec_id: 2 };
    let broker = ("CORE_A".to_string(), "XALPHA".to_string());
    let bybit = ("CORE_B".to_string(), "XBETA".to_string());
    for (key, id, venue) in [
        (&broker, unknown, None),
        (
            &bybit,
            known,
            Some(crate::venue::Venue {
                brand: crate::venue::Brand::Bybit,
                kind: crate::venue::MarketKind::Futures,
            }),
        ),
    ] {
        recorder.apply(
            Cmd::Opened {
                exchange: key.0.clone(),
                market: key.1.clone(),
                trade: id,
                open_ms: 1000,
                emulator: Some(false),
            },
            1000,
        );
        recorder.apply(
            Cmd::Closed {
                exchange: key.0.clone(),
                market: key.1.clone(),
                venue,
                trade: id,
                open_ms: 1000,
                close_ms: 2000,
                emulator: Some(false),
            },
            2000,
        );
    }
    assert!(
        recorder
            .tasks
            .get(&broker)
            .is_some_and(|t| t.knows(unknown)),
        "the unknown venue's trade is recorded"
    );
    assert!(
        recorder.catchup.knows(unknown),
        "venue-less closes retain their trade windows for catch-up and retention"
    );
    assert!(
        recorder.catchup.knows(known),
        "a known venue still reaches the catch-up"
    );
}

/// A listing page answers a deal-chart reader through the protection set, a live close after
/// it takes over, and a later page takes back — the order the closed map kept when it held both.
#[test]
fn a_listed_trade_is_peeked_through_the_protection_set() {
    let _guard = catchup::tests::SWITCH_LOCK.lock().unwrap();
    let mut recorder = chart_recorder();
    let key = ("CORE_A".to_string(), "XALPHA".to_string());
    let id = TradeId {
        core: 7,
        rec_id: 43,
    };
    let listed = catchup::CatchupTrade {
        id,
        exchange: key.0.clone(),
        market: key.1.clone(),
        venue: None,
        core: None,
        open_ms: 1500,
        close_ms: 2500,
        emulator: false,
        tunable: true,
    };
    let window = |recorder: &Recorder| {
        recorder
            .peek(id, 0, 0)
            .map(|tape| (tape.exchange, tape.market, tape.open_ms, tape.close_ms))
    };
    assert!(matches!(window(&recorder), Err(TapeMiss::Unknown)));
    let now_ms = crate::util::now_unix_ms_i64();
    recorder.apply(Cmd::Catchup(vec![listed.clone()]), now_ms);
    assert_eq!(
        window(&recorder).ok(),
        Some((key.0.clone(), key.1.clone(), 1500, 2500))
    );
    recorder.closed.closed(id, &key, 1600, 2600);
    assert_eq!(
        window(&recorder).ok(),
        Some((key.0.clone(), key.1.clone(), 1600, 2600))
    );
    recorder.apply(Cmd::Catchup(vec![listed]), now_ms);
    assert_eq!(window(&recorder).ok(), Some((key.0, key.1, 1500, 2500)));
}
