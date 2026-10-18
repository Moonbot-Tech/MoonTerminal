use super::*;
use std::io::{Cursor, Write};

/// A successful retry cannot renew a day's allowance before the failure cooldown expires.
#[test]
fn barrier_b6_success_does_not_reset_failed_day_attempts() {
    /// A body that disconnects after its prefix has been received.
    struct Broken;
    impl Read for Broken {
        /// Model a transport read failure after the cursor prefix has reached disk.
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "synthetic disconnect",
            ))
        }
    }
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let calls = std::cell::Cell::new(0);
    for pass in 0..5 {
        let result = fill_with(
            route,
            "SYNINTERMITTENT",
            &Coverage::one((1000, 2000)),
            &ReplayGate::new(),
            (root.path(), vec![day()]),
            (|| Some(u64::MAX), u64::MAX),
            |_| {
                let attempt = calls.get();
                calls.set(attempt + 1);
                if attempt == 1 {
                    return Ok(response(body(route)));
                }
                Ok(DayResponse {
                    status: 200,
                    retry_after_s: None,
                    content_length: None,
                    reader: Box::new(Cursor::new(vec![0; 16]).chain(Broken)),
                })
            },
        );
        if pass == 1 {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(ArchiveError::Transient(_))));
        }
    }
    assert_eq!(
        calls.get(),
        4,
        "three failed bodies plus one success, then no further transport"
    );
}

/// The terminal's reserve callback must reject a small disk before archive transport starts.
#[test]
fn barrier_b6_terminal_reserve_returns_no_room() {
    let root = root();
    let result = fill_with(
        ArchiveRoute::BinanceUmDaily,
        "SYNTERMINAL",
        &Coverage::one((1000, 2000)),
        &ReplayGate::new(),
        (root.path(), vec![day()]),
        (
            || reserve_budget(1024 * 1024 * 1024, 10 * 1024 * 1024 * 1024),
            u64::MAX,
        ),
        |_| panic!("terminal disk reserve must precede HTTP"),
    );
    assert_eq!(result.unwrap_err(), ArchiveError::NoRoom);
}

/// Body writes must charge extraction headroom against one baseline, without repeated probes.
#[test]
fn barrier_b6_download_snapshots_free_space_once() {
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let bytes = body(route);
    let probes = std::cell::Cell::new(0);
    let length = bytes.len() as u64;
    let result = fill_with(
        route,
        "SYNSNAPSHOT",
        &Coverage::one((1000, 2000)),
        &ReplayGate::new(),
        (root.path(), vec![day()]),
        (
            || {
                probes.set(probes.get() + 1);
                Some(length * 4)
            },
            u64::MAX,
        ),
        |_| Ok(response(bytes.clone())),
    )
    .unwrap();
    assert_eq!(result.ticks.len(), 2);
    assert_eq!(probes.get(), 1);
}

/// Per-chunk reserve probes multiply syscall cost by 1024 for each 64 MiB checkpoint.
#[test]
fn barrier_b6_stream_guard_is_not_polled_per_chunk() {
    let probes = std::cell::Cell::new(0);
    let copied = copy_bounded(
        std::io::repeat(0).take(2 * 1024 * 1024),
        &mut std::io::sink(),
        u64::MAX,
        None,
        |_| {
            probes.set(probes.get() + 1);
            true
        },
    )
    .unwrap();
    assert_eq!(copied, 2 * 1024 * 1024);
    assert!(
        probes.get() <= 2,
        "reserve syscalls per chunk: {}",
        probes.get()
    );
}

/// Failed large days must stop consuming transport after three failures in one process.
#[test]
fn barrier_b6_three_corrupt_attempts_latch_a_day() {
    let root = root();
    let calls = std::cell::Cell::new(0);
    for _ in 0..4 {
        let result = fill_with(
            ArchiveRoute::BinanceUmDaily,
            "SYNFAIL",
            &Coverage::one((0, 2000)),
            &ReplayGate::new(),
            (root.path(), vec![day()]),
            (|| Some(u64::MAX), u64::MAX),
            |_| {
                calls.set(calls.get() + 1);
                Ok(response(b"broken zip".to_vec()))
            },
        );
        assert!(matches!(result, Err(ArchiveError::Corrupt(_))));
    }
    assert_eq!(calls.get(), 3);
}

