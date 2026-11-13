use super::super::ring::{Point, Segment};
use super::*;

/// Station and terminal primaries may differ; imports must not duplicate local group coverage.
#[test]
fn imports_and_local_bounds_deduplicate_across_group_assignments() {
    let mut store = Store::in_memory().unwrap();
    let mut local = segment(1_000, 1_999, &[(1_000, 2.0)]);
    local.exchange.push_str("|USDC");
    store.write(&[local]).unwrap();
    let remote = segment(1_000, 1_999, &[(1_000, 3.0)]);
    let packed = Packed {
        exchange: remote.exchange,
        market: remote.market,
        from_ms: remote.segment.from_ms,
        to_ms: remote.segment.to_ms,
        data: codec::encode(&remote.segment),
    };
    assert_eq!(store.insert_packed(&[packed]).unwrap(), 0);
    assert_eq!(
        market_bounds(store.conn(), "ex:00000000", "XUSDT", 0, 3_000).unwrap(),
        vec![(1_000, 1_999)]
    );
}

/// Coarse replay reads must find secondary groups offline and retain nonoverlapping legacy data.
#[test]
fn grouped_prices_override_legacy_only_where_the_group_recorded() {
    let mut store = Store::in_memory().unwrap();
    let legacy = segment(0, 9_999, &[(0, 1.0), (7_000, 3.0)]);
    let mut grouped = segment(2_000, 5_999, &[(2_000, 2.0)]);
    grouped.exchange.push_str("|USDC");
    store.write(&[legacy, grouped]).unwrap();
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 0, 10_000).unwrap();
    assert_eq!(
        spans
            .iter()
            .map(|s| (s.from_ms, s.to_ms))
            .collect::<Vec<_>>(),
        vec![(0, 1_999), (2_000, 5_999), (6_000, 9_999)]
    );
    assert_eq!(spans[1].samples[0].bid, 2.0);
    assert_eq!(
        spans[2]
            .samples
            .iter()
            .map(|s| (s.t_ms, s.bid))
            .collect::<Vec<_>>(),
        vec![(6_000, 1.0), (7_000, 3.0)]
    );
    // Station requests identify a physical key and must not fetch another group's legacy rows.
    assert_eq!(
        read_conn(store.conn(), "ex:00000000|USDC", "XUSDT", 0, 10_000)
            .unwrap()
            .len(),
        1
    );
    // A tape keep map still uses the coarse key and must purge the secondary group's gap too.
    store
        .keep_only(
            "ex:00000000",
            "XUSDT",
            &Coverage::from_spans([(0, 999), (8_000, 9_999)]),
            10_000,
        )
        .unwrap();
    assert!(
        held_bounds(store.conn(), "ex:00000000|USDC", "XUSDT", 0, 10_000)
            .unwrap()
            .is_empty()
    );
}

fn segment(from_ms: i64, to_ms: i64, bids: &[(i64, f64)]) -> Filed {
    Filed {
        exchange: "ex:00000000".to_owned(),
        market: "XUSDT".to_owned(),
        segment: Segment {
            from_ms,
            to_ms,
            points: bids
                .iter()
                .map(|&(t_ms, bid)| Point {
                    t_ms,
                    bid,
                    ask: 2.0,
                    mark: 1.5,
                })
                .collect(),
            funding: Vec::new(),
        },
    }
}

#[test]
fn abutting_segments_read_as_one_stretch_and_a_gap_splits_it() {
    let mut store = Store::in_memory().unwrap();
    store
        .write(&[
            segment(1_000, 1_999, &[(1_000, 1.0), (1_500, 1.1)]),
            // Opens with the state it starts from, which the reader does not repeat.
            segment(2_000, 2_999, &[(2_000, 1.1), (2_500, 1.2)]),
            segment(5_000, 5_999, &[(5_000, 1.3)]),
        ])
        .unwrap();
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 0, 10_000).unwrap();
    let shape: Vec<_> = spans
        .iter()
        .map(|s| {
            (
                s.from_ms,
                s.to_ms,
                s.samples
                    .iter()
                    .map(|p| (p.t_ms, p.bid))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            (1_000, 2_999, vec![(1_000, 1.0), (1_500, 1.1), (2_500, 1.2)]),
            (5_000, 5_999, vec![(5_000, 1.3)]),
        ]
    );
    // Bounds pick whole segments that touch them, and another market reads nothing.
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 2_500, 2_600).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].from_ms, spans[0].to_ms), (2_000, 2_999));
    assert!(
        read_conn(store.conn(), "ex:00000000", "YUSDT", 0, 10_000)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_unreadable_segment_breaks_the_stretch() {
    let mut store = Store::in_memory().unwrap();
    store
        .write(&[
            segment(1_000, 1_999, &[(1_000, 1.0)]),
            segment(2_000, 2_999, &[(2_000, 1.1)]),
            segment(3_000, 3_999, &[(3_000, 1.2)]),
        ])
        .unwrap();
    store
        .conn()
        .execute("UPDATE segments SET data = x'63' WHERE from_ms = 2000", [])
        .unwrap();
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 0, 10_000).unwrap();
    let bounds: Vec<_> = spans.iter().map(|s| (s.from_ms, s.to_ms)).collect();
    assert_eq!(bounds, vec![(1_000, 1_999), (3_000, 3_999)]);
}

