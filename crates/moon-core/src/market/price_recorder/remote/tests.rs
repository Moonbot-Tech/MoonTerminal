use super::super::ring::{Point, Segment};
use super::super::store::{Filed, Store, open_read, read_conn};
use super::*;

const EX: &str = "ex:00000000";

fn filed(market: &str, from_ms: i64, to_ms: i64, bids: &[(i64, f64)]) -> Filed {
    Filed {
        exchange: EX.to_owned(),
        market: market.to_owned(),
        segment: Segment {
            from_ms,
            to_ms,
            points: bids
                .iter()
                .map(|&(t_ms, bid)| Point {
                    t_ms,
                    bid,
                    ask: bid + 1.0,
                    mark: bid + 0.5,
                })
                .collect(),
            funding: Vec::new(),
        },
    }
}

/// A station file: two abutting segments and a gap on X, one old segment on Y.
fn station() -> Store {
    let mut store = Store::in_memory().unwrap();
    store
        .write(&[
            filed("XUSDT", 1_000, 1_999, &[(1_000, 1.0), (1_500, 1.1)]),
            filed("XUSDT", 2_000, 2_999, &[(2_000, 1.1), (2_500, 1.2)]),
            filed("XUSDT", 5_000, 5_999, &[(5_000, 1.3)]),
            filed("YUSDT", 100, 199, &[(100, 7.0)]),
        ])
        .unwrap();
    store
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "moon-price-pull-{name}-{}-{}",
        std::process::id(),
        crate::util::time::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("market_prices.sqlite")
}

#[test]
fn the_index_lists_stretches_since_and_pages_without_losing_any() {
    let store = station();
    let whole = index_conn(store.conn(), 500, None, usize::MAX).unwrap();
    assert_eq!(whole.resume, None);
    assert_eq!(
        whole.markets,
        vec![PriceHeld {
            exchange: EX.into(),
            market: "XUSDT".into(),
            spans: vec![(1_000, 2_999), (5_000, 5_999)],
        }],
        "abutting segments are one stretch, a gap splits, Y ended before `since`"
    );
    // A budget of one segment per page: every page gets further, and together they say the same.
    let mut got = Vec::new();
    let mut after = None;
    for _ in 0..10 {
        let page = index_conn(store.conn(), 500, after.as_ref(), 1).unwrap();
        assert!(!page.markets.is_empty());
        got.extend(page.markets);
        match page.resume {
            Some(key) => after = Some(key),
            None => break,
        }
    }
    let spans: Vec<_> = got.iter().flat_map(|m| m.spans.iter().copied()).collect();
    assert_eq!(
        Coverage::from_spans(spans).spans(),
        &[(1_000, 2_999), (5_000, 5_999)]
    );
}

#[test]
fn a_fetch_cuts_segments_to_the_stretch_and_resumes_past_its_budget() {
    let store = station();
    let want = TapeWant {
        exchange: EX.into(),
        market: "XUSDT".into(),
        spans: vec![(1_700, 2_200), (5_500, 9_000)],
    };
    let all = fetch_conn(store.conn(), std::slice::from_ref(&want), usize::MAX).unwrap();
    assert_eq!(all.resume, None);
    let bounds: Vec<_> = all.pieces.iter().map(|p| (p.from_ms, p.to_ms)).collect();
    assert_eq!(bounds, vec![(1_700, 1_999), (2_000, 2_200), (5_500, 5_999)]);
    // Room for one piece: the answer says where the next one starts.
    let first = fetch_conn(
        store.conn(),
        std::slice::from_ref(&want),
        all.pieces[0].data.len() + PIECE_OVERHEAD,
    )
    .unwrap();
    assert_eq!(first.pieces.len(), 1);
    assert_eq!(
        first.resume,
        Some(TapeResume {
            item: 0,
            from_ms: 2_000
        })
    );
}

#[test]
fn what_the_terminal_files_reads_back_as_the_station_recorded_it() {
    let store = station();
    let want = TapeWant {
        exchange: EX.into(),
        market: "XUSDT".into(),
        spans: vec![(1_700, 2_700)],
    };
    let answer = fetch_conn(store.conn(), std::slice::from_ref(&want), usize::MAX).unwrap();
    let path = temp_path("roundtrip");
    let mut importer = Importer::open_at(&path).unwrap();
    for piece in &answer.pieces {
        importer.add(&want, piece).unwrap();
    }
    importer.finish().unwrap();
    assert_eq!((importer.filed, importer.refused), (2, 0));
    let conn = open_read(&path).unwrap().unwrap();
    let spans = read_conn(&conn, EX, "XUSDT", 0, 10_000).unwrap();
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
    // 1 700 holds what the station held since 1 500; one unbroken stretch across the seam.
    assert_eq!(
        shape,
        vec![(1_700, 2_700, vec![(1_700, 1.1), (2_500, 1.2)])]
    );
}

#[test]
fn the_terminal_never_replaces_its_own_segment_and_refuses_a_broken_piece() {
    let path = temp_path("own");
    {
        let mut own = Store::open(&path).unwrap();
        own.write(&[filed("XUSDT", 1_000, 1_999, &[(1_000, 42.0)])])
            .unwrap();
    }
    let station = station();
    let want = TapeWant {
        exchange: EX.into(),
        market: "XUSDT".into(),
        spans: vec![(1_000, 1_999)],
    };
    let answer = fetch_conn(station.conn(), std::slice::from_ref(&want), usize::MAX).unwrap();
    let mut importer = Importer::open_at(&path).unwrap();
    importer.add(&want, &answer.pieces[0]).unwrap();
    let broken = PricePiece {
        data: "not base64!".into(),
        ..answer.pieces[0].clone()
    };
    importer.add(&want, &broken).unwrap();
    // A piece that overlaps the file's own segment without sharing its start is left out too.
    let shifted = PricePiece {
        from_ms: 900,
        ..answer.pieces[0].clone()
    };
    importer.add(&want, &shifted).unwrap();
    importer.finish().unwrap();
    assert_eq!(
        (importer.filed, importer.refused, importer.overlapped),
        (0, 1, 2)
    );
    let conn = open_read(&path).unwrap().unwrap();
    let spans = read_conn(&conn, EX, "XUSDT", 0, 10_000).unwrap();
    assert_eq!(spans[0].samples[0].bid, 42.0, "the terminal's own stays");
}

#[test]
fn lacking_is_what_the_station_holds_minus_what_is_here_before_the_edge() {
    let now = 1_000_000_000;
    let edge = now - PULL_EDGE_MS;
    let held = vec![
        PriceHeld {
            exchange: EX.into(),
            market: "XUSDT".into(),
            spans: vec![(1_000, 4_999)],
        },
        // Same market on the next page, and one reaching past the edge.
        PriceHeld {
            exchange: EX.into(),
            market: "XUSDT".into(),
            spans: vec![(5_000, 5_999), (edge - 100, edge + 5_000)],
        },
        // Wholly held here: not asked.
        PriceHeld {
            exchange: EX.into(),
            market: "YUSDT".into(),
            spans: vec![(100, 199)],
        },
    ];
    let wants = lacking_with(held, now, |_, market, _, _| {
        Ok(match market {
            "XUSDT" => vec![(2_000, 2_999)],
            _ => vec![(0, 500)],
        })
    })
    .unwrap();
    assert_eq!(
        wants,
        vec![TapeWant {
            exchange: EX.into(),
            market: "XUSDT".into(),
            spans: vec![(1_000, 1_999), (3_000, 5_999), (edge - 100, edge)],
        }]
    );
}
