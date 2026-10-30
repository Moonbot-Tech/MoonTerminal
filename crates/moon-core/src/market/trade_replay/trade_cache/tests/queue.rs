use super::*;

/// Build one empty synthetic insert for queue-admission tests.
fn insert_op() -> Op {
    Op::Insert {
        exchange: "CORE_A".into(),
        market: "XALPHA".into(),
        from_ms: 0,
        to_ms: 1,
        ticks: Vec::new(),
        source: TileSource::Core,
    }
}

/// A full bounded queue must refuse a read at once, while a background insert waits for the
/// receiver and arrives whole instead of being dropped or moved to an unbounded fallback.
#[test]
fn full_cache_queue_defers_read_and_backpressures_insert() {
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(insert_op()).unwrap();
    let cache = TradeCache { tx };
    assert!(cache.read("CORE_A", "XALPHA", 0, 1).is_none());
    let (started, writing) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        started.send(()).unwrap();
        cache.insert("CORE_A", "XALPHA", 2, 3, Vec::new(), TileSource::Core);
    });
    writing.recv().unwrap();
    assert!(matches!(rx.recv().unwrap(), Op::Insert { .. }));
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Op::Insert { .. }
    ));
    writer.join().unwrap();
}

/// Shutdown sync must wait for queue space and the durable barrier instead of reporting success
/// while an earlier insert still occupies the full queue.
#[test]
fn full_cache_sync_waits_for_space_and_barrier() {
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(insert_op()).unwrap();
    let cache = TradeCache { tx };
    let (started, waiting) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        cache.sync(Duration::from_secs(1))
    });
    waiting.recv().unwrap();
    assert!(matches!(rx.recv().unwrap(), Op::Insert { .. }));
    let Op::Sync { reply, .. } = rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
        panic!("expected sync barrier")
    };
    reply.send(()).unwrap();
    assert!(worker.join().unwrap());
}

/// A background retention read must wait for queue room and receive its spans instead of being
/// mistaken for a failed read and suppressing completion accounting.
#[test]
fn full_cache_span_read_waits_for_queue_space() {
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(insert_op()).unwrap();
    let cache = TradeCache { tx };
    let (started, reading) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        cache.held_spans_start("CORE_A", "XALPHA", 0, 1)
    });
    reading.recv().unwrap();
    assert!(matches!(rx.recv().unwrap(), Op::Insert { .. }));
    let Op::Spans { reply, .. } = rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
        panic!("expected span read")
    };
    reply.send(Ok(vec![(0, 1)])).unwrap();
    let answer = worker.join().unwrap();
    assert_eq!(answer.recv().unwrap().unwrap(), vec![(0, 1)]);
}