/// A missing day in a mixed window must not be downloaded again on every retry.
#[test]
fn barrier_b6_mixed_window_remembers_missing_daily_files() {
    let root = root();
    let missing_calls = std::cell::Cell::new(0);
    let route = ArchiveRoute::BinanceUmDaily;
    for _ in 0..2 {
        let answer = fill_with(
            route,
            "SYNMIX",
            &Coverage::one((0, 2 * 86_400_000 - 1)),
            &ReplayGate::new(),
            (root.path(), vec![day(), day().succ_opt().unwrap()]),
            (|| Some(u64::MAX), u64::MAX),
            |url| {
                if url.contains("1970-01-01") {
                    missing_calls.set(missing_calls.get() + 1);
                    let mut answer = response(Vec::new());
                    answer.status = 404;
                    Ok(answer)
                } else {
                    Ok(response(body(route)))
                }
            },
        )
        .unwrap();
        assert_eq!(
            answer.covered,
            Coverage::one((86_400_000, 2 * 86_400_000 - 1))
        );
    }
    assert_eq!(missing_calls.get(), 1);
}

/// A pre-HTTP reserve refusal must recover when the caller frees disk space.
#[test]
fn barrier_b7_pre_http_no_room_recovers() {
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let spans = Coverage::one((1000, 1500));
    assert_eq!(
        fill_with(
            route,
            "SYNNOROOM",
            &spans,
            &ReplayGate::new(),
            (root.path(), vec![day()]),
            (|| None, u64::MAX),
            |_| panic!("reserve before HTTP")
        )
        .unwrap_err(),
        ArchiveError::NoRoom
    );
    let calls = std::cell::Cell::new(0);
    let again = fill_with(
        route,
        "SYNNOROOM",
        &spans,
        &ReplayGate::new(),
        (root.path(), vec![day()]),
        (|| Some(u64::MAX), u64::MAX),
        |_| {
            calls.set(calls.get() + 1);
            Ok(response(body(route)))
        },
    );
    assert_eq!(calls.get(), 1, "free space must permit retry");
    assert!(again.is_ok());
}

/// Recent publication delays must not turn a daily 404 into a permanent negative cache.
#[test]
fn barrier_b7_recent_404_is_requested_again() {
    let root = root();
    let recent = chrono::DateTime::from_timestamp_millis(crate::util::time::now_unix_ms_i64())
        .unwrap()
        .date_naive()
        .pred_opt()
        .unwrap();
    let calls = std::cell::Cell::new(0);
    for _ in 0..2 {
        let result = fill_with(
            ArchiveRoute::BinanceUmDaily,
            "SYNRECENT",
            &Coverage::one((0, 2000)),
            &ReplayGate::new(),
            (root.path(), vec![recent]),
            (|| Some(u64::MAX), u64::MAX),
            |_| {
                calls.set(calls.get() + 1);
                let mut answer = response(Vec::new());
                answer.status = 404;
                Ok(answer)
            },
        );
        assert_eq!(result.unwrap_err(), ArchiveError::NotPublished);
    }
    assert_eq!(calls.get(), 2);
}

