use super::super::station::drop_held;
use super::*;
use moon_core::db::tuner::ticks::tail::{floor_ms, required_with_floor};
use moon_core::market::trade_replay::ReplayWindow;

mod replay;

/// Clearing growth at the read loses older deals when cancellation interrupts station I/O.
#[test]
fn barrier_b6_cancelled_growth_keeps_whole_history_until_a_current_pass_completes() {
    let _restore = RestoreAutoload::take();
    {
        let mut st = lock();
        st.whole_history = true;
        st.phase = Phase::Running;
    }
    let pass = Pass {
        generation: generation(),
        station: None,
        station_only: false,
        whole_history: true,
    };
    let mut sources = FakeSources {
        prefill: Box::new(|rows, _, _| {
            reset_for_station();
            (rows, 0, 0)
        }),
        venue_rows: 0,
        core_rows: 0,
        deals: Vec::new(),
        from: None,
    };
    run_pass_with(
        resolver(),
        HashMap::new(),
        Default::default(),
        None,
        1,
        &pass,
        &mut sources,
    );
    assert!(
        lock().whole_history,
        "cancelled growth must reread older history"
    );
    {
        let mut st = lock();
        st.phase = Phase::Running;
        st.result = Some((Vec::new(), 1));
    }
    tick_with(
        || true,
        || false,
        || None,
        |_, _, _, _, _, _| panic!("finish only"),
    );
    assert!(!lock().whole_history);
}

/// Discovering a station during resolution must cancel the queued fallback handoff.
#[test]
fn barrier_b6_station_arriving_mid_pass_prevents_venue_handoff() {
    let _restore = RestoreAutoload::take();
    lock().phase = Phase::Running;
    let pass = Pass {
        generation: generation(),
        station: None,
        station_only: false,
        whole_history: false,
    };
    let mut deal = super::super::tests::synthetic_deal();
    deal.buy_ms = moon_core::util::time::now_unix_ms_i64() - 20_000;
    deal.close_ms = deal.buy_ms + 10_000;
    let mut sources = FakeSources {
        prefill: Box::new(|rows, _, _| {
            tick_with(
                || true,
                || true,
                || {
                    Some(moon_remote::ssh::Target {
                        host: "synthetic.invalid".into(),
                        port: 22,
                    })
                },
                |_, _, _, _, _, _| panic!("arrival must rearm before launch"),
            );
            (rows, 0, 0)
        }),
        venue_rows: 0,
        core_rows: 0,
        deals: Vec::new(),
        from: None,
    };
    let mut venue_resolver = resolver();
    for address in venue_resolver.addresses.values_mut().flatten() {
        std::sync::Arc::make_mut(address).venue = Some(moon_core::venue::Venue {
            brand: moon_core::venue::Brand::Binance,
            kind: moon_core::venue::MarketKind::Futures,
        });
    }
    run_pass_with(
        venue_resolver,
        HashMap::new(),
        Default::default(),
        Some(vec![deal]),
        1,
        &pass,
        &mut sources,
    );
    assert_eq!((sources.venue_rows, sources.core_rows), (0, 0));
}

/// A fake station transport retaining the production pass's source decisions.
type FakePrefill<'a> = dyn FnMut(
        Vec<QueuedRow>,
        Option<&moon_remote::ssh::Target>,
        &dyn Fn() -> bool,
    ) -> (Vec<QueuedRow>, usize, usize)
    + 'a;

/// Count every admitted downstream leg without using a real venue or core connection.
struct FakeSources<'a> {
    prefill: Box<FakePrefill<'a>>,
    venue_rows: usize,
    core_rows: usize,
    deals: Vec<Deal>,
    from: Option<i64>,
}

impl Sources for FakeSources<'_> {
    /// Apply ordinary report time comparisons to synthetic rows, independent of horizon policy.
    fn read(
        &mut self,
        query: &moon_core::db::analytics::Query,
    ) -> Result<Vec<Deal>, moon_core::db::ReadFail> {
        self.from = Some(query.from);
        Ok(self
            .deals
            .iter()
            .filter(|deal| deal.close_ms / 1_000 >= query.from && deal.close_ms / 1_000 < query.to)
            .cloned()
            .collect())
    }
    /// Feed the station answer into the unchanged scheduling pass.
    fn prefill(
        &mut self,
        rows: Vec<QueuedRow>,
        target: Option<&moon_remote::ssh::Target>,
        stop: &dyn Fn() -> bool,
    ) -> (Vec<QueuedRow>, usize, usize) {
        (self.prefill)(rows, target, stop)
    }
    /// Record requests that would reach the venue queue.
    fn enqueue(&mut self, rows: Vec<QueuedRow>, _: HashMap<String, f64>) -> usize {
        self.venue_rows += rows.len();
        rows.len()
    }
    /// Record requests that would reach core archives.
    fn cores(&mut self, rows: Vec<QueuedRow>, _: u64) {
        self.core_rows += rows.len();
    }
}

