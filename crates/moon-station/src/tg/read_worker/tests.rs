//! Read-pool concurrency, completion delivery and paired thread-creation measurements.

use super::*;
use std::time::Duration;

/// A blocked first read must not stop an independent read or consume its completion.
#[test]
fn a_blocked_read_leaves_a_worker_for_other_reads() {
    let (pool, completed) = ReadWorker::start();
    let (release, blocked) = mpsc::channel();
    let (started, running) = mpsc::channel();
    pool.spawn(Box::new(move || {
        started.send(()).unwrap();
        blocked.recv().unwrap();
        Box::new(|_| {})
    }));
    running.recv_timeout(Duration::from_secs(5)).unwrap();
    pool.spawn(Box::new(|| Box::new(|_| {})));
    let _ = completed.recv_timeout(Duration::from_secs(5)).unwrap();
    release.send(()).unwrap();
    let _ = completed.recv_timeout(Duration::from_secs(5)).unwrap();
}

/// Paired empty reads expose pool handoff cost without database time hiding thread creation.
#[test]
#[ignore = "synthetic station read pool before/after measurement"]
fn station_read_pool_measurement() {
    for round in 0..5 {
        for pooled in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let (pool, completed) = ReadWorker::start();
            let (legacy, legacy_done) = mpsc::channel();
            let started = std::time::Instant::now();
            for i in 0..1_000 {
                if pooled {
                    pool.spawn(Box::new(|| Box::new(|_| {})));
                    let _ = completed.recv().unwrap();
                } else {
                    let legacy = legacy.clone();
                    std::thread::Builder::new()
                        .name("tg-read".into())
                        .spawn(move || {
                            legacy.send(i).unwrap();
                        })
                        .unwrap();
                    assert_eq!(legacy_done.recv().unwrap(), i);
                }
            }
            println!(
                "reads 1000 pooled={pooled}: {:.3} ms",
                started.elapsed().as_secs_f64() * 1_000.0
            );
        }
    }
}
