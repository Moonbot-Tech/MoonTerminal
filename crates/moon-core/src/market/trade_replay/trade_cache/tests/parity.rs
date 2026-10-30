use super::*;

/// Batched commits must produce the old per-insert database exactly and must not let a queued
/// barrier jump ahead of the inserts that preceded it.
#[test]
fn insert_batch_matches_independent_commits_and_preserves_order() {
    let old = conn();
    let new = conn();
    file_legacy(&old, "XALPHA", 50, 59, &[tick(55, Side::Buy)], 1);
    file_legacy(&new, "XALPHA", 50, 59, &[tick(55, Side::Buy)], 1);
    let inserts = || {
        vec![
            Op::Insert {
                exchange: "x".into(),
                market: "XALPHA".into(),
                from_ms: 0,
                to_ms: 99,
                ticks: vec![
                    tick(5, Side::Buy),
                    tick(55, Side::Buy),
                    tick(95, Side::Sell),
                ],
                source: TileSource::Core,
            },
            Op::Insert {
                exchange: "x".into(),
                market: "XALPHA".into(),
                from_ms: 100,
                to_ms: 199,
                ticks: vec![tick(105, Side::Sell)],
                source: TileSource::Venue,
            },
            Op::Insert {
                exchange: "x".into(),
                market: "XALPHA".into(),
                from_ms: 150,
                to_ms: 249,
                ticks: vec![tick(155, Side::Buy), tick(205, Side::Buy)],
                source: TileSource::Core,
            },
        ]
    };
    let mut old_held = held_bytes(&old).unwrap();
    for op in inserts() {
        serve(&old, op, &mut old_held, || None);
    }

    let (reply, barrier) = mpsc::channel();
    let (tx, rx) = mpsc::sync_channel(256);
    let mut queued = inserts();
    queued.insert(
        2,
        Op::Sync {
            reply,
            reservation: None,
        },
    );
    for op in queued {
        tx.send(op).unwrap();
    }
    let mut deferred = std::collections::VecDeque::new();
    let first = batch_inserts(rx.recv().unwrap(), &rx, &mut deferred);
    assert!(matches!(first, Op::InsertBatch(ref ops) if ops.len() == 2));
    let mut new_held = held_bytes(&new).unwrap();
    serve(&new, first, &mut new_held, || None);
    let barrier_op = deferred.pop_front().expect("queued barrier");
    serve(&new, barrier_op, &mut new_held, || None);
    barrier
        .try_recv()
        .expect("preceding inserts reached their barrier");
    let last = batch_inserts(rx.recv().unwrap(), &rx, &mut deferred);
    serve(&new, last, &mut new_held, || None);

    assert_eq!(
        read_span_bounds(&new, "x", "XALPHA", 0, 300).unwrap(),
        read_span_bounds(&old, "x", "XALPHA", 0, 300).unwrap()
    );
    assert_eq!(
        times(&read_spans(&new, "x", "XALPHA", 0, 300).unwrap()),
        times(&read_spans(&old, "x", "XALPHA", 0, 300).unwrap())
    );
    assert_eq!(new_held, held_bytes(&new).unwrap());
}

/// Batched eviction must match independent commits under several byte ceilings, rather than
/// deleting coverage at batch end that a later overlapping insert expected to remain.
#[test]
fn insert_batch_preserves_per_insert_eviction_order() {
    for ceiling in [Some(0), Some(4), Some(8), Some(16)] {
        let old = conn();
        let new = conn();
        let ops = || {
            vec![
                Op::Insert {
                    exchange: "x".into(),
                    market: "XALPHA".into(),
                    from_ms: 0,
                    to_ms: 19,
                    ticks: vec![tick(5, Side::Buy), tick(15, Side::Sell)],
                    source: TileSource::Core,
                },
                Op::Insert {
                    exchange: "x".into(),
                    market: "XALPHA".into(),
                    from_ms: 10,
                    to_ms: 29,
                    ticks: vec![tick(15, Side::Sell), tick(25, Side::Buy)],
                    source: TileSource::Venue,
                },
                Op::Insert {
                    exchange: "x".into(),
                    market: "XALPHA".into(),
                    from_ms: 30,
                    to_ms: 39,
                    ticks: vec![tick(35, Side::Buy)],
                    source: TileSource::Core,
                },
            ]
        };
        let mut old_held = 0;
        for op in ops() {
            serve(&old, op, &mut old_held, || ceiling);
        }
        let mut new_held = 0;
        serve(&new, Op::InsertBatch(ops()), &mut new_held, || ceiling);
        assert_eq!(
            read_span_bounds(&new, "x", "XALPHA", 0, 100).unwrap(),
            read_span_bounds(&old, "x", "XALPHA", 0, 100).unwrap(),
            "ceiling {ceiling:?}"
        );
        assert_eq!(new_held, old_held, "ceiling {ceiling:?}");
    }
}