/// A CDN outage before a body starts must not spend the failed-transfer allowance.
#[test]
fn barrier_b7_connection_and_http_failures_recover() {
    for http in [false, true] {
        let root = root();
        let route = ArchiveRoute::BinanceUmDaily;
        let calls = std::cell::Cell::new(0);
        for pass in 0..4 {
            let result = fill_with(
                route,
                "SYNCONNECT",
                &Coverage::one((0, 2000)),
                &ReplayGate::new(),
                (root.path(), vec![day()]),
                (|| Some(u64::MAX), u64::MAX),
                |_| {
                    calls.set(calls.get() + 1);
                    if pass == 3 {
                        return Ok(response(body(route)));
                    }
                    if http {
                        let mut answer = response(Vec::new());
                        answer.status = 503;
                        Ok(answer)
                    } else {
                        Err(ArchiveError::Transient(
                            "synthetic connection failure".into(),
                        ))
                    }
                },
            );
            if pass == 3 {
                assert!(result.is_ok(), "host recovered: {result:?}");
            } else {
                assert!(result.is_err());
            }
        }
        assert_eq!(calls.get(), 4);
    }
}

/// A failed streamed day stays quiet for 24 hours and becomes fetchable at the boundary.
#[test]
fn barrier_b7_mid_body_latch_expires_after_24_hours() {
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let now = Instant::now();
    let calls = std::cell::Cell::new(0);
    for (offset, success) in [
        (0, false),
        (0, false),
        (0, false),
        (86_399, false),
        (86_400, true),
    ] {
        let result = fill_at(
            route,
            "SYNTTL",
            &Coverage::one((0, 2000)),
            &ReplayGate::new(),
            (root.path(), vec![day()], || {
                now + Duration::from_secs(offset)
            }),
            (|| Some(u64::MAX), u64::MAX),
            |_| {
                calls.set(calls.get() + 1);
                Ok(response(if success {
                    body(route)
                } else {
                    b"broken zip".to_vec()
                }))
            },
        );
        if success {
            assert!(result.is_ok(), "latch expired: {result:?}");
        } else {
            assert!(matches!(result, Err(ArchiveError::Corrupt(_))));
        }
        if offset == 86_399 {
            assert_eq!(calls.get(), 3);
        }
    }
    assert_eq!(calls.get(), 4);
}

/// A streamed disk refusal must skip repeated downloads until the measured budget increases.
#[test]
fn barrier_b7_mid_body_no_room_recovers_only_with_more_room() {
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let calls = std::cell::Cell::new(0);
    for pass in 0..3 {
        let result = fill_with(
            route,
            "SYNROOM",
            &Coverage::one((0, 2000)),
            &ReplayGate::new(),
            (root.path(), vec![day()]),
            (|| Some(if pass == 2 { u64::MAX } else { 16 }), u64::MAX),
            |_| {
                calls.set(calls.get() + 1);
                let mut answer = response(if pass == 2 { body(route) } else { vec![0; 32] });
                answer.content_length = None;
                Ok(answer)
            },
        );
        if pass == 2 {
            assert!(result.is_ok(), "disk recovered: {result:?}");
        } else {
            assert_eq!(result.unwrap_err(), ArchiveError::NoRoom);
        }
        if pass == 1 {
            assert_eq!(calls.get(), 1);
        }
    }
    assert_eq!(calls.get(), 2);
}

