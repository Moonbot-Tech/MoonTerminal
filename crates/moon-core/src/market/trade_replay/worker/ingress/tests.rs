//! Held readers progress independently of a blocked held request.

use super::*;

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