/// Resolve a synthetic deal without waiting for a live core catalogue.
fn resolver() -> FetchResolver {
    FetchResolver {
        source: moon_core::fixture::unlisted::source(1),
        quotes: Default::default(),
        addresses: HashMap::from([(
            (1, "XALPHA".into()),
            Some(std::sync::Arc::new(
                crate::analytics::tuner::ticks::state::RowAddress {
                    core_uid: 1,
                    venue: None,
                    exchange_key: moon_core::fixture::unlisted::EXCHANGE_KEY.into(),
                    market: "XALPHA".into(),
                    btc_market: None,
                },
            )),
        )]),
        btc_markets: Default::default(),
        archive: None,
    }
}

/// Backoff must never route a configured station's missing tape to core/venue legs.
#[test]
fn station_policy_blocks_fallback_even_without_a_reachable_target() {
    let _restore = RestoreAutoload::take();
    let mut deal = super::super::tests::synthetic_deal();
    deal.buy_ms = moon_core::util::time::now_unix_ms_i64() - 20_000;
    deal.close_ms = deal.buy_ms + 10_000;
    for station_only in [true, false] {
        let pass = Pass {
            generation: generation(),
            station: None,
            station_only,
            whole_history: false,
        };
        let mut sources = FakeSources {
            prefill: Box::new(|rows, _, _| (rows, 0, 0)),
            venue_rows: 0,
            core_rows: 0,
            deals: Vec::new(),
            from: None,
        };
        let left = run_pass_with(
            resolver(),
            HashMap::new(),
            Default::default(),
            Some(vec![deal.clone()]),
            1,
            &pass,
            &mut sources,
        );
        assert_eq!(sources.venue_rows, 0);
        assert_eq!(sources.core_rows, usize::from(!station_only));
        assert_eq!(left.len(), usize::from(station_only));
    }
}

/// A cancel delivered during station I/O must prevent both downstream handoffs.
#[test]
fn cancellation_mid_pass_never_enqueues_or_files_core_tape() {
    let _restore = RestoreAutoload::take();
    lock().phase = Phase::Running;
    let mut deal = super::super::tests::synthetic_deal();
    deal.buy_ms = moon_core::util::time::now_unix_ms_i64() - 20_000;
    deal.close_ms = deal.buy_ms + 10_000;
    let pass = Pass {
        generation: generation(),
        station: None,
        station_only: false,
        whole_history: false,
    };
    let mut sources = FakeSources {
        prefill: Box::new(|rows, _, _| {
            reset_for_station();
            (rows, 0, 0)
        }),
        venue_rows: 0,
        core_rows: 0,
        deals: Vec::new(),
        from: None,
    };
    let left = run_pass_with(
        resolver(),
        HashMap::new(),
        Default::default(),
        Some(vec![deal]),
        1,
        &pass,
        &mut sources,
    );
    assert!(left.is_empty());
    assert_eq!((sources.venue_rows, sources.core_rows), (0, 0));
    assert!(matches!(lock().phase, Phase::Armed));
}

/// Restore scheduler state even if a synthetic pass fails an assertion.
struct RestoreAutoload {
    saved: Autoload,
    _window: crate::test_support::SharedWindowGuard,
}

impl RestoreAutoload {
    /// Isolate global autoload state while retaining the real scheduler and pass code.
    fn take() -> Self {
        let window = crate::test_support::SharedWindowGuard::take();
        Self {
            saved: std::mem::take(&mut *lock()),
            _window: window,
        }
    }
}

impl Drop for RestoreAutoload {
    /// Restore the scheduler before releasing process-wide settings isolation.
    fn drop(&mut self) {
        *lock() = std::mem::take(&mut self.saved);
    }
}

