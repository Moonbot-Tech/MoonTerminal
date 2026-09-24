use std::time::{Duration, Instant};

use super::*;

/// Asking records nothing: two lanes of one host may both be sent to while neither has been
/// refused, and a refusal is what starts the backoff — at the floor, doubling per refusal in a
/// row, gone once the venue answers again.
#[test]
fn only_a_refusal_starts_the_backoff_and_an_answer_ends_it() {
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let t0 = Instant::now();
    assert_eq!(gate.claim(host, t0), Ok(()));
    assert_eq!(
        gate.claim(host, t0),
        Ok(()),
        "a second asker is not refused by the first"
    );
    gate.refuse(host, t0);
    assert_eq!(gate.claim(host, t0), Err(RETRY_MIN_S));
    assert_eq!(
        gate.claim(host, t0 + Duration::from_secs(u64::from(RETRY_MIN_S))),
        Ok(()),
        "the floor waited out"
    );
    // A second refusal in a row doubles the wait.
    gate.refuse(host, t0 + Duration::from_secs(u64::from(RETRY_MIN_S)));
    assert_eq!(
        gate.refused_for(host, t0 + Duration::from_secs(u64::from(RETRY_MIN_S))),
        Some(2 * RETRY_MIN_S)
    );
    let refused_at = t0 + Duration::from_secs(u64::from(RETRY_MIN_S));
    // An answer to a request sent before or AT the refusal proves nothing and leaves it
    // standing; one sent after it erases the history.
    gate.clear(host, refused_at - Duration::from_millis(1));
    gate.clear(host, refused_at);
    assert_eq!(gate.refused_for(host, refused_at), Some(2 * RETRY_MIN_S));
    gate.clear(host, refused_at + Duration::from_millis(1));
    assert_eq!(gate.claim(host, t0), Ok(()), "an answer erases the history");
    assert_eq!(
        gate.claim("api.gateio.ws", t0),
        Ok(()),
        "another host is another budget"
    );
}

/// Two threads pacing one host — a chart lane beside a model lane — never book slots closer
/// than the floor to each other. Judged on the BOOKED slots, which the gate sets exactly; the
/// wake-ups after them are the scheduler's and are not what the floor promises.
#[test]
fn two_lanes_of_one_host_keep_the_floor_between_them() {
    use std::sync::{Arc, Mutex};
    let gate = Arc::new(ReplayGate::new());
    let host = "fapi.binance.com";
    let floor = Duration::from_millis(40);
    let booked: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
    let lanes: Vec<_> = (0..2)
        .map(|_| {
            let gate = gate.clone();
            let booked = booked.clone();
            std::thread::spawn(move || {
                for _ in 0..4 {
                    let slot = gate.pace_at(host, floor);
                    booked.lock().unwrap().push(slot);
                }
            })
        })
        .collect();
    for lane in lanes {
        lane.join().unwrap();
    }
    let mut booked = booked.lock().unwrap().clone();
    booked.sort();
    assert_eq!(booked.len(), 8);
    for pair in booked.windows(2) {
        let gap = pair[1].duration_since(pair[0]);
        assert!(
            gap >= floor,
            "two slots {gap:?} apart, under the {floor:?} floor"
        );
    }
}

/// The floor between two requests to one host holds across callers: a second caller books
/// its slot after the FIRST caller's booked slot plus the floor, even while that slot still
/// lies in the future — where `elapsed` would have read zero.
#[test]
fn the_floor_holds_across_two_callers_of_one_host() {
    let gate = ReplayGate::new();
    let host = "fapi.binance.com";
    let floor = Duration::from_millis(120);
    let started = Instant::now();
    let first = gate.pace_at(host, floor);
    // Booked at once after the first: the floor from the first's slot, not from now.
    let second = gate.pace_at(host, floor);
    assert!(
        second.duration_since(first) >= floor,
        "second slot {:?} after the first",
        second.duration_since(first)
    );
    // A third caller under a smaller floor still books after the second's slot.
    let third = gate.pace_at(host, Duration::from_millis(10));
    assert!(third >= second + Duration::from_millis(10));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "no runaway wait"
    );
}
