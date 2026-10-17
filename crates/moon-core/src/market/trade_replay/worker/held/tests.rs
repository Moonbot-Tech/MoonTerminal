//! Disk hydration regressions without a process-wide cache or timing-dependent worker.

use super::*;
use crate::feed::types::Side;

/// The old full-focus hydrate and the gap hydrate preserve identical covered runs and ticks.
#[test]
fn gap_hydration_matches_full_read_answers() {
    let key = ("VENUE_A".into(), "XZETA".into());
    let archive = [span(0, 19), span(30, 90_000)];
    let old = Mutex::new(TickTileStore::default());
    let new = Mutex::new(TickTileStore::default());
    for focus in [
        Coverage::one((5, 10)),
        Coverage::one((0, 60_000)),
        Coverage::from_spans([(0, 9), (60_000, 90_000)]),
    ] {
        // Previous production behavior reads every intersecting row, then inserts whole rows.
        for &(from, to) in focus.spans() {
            for row in archive
                .iter()
                .filter(|row| row.to_ms >= from && row.from_ms <= to)
            {
                lock_tiles(&old).insert(
                    key.clone(),
                    row.from_ms,
                    row.to_ms,
                    row.ticks.clone(),
                    row.source,
                );
            }
        }
        hydrate_with(
            &new,
            &key,
            &focus,
            || 1,
            |from, to| {
                Some(
                    archive
                        .iter()
                        .filter(|row| row.to_ms >= from && row.from_ms <= to)
                        .cloned()
                        .collect(),
                )
            },
        );
        let previous = lock_tiles(&old);
        let current = lock_tiles(&new);
        let covered = held_coverage(&previous, &key, &focus, Coverage::none());
        assert_eq!(
            held_coverage(&current, &key, &focus, Coverage::none()),
            covered
        );
        assert_eq!(
            format!("{:?}", read_coverage(&previous, &key, &covered)),
            format!("{:?}", read_coverage(&current, &key, &covered))
        );
    }
}

/// A synthetic archive row with a print on each inclusive edge.
fn span(from_ms: i64, to_ms: i64) -> trade_cache::StoredSpan {
    trade_cache::StoredSpan {
        from_ms,
        to_ms,
        ticks: [from_ms, to_ms]
            .into_iter()
            .map(|time| Tick {
                time_ms: time as f64,
                price: 10.0,
                qty: 2.0,
                side: Side::Buy,
            })
            .collect(),
        source: TileSource::Station,
    }
}