/// The startup horizon stays bounded while a growth pass can select a forty-day-old deal.
#[test]
fn growth_reads_whole_history_but_ordinary_startup_reads_thirty_days() {
    let now = 100 * 86_400_000;
    let mut old = super::super::tests::synthetic_deal();
    old.close_ms = now - 40 * 86_400_000;
    old.buy_ms = old.close_ms - 10_000;
    let mut sources = FakeSources {
        prefill: Box::new(|_, _, _| panic!("query selection must not fetch")),
        venue_rows: 0,
        core_rows: 0,
        deals: vec![old],
        from: None,
    };
    assert!(
        read_recent(Default::default(), now, false, &mut sources)
            .unwrap()
            .is_empty()
    );
    assert_eq!(sources.from, Some(70 * 86_400));
    assert_eq!(
        read_recent(Default::default(), now, true, &mut sources)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(sources.from, Some(0));
}

/// Held rows leave progress before I/O; detached filing stays visible until completion or stop.
#[test]
fn pending_count_excludes_held_rows_and_releases_cancelled_core_work() {
    let _restore = RestoreAutoload::take();
    {
        let mut st = lock();
        st.phase = Phase::Running;
        st.generation = 7;
        st.pending = 10;
    }
    held_rows_dropped(7, 7, 2);
    assert_eq!(pending_rows(), Some(5));
    held_rows_dropped(6, 10, 0);
    assert_eq!(
        pending_rows(),
        Some(5),
        "obsolete passes cannot change the current count"
    );
    lock().phase = Phase::Done;
    let mut cores = CoreWork::new(2);
    assert_eq!(pending_rows(), Some(2));
    cores.filed_one();
    assert_eq!(pending_rows(), Some(1));
    drop(cores);
    assert_eq!(pending_rows(), None);
}

/// A production scheduled pass widens held tape without a tuner and resumes with no requests.
#[test]
fn scheduled_growth_fills_both_gaps_without_a_tuner_and_resumes_without_requests() {
    use super::super::station::{Filled, lacking, prefill_rows};
    use moon_core::feed::types::{Side, Tick};
    use moon_core::market::trade_replay::trade_cache::TradeCache;
    let _restore = RestoreAutoload::take();
    let started = Instant::now();
    moon_core::market::trade_replay::set_margin_s(600);
    let cache = TradeCache::open_with_ceiling(":memory:".into(), || None).unwrap();
    let mut deal = super::super::tests::synthetic_deal();
    deal.buy_ms = 1_000_000;
    deal.close_ms = 1_010_000;
    let exchange = moon_core::fixture::unlisted::EXCHANGE_KEY;
    let tick = |time: i64| Tick {
        time_ms: time as f64,
        price: 1.0,
        qty: 1.0,
        side: Side::Buy,
    };
    cache.insert_from_station(
        exchange,
        "XALPHA",
        820_000,
        1_190_000,
        vec![tick(820_000), tick(1_190_000)],
    );
    assert!(cache.sync(Duration::from_secs(5)));
    assert_eq!(
        cache
            .held_spans(exchange, "XALPHA", 820_000, 1_190_000)
            .unwrap(),
        vec![(820_000, 1_190_000)]
    );
    let mut issued = 0;
    let old = moon_core::station_api::TapeWindow {
        margin_s: 180,
        long_position_min: 5,
    };
    let new = moon_core::station_api::TapeWindow {
        margin_s: 600,
        long_position_min: 5,
    };
    lock().phase = Phase::Done;
    window_grown(old, new);
    for resumed in [false, true] {
        if resumed {
            lock().phase = Phase::Armed;
        }
        tick_with(
            || true,
            || false,
            || None,
            |_, _, _, _, _, _| panic!("first delay"),
        );
        if let Phase::Waiting { due, .. } = &mut lock().phase {
            *due = Instant::now();
        }
        let mut captured = None;
        tick_with(
            || true,
            || true,
            || {
                Some(moon_remote::ssh::Target {
                    host: "synthetic.invalid".into(),
                    port: 22,
                })
            },
            |_, _, generation, station, station_only, whole_history| {
                captured = Some(Pass {
                    generation,
                    station,
                    station_only,
                    whole_history,
                });
                true
            },
        );
        let pass = captured.unwrap();
        assert!(pass.station_only);
        assert_eq!(pass.whole_history, !resumed);
        let place = |row: &QueuedRow| {
            (
                row.address.exchange_key.clone(),
                row.address.market.clone(),
                row.window,
            )
        };
        let held =
            |exchange: &str, market: &str, from, to| cache.held_spans(exchange, market, from, to);
        let requests = std::cell::Cell::new(0);
        let mut sources = FakeSources {
            venue_rows: 0,
            core_rows: 0,
            deals: vec![deal.clone()],
            from: None,
            prefill: Box::new(|rows, target, stop| {
                assert_eq!(target.unwrap().host, "synthetic.invalid");
                assert!(!stop());
                let (rows, held_count, station_count, _) = prefill_rows(
                    rows,
                    place,
                    held,
                    |_, _| false,
                    |rows| {
                        let wants = lacking(rows, place, held);
                        requests.set(requests.get() + wants.len());
                        assert_eq!(
                            wants[0].spans,
                            vec![(400_000, 819_999), (1_190_001, 1_610_000)]
                        );
                        for want in wants {
                            for (from, to) in want.spans {
                                cache.insert_from_station(
                                    &want.exchange,
                                    &want.market,
                                    from,
                                    to,
                                    vec![tick(from), tick(to)],
                                );
                            }
                        }
                        assert!(cache.sync(Duration::from_secs(5)));
                        Filled {
                            markets: 1,
                            pieces: 2,
                            ..Default::default()
                        }
                    },
                );
                (rows, held_count, station_count)
            }),
        };
        let left = run_pass_with(
            resolver(),
            HashMap::new(),
            Default::default(),
            resumed.then(|| vec![deal.clone()]),
            1,
            &pass,
            &mut sources,
        );
        assert!(left.is_empty());
        assert_eq!(sources.from, (!resumed).then_some(0));
        assert_eq!(requests.get(), usize::from(!resumed));
        issued += requests.get();
        assert_eq!((sources.venue_rows, sources.core_rows), (0, 0));
        lock().result = Some((left, 1));
        tick_with(
            || true,
            || false,
            || None,
            |_, _, _, _, _, _| panic!("result must finish"),
        );
        assert!(matches!(lock().phase, Phase::Done));
        assert_eq!(pending_rows(), None);
    }
    let spans = cache.read(exchange, "XALPHA", 400_000, 1_610_000).unwrap();
    assert_eq!(
        spans
            .iter()
            .map(|span| (span.from_ms, span.to_ms))
            .collect::<Vec<_>>(),
        vec![
            (400_000, 819_999),
            (820_000, 1_190_000),
            (1_190_001, 1_610_000)
        ]
    );
    let times: Vec<_> = spans
        .iter()
        .flat_map(|span| span.ticks.iter().map(|tick| tick.time_ms as i64))
        .collect();
    assert_eq!(
        times,
        vec![400_000, 819_999, 820_000, 1_190_000, 1_190_001, 1_610_000]
    );
    let unique: std::collections::HashSet<_> = times.iter().copied().collect();
    assert_eq!(unique.len(), times.len(), "duplicate seam prints");
    for pair in spans.windows(2) {
        assert_eq!(pair[0].to_ms + 1, pair[1].from_ms, "seam gap");
    }
    assert_eq!(issued, 1, "one merged market request is needed");
    println!(
        "[replay] deals=1 covered_180_before=1 covered_600_after=1 seams_checked=2 duplicates={} requests_issued={issued} requests_needed=1 elapsed_ms={}",
        times.len() - unique.len(),
        started.elapsed().as_millis()
    );
    assert!(
        cache
            .held_spans("other", "XALPHA", 400_000, 1_610_000)
            .unwrap()
            .is_empty()
    );
}

/// A wider window must invalidate a finished pass; shrinking must preserve its completion.
#[test]
fn growing_window_rearms_and_cancels_the_previous_generation() {
    let _guard = crate::test_support::SharedWindowGuard::take();
    let old = moon_core::station_api::TapeWindow {
        margin_s: 180,
        long_position_min: 5,
    };
    let new = moon_core::station_api::TapeWindow {
        margin_s: 600,
        long_position_min: 30,
    };
    let saved = {
        let mut st = lock();
        let saved = std::mem::take(&mut *st);
        st.phase = Phase::Done;
        saved
    };
    let before = generation();
    window_grown(new, old);
    assert!(matches!(lock().phase, Phase::Done));
    window_grown(old, new);
    let mut st = lock();
    let rearmed = matches!(st.phase, Phase::Armed);
    let cancelled = st.generation != before;
    *st = saved;
    assert!(rearmed, "growth must rearm a completed startup pass");
    assert!(cancelled, "old in-flight work must become obsolete");
}

/// Directory absence must select cores for a recent EX-XX row and stop after core retention.
#[test]
fn unlisted_rows_use_cores_only_within_retention() {
    let now = 100 * 86_400_000;
    for (age, expected) in [
        (3_600_000, RowPath::Cores),
        (49 * 3_600_000, RowPath::TooOldForCores),
    ] {
        let source = fetchable(None, "XALPHA", now - age, now);
        assert_eq!(
            row_path(
                matches!(source, Fetchable::Rest | Fetchable::Archive),
                now - age,
                now
            ),
            expected
        );
    }
}

/// A trade `(exchange, market, open_ms, close_ms)` and its window.
fn trade(market: &str, open_ms: i64, close_ms: i64) -> (String, String, ReplayWindow) {
    let window = moon_core::market::trade_replay::replay_window_ms(open_ms, close_ms, 30_000)
        .expect("a window");
    ("x".to_string(), market.to_string(), window)
}

/// A row whose every needed stretch the disk holds is dropped; one with a hole, one on a market
/// the disk has nothing of, and every row of a market whose read did not happen are kept.
#[test]
fn only_the_rows_the_disk_fully_holds_are_dropped() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let base = 1_790_000_000_000;
    let held_trade = trade("M", base, base + 10_000);
    let need = required_with_floor(
        &held_trade.2,
        floor_ms(super::super::super::tail::current_s()),
    )
    .hull()
    .expect("a need");
    let hole_trade = trade("M", base + 3_600_000, base + 3_610_000);
    let other_market = trade("N", base, base + 10_000);
    let unread_market = trade("U", base, base + 10_000);
    let rows = vec![held_trade, hole_trade, other_market, unread_market];
    let (kept, held) = drop_held(
        rows,
        |row| row.clone(),
        |_, market, _, _| match market {
            // Two abutting spans over the first trade's need: one stretch.
            "M" => Some(vec![(need.0 - 5, need.0 + 100), (need.0 + 101, need.1 + 5)]),
            "N" => Some(Vec::new()),
            _ => None,
        },
        |_, _| false,
    );
    assert_eq!(held, 1);
    let mut markets: Vec<(String, i64)> = kept
        .iter()
        .map(|(_, m, w)| (m.clone(), w.open_ms))
        .collect();
    markets.sort();
    assert_eq!(
        markets,
        vec![
            ("M".to_string(), base + 3_600_000),
            ("N".to_string(), base),
            ("U".to_string(), base),
        ]
    );
}

