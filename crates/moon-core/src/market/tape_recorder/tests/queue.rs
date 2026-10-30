use super::*;

/// Build one tick-bearing command without touching a real archive or database.
fn backfill() -> Cmd {
    Cmd::Backfilled {
        exchange: "CORE_A".into(),
        market: "XALPHA".into(),
        file: Coverage::none(),
        ticks: Vec::new(),
    }
}

/// Only the background backfill producer may wait when its bounded queue is full; every span is
/// still delivered and small event traffic uses its independent channel.
#[test]
fn full_backfill_queue_backpressures_only_its_producer() {
    let (events, event_rx) = mpsc::channel();
    let (backfills, backfill_rx) = mpsc::sync_channel(1);
    let tx = RecorderTx { events, backfills };
    tx.send(backfill()).unwrap();
    let (started, writing) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        started.send(()).unwrap();
        tx.send(backfill()).unwrap();
    });
    writing.recv().unwrap();
    assert!(matches!(
        backfill_rx.recv().unwrap(),
        Cmd::Backfilled { .. }
    ));
    assert!(matches!(
        backfill_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Cmd::Backfilled { .. }
    ));
    writer.join().unwrap();
    drop(event_rx);
}

/// A wake containing settlement before a late-drained backfill must still file the backfill first;
/// reversing them would mark fetched tape missing and suppress recovery for an hour.
#[test]
fn backfills_apply_before_catchup_done() {
    let mut pending = vec![
        Cmd::CatchupDone(catchup::PassResult {
            finished: Vec::new(),
            archive_bytes: 0,
        }),
        backfill(),
    ];
    backfills_first(&mut pending);
    assert!(matches!(pending[0], Cmd::Backfilled { .. }));
    assert!(matches!(pending[1], Cmd::CatchupDone(_)));
}
