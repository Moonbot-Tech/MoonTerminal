//! Network-free regression checks for login lifetime, identity isolation and bounded replay.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Count opened transports independently of the cache's stored entries.
fn opener(count: &AtomicUsize) -> impl FnMut() -> anyhow::Result<usize> + '_ {
    || Ok(count.fetch_add(1, Ordering::SeqCst) + 1)
}

/// Removing expiry or failing to refresh use would retain a login forever or reconnect too early.
#[test]
fn reuse_refreshes_idle_deadline_and_expiry_opens_fresh() {
    let mut cache = Cache::new();
    let opened = AtomicUsize::new(0);
    let start = Instant::now();
    let mut exchange = |seconds| {
        cache
            .exchange(
                &"peer",
                || start + Duration::from_secs(seconds),
                opener(&opened),
                |conn| Ok(*conn),
                |_| false,
            )
            .unwrap()
    };
    assert_eq!(exchange(0), 1);
    assert_eq!(exchange(119), 1);
    assert_eq!(exchange(238), 1);
    assert_eq!(exchange(358), 2);
    assert_eq!(opened.load(Ordering::SeqCst), 2);
}

/// A timer must actually drop all expired connections, even peers no caller selects again.
#[test]
fn expire_drops_idle_peers_without_another_exchange() {
    /// Observe destruction rather than merely checking the cache's length.
    struct Login(Arc<AtomicUsize>);
    impl Drop for Login {
        /// A dropped login represents the SSH disconnect and runtime shutdown.
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut cache = Cache::new();
    let start = Instant::now();
    cache
        .connect(&"first", start, &mut || Ok(Login(Arc::clone(&dropped))))
        .unwrap();
    cache
        .connect(&"second", start + Duration::from_secs(10), &mut || {
            Ok(Login(Arc::clone(&dropped)))
        })
        .unwrap();
    cache.expire(start + Duration::from_secs(119));
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    cache.expire(start + Duration::from_secs(120));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    cache.expire(start + Duration::from_secs(130));
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
}

/// Omitting any login input can authenticate a changed target through another identity's session.
#[test]
fn each_target_identity_component_selects_another_connection() {
    let original = Identity {
        host: "fixture.invalid".into(),
        port: 22,
        user: "fixture-admin".into(),
        key: "fixture-key".into(),
        pin: "fixture-pin".into(),
    };
    let mut changed = vec![original.clone(); 5];
    changed[0].host = "other.invalid".into();
    changed[1].port = 2222;
    changed[2].user = "other-admin".into();
    changed[3].key = "other-key".into();
    changed[4].pin = "other-pin".into();
    let mut cache = Cache::new();
    let opened = AtomicUsize::new(0);
    for (index, key) in std::iter::once(&original).chain(changed.iter()).enumerate() {
        let id = cache
            .exchange(
                key,
                Instant::now,
                opener(&opened),
                |conn| Ok(*conn),
                |_| false,
            )
            .unwrap();
        assert_eq!(id, index + 1);
    }
    assert_eq!(
        cache
            .exchange(
                &original,
                Instant::now,
                opener(&opened),
                |conn| Ok(*conn),
                |_| false
            )
            .unwrap(),
        1
    );
    assert_eq!(opened.load(Ordering::SeqCst), 6);
}

/// A stale reused transport must reopen once and replay only the failed exchange.
#[test]
fn dead_reused_connection_retries_once_and_keeps_its_replacement() {
    let mut cache = Cache::new();
    let opened = AtomicUsize::new(0);
    cache
        .connect(&"peer", Instant::now(), &mut opener(&opened))
        .unwrap();
    let mut calls = Vec::new();
    let result = cache
        .exchange(
            &"peer",
            Instant::now,
            opener(&opened),
            |conn| {
                calls.push(*conn);
                if *conn == 1 {
                    Err(crate::ssh::TransportLost.into())
                } else {
                    Ok(*conn)
                }
            },
            |e| e.is::<crate::ssh::TransportLost>(),
        )
        .unwrap();
    assert_eq!(result, 2);
    assert_eq!(calls, [1, 2]);
    assert_eq!(
        cache
            .exchange(
                &"peer",
                Instant::now,
                opener(&opened),
                |conn| Ok(*conn),
                |_| false
            )
            .unwrap(),
        2
    );
    assert_eq!(opened.load(Ordering::SeqCst), 2);
}

/// Retrying fresh failures or retrying twice can duplicate a station mutation indefinitely.
#[test]
fn transport_retry_is_bounded_and_fresh_failures_are_not_replayed() {
    let mut cache = Cache::new();
    let opened = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let run = |_: &usize| -> anyhow::Result<()> {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(crate::ssh::TransportLost.into())
    };
    assert!(
        cache
            .exchange(&"peer", Instant::now, opener(&opened), run, |e| {
                e.is::<crate::ssh::TransportLost>()
            })
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    cache
        .connect(&"peer", Instant::now(), &mut opener(&opened))
        .unwrap();
    assert!(
        cache
            .exchange(&"peer", Instant::now, opener(&opened), run, |e| {
                e.is::<crate::ssh::TransportLost>()
            })
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(opened.load(Ordering::SeqCst), 3);
    assert!(cache.entries.is_empty());
}

/// Command/API errors and completed replies must preserve the transport without replaying writes.
#[test]
fn application_errors_and_completed_replies_are_never_retried() {
    let mut cache = Cache::new();
    let opened = AtomicUsize::new(0);
    cache
        .connect(&"peer", Instant::now(), &mut opener(&opened))
        .unwrap();
    let calls = AtomicUsize::new(0);
    let result: anyhow::Result<()> = cache.exchange(
        &"peer",
        Instant::now,
        opener(&opened),
        |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("fixture command refused")
        },
        |e| e.is::<crate::ssh::TransportLost>(),
    );
    assert_eq!(result.unwrap_err().to_string(), "fixture command refused");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        cache
            .exchange(
                &"peer",
                Instant::now,
                opener(&opened),
                |_| Ok("reply"),
                |e| e.is::<crate::ssh::TransportLost>()
            )
            .unwrap(),
        "reply"
    );
    assert_eq!(opened.load(Ordering::SeqCst), 1);
}

/// Concurrent Settings and pull jobs must open once and never overlap transport exchanges.
#[test]
fn simultaneous_callers_share_one_serialized_transport() {
    let shared = Arc::new(Shared {
        cache: Mutex::new(Cache::new()),
        wake: Condvar::new(),
    });
    let opened = AtomicUsize::new(0);
    let active = AtomicUsize::new(0);
    let start = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    shared
                        .exchange(
                            &"peer",
                            opener(&opened),
                            |conn| {
                                assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                                for _ in 0..100 {
                                    std::thread::yield_now();
                                }
                                assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                                Ok(*conn)
                            },
                            |_| false,
                        )
                        .unwrap()
                })
            })
            .collect();
        for job in jobs {
            assert_eq!(job.join().unwrap(), 1);
        }
    });
    assert_eq!(opened.load(Ordering::SeqCst), 1);
}
