use super::*;

/// Live bytes and blocks charged to `slot` now.
fn account(slot: u32) -> (i64, i64) {
    let account = &ACCOUNTS[slot as usize];
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
    let (addr, owner_slot, charged) = std::thread::spawn(move || {
        let slot = current_slot();
        let before = account(slot);
        // SAFETY: a non-zero layout; freed below with the same layout.
        let block = unsafe { Tracking.alloc(layout) };
        assert!(!block.is_null());
        let after = account(slot);
        (
            block as usize,
            slot,
            (after.0 - before.0, after.1 - before.1),
        )
    })
    .join()
    .unwrap();
    assert_eq!(charged, (1000, 1));

    let held = account(owner_slot);
    // SAFETY: `addr` is the block allocated above with `layout`.
    unsafe { Tracking.dealloc(addr as *mut u8, layout) };
    let freed = account(owner_slot);
    assert_eq!((held.0 - freed.0, held.1 - freed.1), (1000, 1));
}

/// Breaks when a reallocation keeps the old owner or loses the header: the bytes must move to the
/// thread that grew the block, whose account the queue-consumer reading depends on.
#[test]
fn a_reallocation_moves_the_block_to_the_thread_that_grew_it() {
    let layout = Layout::from_size_align(64, 8).unwrap();
    let (addr, owner_slot) = std::thread::spawn(move || {
        // SAFETY: a non-zero layout; reallocated and freed below.
        let block = unsafe { Tracking.alloc(layout) };
        (block as usize, current_slot())
    })
    .join()
    .unwrap();
    let mine = current_slot();
    let (owner_before, mine_before) = (account(owner_slot), account(mine));

    // SAFETY: `addr` was allocated with `layout`; 256 is non-zero.
    let grown = unsafe { Tracking.realloc(addr as *mut u8, layout, 256) };
    assert!(!grown.is_null());
    assert_eq!(account(owner_slot).0, owner_before.0 - 64);
    assert_eq!(account(mine).0, mine_before.0 + 256);

    let grown_layout = Layout::from_size_align(256, 8).unwrap();
    // SAFETY: the reallocated block, with its new layout.
    unsafe { Tracking.dealloc(grown, grown_layout) };
    assert_eq!(account(mine).0, mine_before.0);
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