/// Repeat queries keep every print, including the tail, without reading disk again.
#[test]
fn repeat_hydration_reads_only_new_gaps() {
    let tiles = Mutex::new(TickTileStore::default());
    let key = ("VENUE_A".into(), "XALPHA".into());
    let calls = RefCell::new(Vec::new());
    let read = |from, to| {
        calls.borrow_mut().push((from, to));
        Some(vec![span(from, to)])
    };
    hydrate_with(&tiles, &key, &Coverage::one((0, 60_000)), || 1, read);
    hydrate_with(&tiles, &key, &Coverage::one((0, 60_000)), || 1, read);
    assert_eq!(*calls.borrow(), vec![(0, 60_000)]);
    hydrate_with(&tiles, &key, &Coverage::one((30_000, 90_000)), || 1, read);
    assert_eq!(*calls.borrow(), vec![(0, 60_000), (60_001, 90_000)]);
    let store = lock_tiles(&tiles);
    assert_eq!(
        store
            .read(&key, 0, 90_000)
            .iter()
            .map(|t| t.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![0, 60_000, 60_001, 90_000]
    );
    assert!(store.covers(&key, 0, 90_000));
}

/// A filing invalidates both ends, even when the first query refreshes only one of them.
#[test]
fn refiling_rechecks_each_stale_span_without_duplicating_prints() {
    let tiles = Mutex::new(TickTileStore::default());
    let key = ("VENUE_A".into(), "XBETA".into());
    let focus = Coverage::from_spans([(0, 9), (20, 29)]);
    let calls = RefCell::new(Vec::new());
    let read = |from, to| {
        calls.borrow_mut().push((from, to));
        Some(vec![span(from, to)])
    };
    hydrate_with(&tiles, &key, &focus, || 1, read);
    hydrate_with(&tiles, &key, &Coverage::one((0, 9)), || 2, read);
    hydrate_with(&tiles, &key, &focus, || 2, read);
    hydrate_with(&tiles, &key, &focus, || 2, read);
    assert_eq!(*calls.borrow(), vec![(0, 9), (20, 29), (0, 9), (20, 29)]);
    let store = lock_tiles(&tiles);
    assert_eq!(held_coverage(&store, &key, &focus, Coverage::none()), focus);
    assert_eq!(store.read(&key, 0, 29).len(), 4);
}

/// Failed initial and invalidated reads never acknowledge disk freshness or invent coverage.
#[test]
fn failed_hydration_remains_retryable() {
    let tiles = Mutex::new(TickTileStore::default());
    let key = ("VENUE_A".into(), "XGAMMA".into());
    let focus = Coverage::one((0, 9));
    assert!(
        !hydrate_with(&tiles, &key, &focus, || 1, |_, _| None),
        "a failed read is no settled miss"
    );
    assert!(!lock_tiles(&tiles).covers(&key, 0, 9));
    assert!(hydrate_with(
        &tiles,
        &key,
        &focus,
        || 1,
        |from, to| Some(vec![span(from, to)]),
    ));
    assert!(
        hydrate_with(&tiles, &key, &focus, || 1, |_, _| None),
        "nothing left to read settles the answer"
    );
    assert!(!hydrate_with(&tiles, &key, &focus, || 2, |_, _| None));
    let calls = Cell::new(0);
    hydrate_with(
        &tiles,
        &key,
        &focus,
        || 2,
        |from, to| {
            calls.set(calls.get() + 1);
            Some(vec![span(from, to)])
        },
    );
    assert_eq!(calls.get(), 1);
}

/// A filing during I/O is newer than that read's snapshot and still invalidates the next ask.
#[test]
fn filing_during_hydration_is_not_acknowledged_early() {
    let tiles = Mutex::new(TickTileStore::default());
    let key = ("VENUE_A".into(), "XETA".into());
    let focus = Coverage::one((0, 9));
    let revision = Cell::new(1);
    hydrate_with(
        &tiles,
        &key,
        &focus,
        || revision.get(),
        |from, to| {
            revision.set(2);
            Some(vec![span(from, to)])
        },
    );
    let calls = Cell::new(0);
    hydrate_with(
        &tiles,
        &key,
        &focus,
        || revision.get(),
        |from, to| {
            calls.set(calls.get() + 1);
            Some(vec![span(from, to)])
        },
    );
    assert_eq!(calls.get(), 1);
}

/// A trim during disk I/O cannot resurrect the deleted tape.
#[test]
fn trim_discards_inflight_hydration() {
    let tiles = Mutex::new(TickTileStore::default());
    let key = ("VENUE_A".into(), "XDELTA".into());
    hydrate_with(
        &tiles,
        &key,
        &Coverage::one((0, 9)),
        || 1,
        |from, to| {
            lock_tiles(&tiles).forget();
            Some(vec![span(from, to)])
        },
    );
    assert!(lock_tiles(&tiles).read(&key, 0, 9).is_empty());
}

/// Tiles evicted for space must be fetched again despite their earlier successful hydration.
#[test]
fn evicted_tape_is_read_again() {
    let tiles = Mutex::new(TickTileStore::default());
    let key = ("VENUE_A".into(), "XEPSILON".into());
    let focus = Coverage::one((0, 9));
    hydrate_with(
        &tiles,
        &key,
        &focus,
        || 1,
        |from, to| Some(vec![span(from, to)]),
    );
    for index in 0..super::super::super::tick_tiles::TILE_STORE_MAX_TILES {
        lock_tiles(&tiles).insert(
            ("VENUE_B".into(), format!("X{index}")),
            0,
            9,
            vec![],
            TileSource::Core,
        );
    }
    assert!(!lock_tiles(&tiles).covers(&key, 0, 9));
    let calls = Cell::new(0);
    hydrate_with(
        &tiles,
        &key,
        &focus,
        || 1,
        |from, to| {
            calls.set(calls.get() + 1);
            Some(vec![span(from, to)])
        },
    );
    assert_eq!(calls.get(), 1);
    assert!(lock_tiles(&tiles).covers(&key, 0, 9));
}

/// Another reader evicting this query's planned-held tiles mid-read cannot shrink its answer:
/// the answer equals a serial read of the same tiles and disk.
#[test]
fn concurrent_eviction_keeps_the_answer_whole() {
    let tiles = Mutex::new(TickTileStore::default());
    let key: TileKey = ("VENUE_A".into(), "XTHETA".into());
    let disk = |from: i64, to: i64| Some(vec![span(from, to)]);
    hydrate_with(&tiles, &key, &Coverage::one((0, 30_000)), || 1, disk);
    let focus = Coverage::one((0, 60_000));
    let serial = answer_with(&tiles, &key, &focus, || 1, disk);
    let tiles = Mutex::new(TickTileStore::default());
    hydrate_with(&tiles, &key, &Coverage::one((0, 30_000)), || 1, disk);
    let raced = answer_with(
        &tiles,
        &key,
        &focus,
        || 1,
        |from, to| {
            // A second held reader fills the store past its tile ceiling between plan and answer.
            for index in 0..super::super::super::tick_tiles::TILE_STORE_MAX_TILES {
                lock_tiles(&tiles).insert(
                    ("VENUE_B".into(), format!("X{index}")),
                    0,
                    9,
                    vec![],
                    TileSource::Core,
                );
            }
            disk(from, to)
        },
    );
    assert_eq!(serial.covered, Coverage::one((0, 60_000)));
    assert_eq!(raced.covered, serial.covered);
    assert_eq!(format!("{:?}", raced.ticks), format!("{:?}", serial.ticks));
    assert!(raced.settled);
}
