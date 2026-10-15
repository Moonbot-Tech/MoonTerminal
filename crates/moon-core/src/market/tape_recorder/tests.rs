use super::*;

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
        open_verdict(true, OPEN_HORIZON_MS, false),
        OpenVerdict::Resume
    );
    assert_eq!(
        open_verdict(true, OPEN_HORIZON_MS + 1, false),
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

/// LinKvo 15.10: a core on a venue this build's directory does not know (a broker connected
/// through MoonProto) got no deal picture — its trades never reached the recorder. Its close is
/// now recorded like any other, and only the catch-up, which needs the venue's REST route, leaves
/// it out; a known venue's close still reaches the catch-up as before.
#[test]
fn a_close_on_an_unknown_venue_is_recorded_without_catchup() {
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
    let broker = ("77:00000000".to_string(), "ROLO".to_string());
    let bybit = ("2:00000000".to_string(), "SYNUSDT".to_string());
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
        !recorder.catchup.knows(unknown),
        "no REST route: no catch-up"
    );
    assert!(
        recorder.catchup.knows(known),
        "a known venue still reaches the catch-up"
    );
}
