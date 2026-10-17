//! Held readers progress independently of a blocked held request.

use super::*;
use crate::feed::Side;

/// A slow first read cannot monopolize the pool's receiver or delay the second answer.
#[test]
fn held_readers_answer_while_another_read_waits() {
    let (blocked, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    let pool = held_pool(move |query| {
        if query.market == "XWAIT" {
            blocked.send(()).unwrap();
            wait.lock().unwrap().recv().unwrap();
        }
        TickAnswer {
            ticks: Vec::new(),
            covered: query.spans.clone(),
            settled: true,
        }
    });
    let (first_reply, first) = mpsc::channel();
    pool.send(TickQuery {
        exchange_key: "VENUE_A".into(),
        market: "XWAIT".into(),
        spans: Coverage::one((0, 9)),
        reply: first_reply,
    })
    .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let (second_reply, second) = mpsc::channel();
    pool.send(TickQuery {
        exchange_key: "VENUE_A".into(),
        market: "XREADY".into(),
        spans: Coverage::one((20, 29)),
        reply: second_reply,
    })
    .unwrap();
    let independent = second.recv_timeout(Duration::from_secs(5));
    release.send(()).unwrap();
    assert_eq!(independent.unwrap().covered, Coverage::one((20, 29)));
    assert_eq!(
        first.recv_timeout(Duration::from_secs(5)).unwrap().covered,
        Coverage::one((0, 9))
    );
}

/// A held query sent after `forget_tiles` returns never sees the forgotten prints, even while
/// the coordinator is busy and has not drained a single message.
#[test]
fn held_query_after_forget_never_sees_forgotten_tiles() {
    let tiles = Arc::new(Mutex::new(TickTileStore::default()));
    let cache = Mutex::new(VecDeque::new());
    let key: TileKey = ("VENUE_A".into(), "XFORGET".into());
    let print = Tick {
        time_ms: 5.0,
        price: 1.0,
        qty: 1.0,
        side: Side::Buy,
    };
    lock_tiles(&tiles).insert(key.clone(), 0, 9, vec![print], TileSource::Core);
    // The coordinator's queue exists but nothing ever reads it: a busy coordinator.
    let (_busy, _never_drained) = mpsc::channel::<Inbound>();
    let readers = tiles.clone();
    let pool = held_pool(move |query| {
        let key: TileKey = (query.exchange_key.clone(), query.market.clone());
        TickAnswer {
            ticks: lock_tiles(&readers).read(&key, 0, 9),
            covered: query.spans.clone(),
            settled: true,
        }
    });
    forget_stores(&tiles, &cache);
    let (reply, answer) = mpsc::channel();
    pool.send(TickQuery {
        exchange_key: key.0.clone(),
        market: key.1.clone(),
        spans: Coverage::one((0, 9)),
        reply,
    })
    .unwrap();
    let answer = answer.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(answer.ticks.is_empty());
}