thread_local! { static PACES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Any pacing callback under the shared host lock would delay terminal-side archive users.
fn assert_pacing_without_host_lock() {
    assert!(
        download_lock(ArchiveRoute::BybitTradingDaily.host())
            .try_lock()
            .is_ok()
    );
    PACES.with(|count| count.set(count.get() + 1));
}

/// Download and decode pacing must never sleep under a host lock, including transport reads.
#[test]
fn catchup_pacing_never_waits_under_the_host_lock() {
    let root = root();
    PACES.with(|count| count.set(0));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(b"timestamp,side,size,price\n").unwrap();
    for _ in 0..100_000 {
        gz.write_all(b"1,Sell,2,10\n").unwrap();
    }
    let bytes = gz.finish().unwrap();
    let filled = with_pacing(assert_pacing_without_host_lock, || {
        fill_with(
            ArchiveRoute::BybitTradingDaily,
            "XALPHA",
            &Coverage::one((0, 3000)),
            &ReplayGate::default(),
            (root.path(), vec![day()]),
            (|| Some(u64::MAX), u64::MAX),
            |_| Ok(response(bytes.clone())),
        )
    })
    .unwrap();
    assert_eq!(filled.ticks.len(), 100_000);
    assert!(
        PACES.with(|count| count.get()) >= 3,
        "expected job-edge, extraction, and in-decoder pacing"
    );
}

/// Allocate isolated scratch storage without touching configured production data.
fn root() -> Scratch {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "moon-archive-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    Scratch(path)
}

/// Own an isolated synthetic archive directory until all fixture readers have closed.
struct Scratch(PathBuf);
impl Scratch {
    /// Return the fixture root to the same production pipeline used by `fill`.
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Scratch {
    /// Remove only this test's uniquely created directory.
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Build synthetic compressed day files for the real extraction pipeline.
fn body(route: ArchiveRoute) -> Vec<u8> {
    match route.format() {
        ArchiveFormat::ZipCsv => {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            zip.start_file(
                "SYN-aggTrades.csv",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"1,10,2,1,1,1000,true\n2,11,3,2,2,2000,false\n")
                .unwrap();
            zip.finish().unwrap().into_inner()
        }
        ArchiveFormat::GzCsv => {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            gz.write_all(b"timestamp,side,size,price\n1,Sell,2,10\n2,Buy,3,11\n")
                .unwrap();
            gz.finish().unwrap()
        }
    }
}

/// Return a streaming fixture response with the compressed byte length.
fn response(bytes: Vec<u8>) -> DayResponse {
    DayResponse {
        status: 200,
        retry_after_s: None,
        content_length: Some(bytes.len() as u64),
        reader: Box::new(Cursor::new(bytes)),
    }
}

/// Use a fixed published day so fixture rows need no wall-clock-relative timestamps.
fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
}

/// Losing UTC splitting or the publication lag would request tomorrow's missing file repeatedly.
#[test]
fn days_cross_midnight_and_exclude_unpublished_days() {
    let spans = Coverage::one((86_399_000, 3 * 86_400_000));
    assert_eq!(
        utc_days_at(&spans, 4 * 86_400_000, 36 * 3_600_000),
        vec![day(), day().succ_opt().unwrap()]
    );
}

/// Dropping the route's lag would hammer unpublished files; keeping the boundary would block recovery.
#[test]
fn publication_wait_ends_exactly_when_the_day_becomes_requestable() {
    let route = ArchiveRoute::BybitTradingDaily;
    let spans = Coverage::one((1000, 2000));
    let due = 216_000_000; // End of 1970-01-01 plus the reviewed 36-hour lag.
    assert_eq!(publication_pending(route, &spans, due - 1), Some(due));
    assert!(utc_days_at(&spans, due - 1, route.publish_lag_ms()).is_empty());
    assert_eq!(publication_pending(route, &spans, due), None);
    assert_eq!(
        utc_days_at(&spans, due, route.publish_lag_ms()),
        vec![day()]
    );
    let holed = Coverage::from_spans([(1000, 2000), (172_800_000, 172_801_000)]);
    assert_eq!(publication_pending(route, &holed, due), Some(388_800_000));
}

/// An already-due retry would let every report reload request the same late daily file again.
#[test]
fn late_daily_files_retry_at_the_next_utc_hour() {
    assert_eq!(next_retry_at(3_599_999), 3_600_000);
    assert_eq!(next_retry_at(3_600_000), 7_200_000);
}

/// Both compression envelopes must filter their CSV streams and remove downloaded bodies.
#[test]
fn zip_and_gzip_fill_only_requested_span_and_delete_files() {
    for route in [
        ArchiveRoute::BinanceUmDaily,
        ArchiveRoute::BybitTradingDaily,
    ] {
        let root = root();
        let spans = Coverage::one((1000, 1500));
        let fill = fill_with(
            route,
            "SYNUSDT",
            &spans,
            &ReplayGate::new(),
            (root.path(), vec![day()]),
            (|| Some(u64::MAX), u64::MAX),
            |_| Ok(response(body(route))),
        )
        .unwrap();
        assert_eq!(fill.covered, spans);
        assert_eq!(
            fill.ticks
                .iter()
                .map(|tick| (tick.time_ms, tick.qty))
                .collect::<Vec<_>>(),
            vec![(1000.0, 2.0)]
        );
        let paths = day_cache::Cleanup::new(
            day_cache::paths(root.path(), route, "SYNUSDT", day()).unwrap(),
        );
        assert!(!paths.file().exists());
    }
}

/// A reserve refusal must happen before a network call or partial-file creation.
#[test]
fn guard_refusal_precedes_transport() {
    let root = root();
    let result = fill_with(
        ArchiveRoute::BinanceUmDaily,
        "SYNUSDT",
        &Coverage::one((1000, 1500)),
        &ReplayGate::new(),
        (root.path(), vec![day()]),
        (|| None, u64::MAX),
        |_| panic!("transport before guard"),
    );
    assert!(matches!(result, Err(ArchiveError::NoRoom)));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

/// Crashed partial files must be removed before a new fill, including a refused download.
#[test]
fn startup_sweeps_leftover_part() {
    let root = root();
    let partial = root.path().join("abandoned.zip.9-1.part");
    std::fs::write(&partial, b"partial").unwrap();
    let _ = fill_with(
        ArchiveRoute::BinanceUmDaily,
        "SYNUSDT",
        &Coverage::one((1000, 1500)),
        &ReplayGate::new(),
        (root.path(), vec![day()]),
        (|| None, u64::MAX),
        |_| panic!("no transport"),
    );
    assert!(!partial.exists());
}

/// Sharing the host directory would let a spot download overwrite the futures file.
#[test]
fn spot_and_usdm_paths_are_distinct() {
    let root = root();
    let spot = day_cache::Cleanup::new(
        day_cache::paths(
            root.path(),
            ArchiveRoute::BinanceSpotDaily,
            "SYNUSDT",
            day(),
        )
        .unwrap(),
    );
    let futures = day_cache::Cleanup::new(
        day_cache::paths(root.path(), ArchiveRoute::BinanceUmDaily, "SYNUSDT", day()).unwrap(),
    );
    assert_ne!(spot.file(), futures.file());
    assert!(spot.file().to_string_lossy().contains("binance_spot_daily"));
    assert!(
        futures
            .file()
            .to_string_lossy()
            .contains("binance_um_daily")
    );
}

/// Keep the same-key lock through extraction, not just until its HTTP stream ends.
#[test]
fn same_key_fills_serialize() {
    let root = root();
    let gate = ReplayGate::new();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let active = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                let route = ArchiveRoute::BinanceUmDaily;
                fill_with(
                    route,
                    "SYNUSDT",
                    &Coverage::one((1000, 1500)),
                    &gate,
                    (root.path(), vec![day()]),
                    (|| Some(u64::MAX), u64::MAX),
                    |_| {
                        assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                        calls.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(response(body(route)))
                    },
                )
                .unwrap();
            });
        }
    });
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// Unknown-length downloads must stop at the disk reserve and remove their partial file.
#[test]
fn disk_guard_stops_stream_and_removes_partial_file() {
    let root = root();
    let received = Arc::new(AtomicU64::new(0));
    let result = fill_with(
        ArchiveRoute::BinanceUmDaily,
        "SYNUSDT",
        &Coverage::one((1000, 1500)),
        &ReplayGate::new(),
        (root.path(), vec![day()]),
        (|| Some(65_536 * EXTRACT_HEADROOM), u64::MAX),
        |_| {
            Ok(DayResponse {
                status: 200,
                retry_after_s: None,
                content_length: None,
                reader: Box::new(CountedRead {
                    reader: Box::new(std::io::repeat(0).take(131_072)),
                    bytes: received.clone(),
                }),
            })
        },
    );
    assert!(matches!(result, Err(ArchiveError::NoRoom)));
    assert_eq!(received.load(Ordering::Relaxed), 131_072);
    let paths = day_cache::Cleanup::new(
        day_cache::paths(root.path(), ArchiveRoute::BinanceUmDaily, "SYNUSDT", day()).unwrap(),
    );
    assert!(!paths.file().exists());
    assert_eq!(
        std::fs::read_dir(paths.file().parent().unwrap())
            .unwrap()
            .count(),
        0
    );
}