/// The old deal is held with its short tape; the fresh deal must still reach the fetch queue.
#[test]
fn old_short_tape_is_held_while_a_fresh_tail_stays_queued() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let now = 100 * 86_400_000;
    let old_close = now - 49 * 3_600_000;
    let fresh_close = now - 3_600_000;
    let address = crate::analytics::tuner::ticks::state::RowAddress {
        core_uid: 1,
        venue: None,
        exchange_key: "EX-XX".into(),
        market: "XALPHA".into(),
        btc_market: None,
    };
    let (queued, held) = drop_held(
        vec![
            trade("OLD", old_close - 10_000, old_close),
            trade("FRESH", fresh_close - 10_000, fresh_close),
        ],
        |row| row.clone(),
        |_, market, _, _| {
            let close = if market == "OLD" {
                old_close
            } else {
                fresh_close
            };
            Some(vec![(close - 40_000, close + 30_000)])
        },
        |row, _| {
            !crate::analytics::tuner::ticks::tail::fillable_at(&address, 1, row.2.close_ms, 0, now)
        },
    );
    assert_eq!(held, 1);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].1, "FRESH");
}

/// The venue takes what it serves (its fetch asks the core first) and the
/// cores take the rest that is recent enough.
#[test]
fn venue_takes_what_it_serves_and_cores_take_recent_remainder() {
    let now = 1_790_000_000_000;
    let recent = now - 3_600_000;
    assert_eq!(row_path(true, recent, now), RowPath::Venue);
    assert_eq!(row_path(false, recent, now), RowPath::Cores);
    assert_eq!(
        row_path(false, now - CORE_ARCHIVE_RETENTION_MS - 1, now),
        RowPath::TooOldForCores
    );
}

/// The tick holds the autoload lock on the UI thread while it starts a pass; a second take on the
/// same thread must fail loudly, never hang the terminal as the station-target read once did.
#[test]
#[should_panic(expected = "autoload lock taken twice")]
fn a_second_take_of_the_lock_on_one_thread_panics_instead_of_hanging() {
    let _held = super::lock();
    let _again = super::lock();
}

/// A tick-style caller reads the station target from the lock it already holds, and returns.
#[test]
fn the_station_target_is_read_under_the_held_lock_without_retaking_it() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let held = super::lock();
        let _ = super::station_target_of(&held);
        drop(held);
        tx.send(()).unwrap();
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("station target read under the held lock did not return");
}