#[test]
fn retention_drops_what_is_past_the_edge() {
    let mut store = Store::in_memory().unwrap();
    store
        .write(&[
            segment(1_000, 1_999, &[(1_000, 1.0)]),
            segment(5_000, 5_999, &[(5_000, 1.0)]),
        ])
        .unwrap();
    assert_eq!(store.trim(Some(2_000), u64::MAX).unwrap(), 1);
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 0, 10_000).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].from_ms, 5_000);
}

#[test]
fn retention_drops_the_oldest_past_the_ceiling() {
    let mut store = Store::in_memory().unwrap();
    // Twenty markets filed side by side, as open trades are: the oldest rows of the file are
    // spread over every market.
    let markets: Vec<String> = (0..20).map(|m| format!("M{m}USDT")).collect();
    for i in 0..200 {
        let from = i * 1_000;
        let bids: Vec<_> = (0..20).map(|k| (from + k * 10, k as f64)).collect();
        let batch: Vec<_> = markets
            .iter()
            .map(|market| Filed {
                market: market.clone(),
                ..segment(from, from + 999, &bids)
            })
            .collect();
        store.write(&batch).unwrap();
    }
    let total = 200 * markets.len();
    let ceiling = store.used_bytes().unwrap() / 2;
    let deleted = store.trim(None, ceiling).unwrap();
    // Free pages stay in the file; what is in use fits the ceiling.
    assert!(store.used_bytes().unwrap() <= ceiling);
    // About half goes, not nearly everything.
    assert!(
        deleted > total / 3 && deleted < total * 2 / 3,
        "{deleted} of {total} rows deleted"
    );
    for market in &markets {
        let spans = read_conn(store.conn(), "ex:00000000", market, 0, i64::MAX).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].to_ms, 199_999, "the newest segment stays");
        assert!(spans[0].from_ms > 0, "the oldest went first");
    }
}

#[test]
fn a_missing_file_reads_as_nothing() {
    let path = std::env::temp_dir().join(format!(
        "moon-price-recorder-missing-{}.sqlite",
        std::process::id()
    ));
    assert!(
        read_at(&path, "ex:00000000", "XUSDT", 0, 1)
            .unwrap()
            .is_empty()
    );
    assert!(!path.exists(), "a read never creates the file");
}

#[test]
fn a_size_trim_takes_old_segments_first_even_when_they_were_filed_last() {
    let mut store = Store::in_memory().unwrap();
    // This host's own recording, recent.
    let own: Vec<_> = (0..200)
        .map(|i| {
            segment(
                1_000_000 + i * 1_000,
                1_000_999 + i * 1_000,
                &[(1_000_000 + i * 1_000, 1.0)],
            )
        })
        .collect();
    store.write(&own).unwrap();
    // A station's old segments of another market, filed after it.
    let old: Vec<_> = (0..200)
        .map(|i| {
            let from = i * 1_000;
            Packed {
                exchange: "ex:00000000".to_owned(),
                market: "OLDUSDT".to_owned(),
                from_ms: from,
                to_ms: from + 999,
                data: codec::encode(&segment(from, from + 999, &[(from, 2.0)]).segment),
            }
        })
        .collect();
    assert_eq!(store.insert_packed(&old).unwrap(), 200);
    let ends = |store: &Store| -> Vec<i64> {
        let mut stmt = store.conn().prepare("SELECT to_ms FROM segments").unwrap();
        let ends = stmt.query_map([], |r| r.get(0)).unwrap();
        ends.collect::<rusqlite::Result<_>>().unwrap()
    };
    let before = ends(&store);
    let ceiling = store.used_bytes().unwrap() / 2;
    store.trim(None, ceiling).unwrap();
    let after = ends(&store);
    assert!(after.len() < before.len());
    // Every segment left ends after every one deleted: the pulled old ones went first, though
    // they were filed last.
    let deleted_last = before.iter().filter(|e| !after.contains(e)).max().unwrap();
    assert!(after.iter().all(|e| e > deleted_last));
    assert!(
        after.contains(&1_199_999),
        "the newest of this host's own stays"
    );
}

#[test]
fn the_tapes_keep_map_drops_and_cuts_segments_outside_it() {
    let mut store = Store::in_memory().unwrap();
    store
        .write(&[
            segment(1_000, 1_999, &[(1_000, 1.0), (1_200, 1.1)]),
            segment(2_000, 2_999, &[(2_000, 1.1), (2_800, 1.2)]),
            segment(3_000, 3_999, &[(3_000, 1.3)]),
            segment(5_000, 5_999, &[(5_000, 1.4)]),
        ])
        .unwrap();
    // An emulator trade's ground, 1 500..2 500 and 3 000..3 999, is not kept.
    let keep = Coverage::from_spans([(0, 1_499), (2_501, 2_999), (4_000, i64::MAX)]);
    assert_eq!(
        store
            .keep_only("ex:00000000", "XUSDT", &keep, 10_000)
            .unwrap(),
        3
    );
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 0, 10_000).unwrap();
    let shape: Vec<_> = spans
        .iter()
        .map(|s| {
            (
                s.from_ms,
                s.to_ms,
                s.samples
                    .iter()
                    .map(|p| (p.t_ms, p.bid))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            (1_000, 1_499, vec![(1_000, 1.0), (1_200, 1.1)]),
            // The cut opens with the state in force at its start.
            (2_501, 2_999, vec![(2_501, 1.1), (2_800, 1.2)]),
            (5_000, 5_999, vec![(5_000, 1.4)]),
        ]
    );
}