/// A failed later day must not discard the tape already paid for and extracted.
#[test]
fn later_failure_preserves_earlier_day() {
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let calls = std::cell::Cell::new(0);
    let fill = fill_with(
        route,
        "SYNUSDT",
        &Coverage::one((1000, 86_401_000)),
        &ReplayGate::new(),
        (root.path(), vec![day(), day().succ_opt().unwrap()]),
        (|| Some(u64::MAX), u64::MAX),
        |_| {
            let n = calls.get();
            calls.set(n + 1);
            if n == 0 {
                Ok(response(body(route)))
            } else {
                Err(ArchiveError::Transient("synthetic disconnect".into()))
            }
        },
    )
    .unwrap();
    assert_eq!(fill.covered, Coverage::one((1000, 86_399_999)));
    assert_eq!(fill.ticks.len(), 2);
    assert_eq!(
        fill.deferred,
        Some(ArchiveError::Transient("synthetic disconnect".into()))
    );
}

/// Server and transport failures must close the host gate for every other replay caller.
#[test]
fn server_and_transport_failures_refuse_host() {
    for transport in [false, true] {
        let root = root();
        let gate = ReplayGate::new();
        let route = ArchiveRoute::BinanceUmDaily;
        let result = fill_with(
            route,
            "SYNUSDT",
            &Coverage::one((1000, 1500)),
            &gate,
            (root.path(), vec![day()]),
            (|| Some(u64::MAX), u64::MAX),
            |_| {
                if transport {
                    Err(ArchiveError::Transient("synthetic timeout".into()))
                } else {
                    let mut response = response(Vec::new());
                    response.status = 503;
                    Ok(response)
                }
            },
        );
        if transport {
            assert!(matches!(result, Err(ArchiveError::Transient(_))));
        } else {
            assert!(matches!(result, Err(ArchiveError::Http(503))));
        }
        assert!(gate.claim(route.host(), Instant::now()).is_err());
    }
}

