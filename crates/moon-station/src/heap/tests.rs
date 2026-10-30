use super::*;
use std::sync::atomic::Ordering;

/// Release LTO must not turn an optimized temporary's prefix into an undefined owner pointer.
/// Direct Tracking calls miss this: the temporary must cross the global allocator boundary.
#[test]
fn optimized_temporary_strings_keep_their_owner_header() {
    for (currency, stable) in [("usdt", true), ("unit", false), ("usd", true), ("", false)] {
        assert_eq!(
            moon_core::symbol::is_usd_stable(std::hint::black_box(currency)),
            stable
        );
    }
}

/// Prefix recovery must survive zeroing, growth, shrinking and frees after many producers exit.
/// Sixteen concurrent owners stress retirement while consumers keep their blocks alive.
#[test]
fn allocator_blocks_survive_thread_churn_and_cross_thread_reallocation() {
    let producers: Vec<_> = (0..16)
        .map(|seed| {
            std::thread::spawn(move || {
                (0..16)
                    .map(|index| {
                        let mut bytes = vec![0u8; 32 + index];
                        assert!(bytes.iter().all(|&byte| byte == 0));
                        bytes.fill(seed);
                        bytes
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let consumers: Vec<_> = producers
        .into_iter()
        .enumerate()
        .map(|(seed, producer)| {
            let blocks = producer.join().unwrap();
            std::thread::spawn(move || {
                for mut bytes in blocks {
                    assert!(bytes.iter().all(|&byte| byte == seed as u8));
                    bytes.resize(4096, seed as u8);
                    assert!(bytes.iter().all(|&byte| byte == seed as u8));
                    bytes.truncate(17);
                    bytes.shrink_to_fit();
                    assert_eq!(bytes, vec![seed as u8; 17]);
                }
            })
        })
        .collect();
    for consumer in consumers {
        consumer.join().unwrap();
    }
}

/// Measure record reuse and report cost after synthetic short-lived threads exit.
#[test]
#[ignore = "synthetic station heap thread churn measurement"]
fn station_heap_churn_measurement() {
    let mut owners = std::collections::HashSet::new();
    let started = std::time::Instant::now();
    for _ in 0..10_000 {
        owners.insert(std::thread::spawn(current_owner).join().unwrap());
    }
    let elapsed = started.elapsed();
    let started = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(report());
    }
    println!(
        "heap 10000 threads: {:.3} ms, distinct_record_addresses={}, report={:.3} ms, reported_accounts={}",
        elapsed.as_secs_f64() * 1_000.0,
        owners.len(),
        started.elapsed().as_secs_f64() * 10.0,
        accounts::held().len()
    );
}

/// Compare hot allocation accounting with the frozen two-atomic live-owner path.
#[test]
#[ignore = "synthetic station allocator hot-path measurement"]
fn station_allocator_measurement() {
    let layout = Layout::from_size_align(64, 16).unwrap();
    let (full, pad) = outer(layout).unwrap();
    let counters = [
        std::sync::atomic::AtomicI64::new(0),
        std::sync::atomic::AtomicI64::new(0),
    ];
    let _ = current_owner();
    for round in 0..5 {
        for tracked in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = std::time::Instant::now();
            for _ in 0..1_000_000 {
                // SAFETY: both paths allocate a valid layout, stamp within its header, and
                // free exactly once with that same layout. The legacy counters are frozen.
                unsafe {
                    if tracked {
                        let block = Tracking.alloc(layout);
                        assert!(!block.is_null());
                        std::hint::black_box(block);
                        Tracking.dealloc(block, layout);
                    } else {
                        let base = System.alloc(full);
                        assert!(!base.is_null());
                        let block = base.add(pad);
                        block.sub(8).cast::<u64>().write(0);
                        counters[0].fetch_add(64, Ordering::Relaxed);
                        counters[1].fetch_add(1, Ordering::Relaxed);
                        std::hint::black_box(block);
                        counters[0].fetch_add(-64, Ordering::Relaxed);
                        counters[1].fetch_add(-1, Ordering::Relaxed);
                        System.dealloc(base, full);
                    }
                }
            }
            println!(
                "allocator 1000000 tracked={tracked}: {:.3} ms",
                started.elapsed().as_secs_f64() * 1_000.0
            );
        }
    }
}

/// Live bytes and blocks charged to an active owner's record now.
fn account(owner: usize) -> (i64, i64) {
    // SAFETY: callers keep the allocating thread alive until their final counter read.
    let account = unsafe { &*(owner as *const accounts::Account) };
    (
        account.live.load(Ordering::Relaxed),
        account.blocks.load(Ordering::Relaxed),
    )
}

/// Breaks when the header is read from the wrong place or the free charges the freeing thread:
/// the bytes would not come back off the owner's account.
#[test]
fn a_block_is_charged_to_its_allocator_until_another_thread_frees_it() {
    let layout = Layout::from_size_align(1000, 8).unwrap();
    let (result, received) = std::sync::mpsc::channel();
    let release = std::sync::Arc::new(std::sync::Barrier::new(2));
    let theirs = release.clone();
    let worker = std::thread::spawn(move || {
        let owner = current_owner();
        let before = account(owner);
        // SAFETY: a non-zero layout; freed below with the same layout.
        let block = unsafe { Tracking.alloc(layout) };
        assert!(!block.is_null());
        let after = account(owner);
        result
            .send((
                block as usize,
                owner,
                (after.0 - before.0, after.1 - before.1),
            ))
            .unwrap();
        theirs.wait();
    });
    let (addr, producer, charged) = received.recv().unwrap();
    assert_eq!(charged, (1000, 1));

    let held = account(producer);
    // SAFETY: `addr` is the block allocated above with `layout`.
    unsafe { Tracking.dealloc(addr as *mut u8, layout) };
    let freed = account(producer);
    assert_eq!((held.0 - freed.0, held.1 - freed.1), (1000, 1));
    release.wait();
    worker.join().unwrap();
}

/// Breaks when a reallocation keeps the old owner or loses the header: the bytes must move to the
/// thread that grew the block, whose account the queue-consumer reading depends on.
#[test]
fn a_reallocation_moves_the_block_to_the_thread_that_grew_it() {
    let layout = Layout::from_size_align(64, 8).unwrap();
    let (result, received) = std::sync::mpsc::channel();
    let release = std::sync::Arc::new(std::sync::Barrier::new(2));
    let theirs = release.clone();
    let worker = std::thread::spawn(move || {
        // SAFETY: a non-zero layout; reallocated and freed below.
        let block = unsafe { Tracking.alloc(layout) };
        result.send((block as usize, current_owner())).unwrap();
        theirs.wait();
    });
    let (addr, producer) = received.recv().unwrap();
    let mine = current_owner();
    let (owner_before, mine_before) = (account(producer), account(mine));

    // SAFETY: `addr` was allocated with `layout`; 256 is non-zero.
    let grown = unsafe { Tracking.realloc(addr as *mut u8, layout, 256) };
    assert!(!grown.is_null());
    assert_eq!(account(producer).0, owner_before.0 - 64);
    assert_eq!(account(mine).0, mine_before.0 + 256);

    let grown_layout = Layout::from_size_align(256, 8).unwrap();
    // SAFETY: the reallocated block, with its new layout.
    unsafe { Tracking.dealloc(grown, grown_layout) };
    assert_eq!(account(mine).0, mine_before.0);
    release.wait();
    worker.join().unwrap();
}

/// Breaks when an alignment past the header's is not honoured, or the reallocation copies the
/// header over the data: a page-aligned buffer must stay aligned and keep its contents.
#[test]
fn an_over_aligned_block_keeps_its_alignment_and_contents_through_a_reallocation() {
    let layout = Layout::from_size_align(100, 4096).unwrap();
    // SAFETY: a non-zero layout; freed below.
    unsafe {
        let block = Tracking.alloc(layout);
        assert_eq!(block as usize % 4096, 0);
        for i in 0..100 {
            block.add(i).write(i as u8);
        }
        let grown = Tracking.realloc(block, layout, 8192);
        assert_eq!(grown as usize % 4096, 0);
        assert!((0..100).all(|i| grown.add(i).read() == i as u8));
        Tracking.dealloc(grown, Layout::from_size_align(8192, 4096).unwrap());
    }
}

/// Breaks when the numbered threads of one kind are not summed together, or the order is not by
/// bytes held: seventy `mp-N` threads must read as one group above a single larger thread.
#[test]
fn the_report_sums_numbered_threads_into_one_group_ordered_by_bytes() {
    let thread = |name: &str, mib: i64| Held {
        name: name.to_owned(),
        bytes: mib * MIB,
        blocks: mib * 1000,
    };
    let held = [
        thread("main", 100),
        thread("mp-17", 300),
        thread("mp-4", 200),
        thread("feed-17", 10),
        thread("idle", 0),
    ];
    let text = summarize(&held);
    assert!(
        text.starts_with("Rust heap 610 MiB in 610k blocks, by thread: mp-# 500 MiB/500k (2 threads, max mp-17 300 MiB), main 100 MiB/100k, feed-# 10 MiB/10k"),
        "{text}"
    );
}