/// Damaged compression envelopes must be classified like malformed published CSV content.
#[test]
fn broken_zip_and_gzip_are_corrupt() {
    for route in [
        ArchiveRoute::BinanceUmDaily,
        ArchiveRoute::BybitTradingDaily,
    ] {
        let mut ticks = Vec::new();
        assert!(matches!(
            extract(
                route,
                Cursor::new(b"broken compressed file"),
                &Coverage::one((0, 2000)),
                &mut ticks
            ),
            Err(ArchiveError::Corrupt(_))
        ));
    }
}

/// A locked leftover must not prevent the next download or the removal of other partial files.
#[test]
fn sweep_survives_failed_removal() {
    let root = root();
    let locked = root.path().join("locked.part");
    let removable = root.path().join("removable.part");
    std::fs::write(&locked, b"partial").unwrap();
    std::fs::write(&removable, b"partial").unwrap();
    day_cache::sweep_with_remove(root.path(), &|path| {
        if path == locked {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        } else {
            std::fs::remove_file(path)
        }
    })
    .unwrap();
    assert!(locked.exists());
    assert!(!removable.exists());
}

/// Recent 404s remain retryable while a long-missing published day refuses the market.
#[test]
fn missing_day_uses_publication_age() {
    let route = ArchiveRoute::BinanceUmDaily;
    assert_eq!(
        missing_day(route, day(), 3 * 86_400_000),
        ArchiveError::NotPublished
    );
    assert_eq!(
        missing_day(route, day(), 11 * 86_400_000),
        ArchiveError::UnknownSymbol
    );
}

/// A throttle after a completed day must stop the walk while retaining its earlier tape.
#[test]
fn throttle_keeps_progress_and_stops_later_days() {
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let gate = ReplayGate::new();
    let calls = std::cell::Cell::new(0);
    let fill = fill_with(
        route,
        "SYNUSDT",
        &Coverage::one((1000, 3 * 86_400_000)),
        &gate,
        (
            root.path(),
            vec![
                day(),
                day().succ_opt().unwrap(),
                day().succ_opt().unwrap().succ_opt().unwrap(),
            ],
        ),
        (|| Some(u64::MAX), u64::MAX),
        |_| {
            let n = calls.get();
            calls.set(n + 1);
            if n == 0 {
                Ok(response(body(route)))
            } else {
                let mut response = response(Vec::new());
                response.status = 429;
                response.retry_after_s = Some(60);
                Ok(response)
            }
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(fill.covered, Coverage::one((1000, 86_399_999)));
    assert_eq!(
        fill.deferred,
        Some(ArchiveError::Throttled {
            retry_after_s: Some(60)
        })
    );
    assert!(gate.claim(route.host(), Instant::now()).is_err());
}

/// Missing files do not prevent later available dates from answering the same request.
#[test]
fn missing_day_is_skipped_before_later_success() {
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let calls = std::cell::Cell::new(0);
    let fill = fill_with(
        route,
        "SYNUSDT",
        &Coverage::one((0, 2 * 86_400_000 - 1)),
        &ReplayGate::new(),
        (root.path(), vec![day(), day().succ_opt().unwrap()]),
        (|| Some(u64::MAX), u64::MAX),
        |_| {
            let n = calls.get();
            calls.set(n + 1);
            if n == 0 {
                let mut response = response(Vec::new());
                response.status = 404;
                Ok(response)
            } else {
                Ok(response(body(route)))
            }
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(
        fill.covered,
        Coverage::one((86_400_000, 2 * 86_400_000 - 1))
    );
    assert_eq!(fill.deferred, Some(ArchiveError::NotPublished));
}

/// Keeping a skipped 404 as the deferred error would hide a later throttle from the pass.
#[test]
fn missing_then_throttled_preserves_stopping_error() {
    let root = root();
    let calls = std::cell::Cell::new(0);
    let result = fill_with(
        ArchiveRoute::BinanceUmDaily,
        "SYNUSDT",
        &Coverage::one((0, 2 * 86_400_000 - 1)),
        &ReplayGate::new(),
        (root.path(), vec![day(), day().succ_opt().unwrap()]),
        (|| Some(u64::MAX), u64::MAX),
        |_| {
            let mut answer = response(Vec::new());
            answer.status = if calls.get() == 0 { 404 } else { 429 };
            answer.retry_after_s = Some(60);
            calls.set(calls.get() + 1);
            Ok(answer)
        },
    );
    assert_eq!(
        result.unwrap_err(),
        ArchiveError::Throttled {
            retry_after_s: Some(60)
        }
    );
    assert_eq!(calls.get(), 2);
}

/// Only all requested daily files missing may classify the market as unknown.
#[test]
fn all_old_missing_days_refuse_symbol() {
    let root = root();
    let result = fill_with(
        ArchiveRoute::BinanceUmDaily,
        "SYNUSDT",
        &Coverage::one((0, 2 * 86_400_000 - 1)),
        &ReplayGate::new(),
        (root.path(), vec![day(), day().succ_opt().unwrap()]),
        (|| Some(u64::MAX), u64::MAX),
        |_| {
            let mut answer = response(Vec::new());
            answer.status = 404;
            Ok(answer)
        },
    );
    assert_eq!(result.unwrap_err(), ArchiveError::UnknownSymbol);
}

/// A corrupt published day must not prevent later daily files from answering the trade.
#[test]
fn corrupt_day_is_skipped_before_later_success() {
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let calls = std::cell::Cell::new(0);
    let fill = fill_with(
        route,
        "SYNUSDT",
        &Coverage::one((0, 2 * 86_400_000 - 1)),
        &ReplayGate::new(),
        (root.path(), vec![day(), day().succ_opt().unwrap()]),
        (|| Some(u64::MAX), u64::MAX),
        |_| {
            let n = calls.get();
            calls.set(n + 1);
            Ok(response(if n == 0 {
                b"broken zip".to_vec()
            } else {
                body(route)
            }))
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(
        fill.covered,
        Coverage::one((86_400_000, 2 * 86_400_000 - 1))
    );
    assert!(matches!(fill.deferred, Some(ArchiveError::Corrupt(_))));
}

/// Removing the old ceiling must allow a large day through download, extraction and coverage.
#[test]
fn day_above_old_ceiling_is_downloaded_and_filed() {
    use std::io::Read;
    let source = root();
    let path = source.path().join("synthetic.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("trades.csv", options).unwrap();
    zip.write_all(b"1,10,2,1,1,1000,true\n").unwrap();
    zip.start_file("padding", options).unwrap();
    std::io::copy(&mut std::io::repeat(0).take(256 * 1024 * 1024), &mut zip).unwrap();
    zip.finish().unwrap();
    let length = std::fs::metadata(&path).unwrap().len();
    assert!(length > 256 * 1024 * 1024);
    let root = root();
    let route = ArchiveRoute::BinanceUmDaily;
    let spans = Coverage::one((1000, 1500));
    let fill = fill_with(
        route,
        "SYNUSDT",
        &spans,
        &ReplayGate::new(),
        (root.path(), vec![day()]),
        (|| Some(u64::MAX), u64::MAX),
        |_| {
            Ok(DayResponse {
                status: 200,
                retry_after_s: None,
                content_length: Some(length),
                reader: Box::new(std::fs::File::open(&path).unwrap()),
            })
        },
    )
    .unwrap();
    assert_eq!(fill.covered, spans);
    assert_eq!(fill.ticks.len(), 1);
    assert_eq!(fill.ticks[0].time_ms, 1000.0);
    assert_eq!(fill.deferred, None);
}

/// Admission must use response headers before any body bytes spend an insufficient pass budget.
#[test]
fn body_budget_admission_uses_known_lengths() {
    /// Synthetic body proving admission happens before a transport read.
    struct Unread;
    impl Read for Unread {
        /// A deferred body must never reach transport reads.
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("budget-deferred body read")
        }
    }
    {
        let root = root();
        let result = fill_with(
            ArchiveRoute::BinanceUmDaily,
            "SYNUSDT",
            &Coverage::one((1000, 1500)),
            &ReplayGate::new(),
            (root.path(), vec![day()]),
            (|| Some(u64::MAX), 1023),
            |_| {
                Ok(DayResponse {
                    status: 200,
                    retry_after_s: None,
                    content_length: Some(1024),
                    reader: Box::new(Unread),
                })
            },
        );
        assert_eq!(result.unwrap_err(), ArchiveError::BudgetDeferred);
    }
    let route = ArchiveRoute::BinanceUmDaily;
    let bytes = body(route);
    let root = root();
    assert!(
        fill_with(
            route,
            "SYNUSDT",
            &Coverage::one((1000, 1500)),
            &ReplayGate::new(),
            (root.path(), vec![day()]),
            (|| Some(u64::MAX), bytes.len() as u64),
            |_| Ok(response(bytes.clone()))
        )
        .is_ok()
    );
}

/// Unknown-length responses use one reserve snapshot instead of guessing a day size.
#[test]
fn unknown_length_snapshots_disk_budget() {
    let root = root();
    let checks = std::cell::RefCell::new(Vec::new());
    let route = ArchiveRoute::BinanceUmDaily;
    fill_with(
        route,
        "SYNUSDT",
        &Coverage::one((1000, 1500)),
        &ReplayGate::new(),
        (root.path(), vec![day()]),
        (
            || {
                checks.borrow_mut().push(());
                Some(u64::MAX)
            },
            u64::MAX,
        ),
        |_| {
            let mut answer = response(body(route));
            answer.content_length = None;
            Ok(answer)
        },
    )
    .unwrap();
    assert_eq!(checks.borrow().len(), 1);
}
