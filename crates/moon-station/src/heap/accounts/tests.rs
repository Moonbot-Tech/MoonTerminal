//! Owner retirement with live allocations and late cross-thread frees.

use super::*;

/// Retired records must leave the scan before their remaining blocks are freed elsewhere.
#[test]
fn exited_owners_leave_the_live_list_while_their_blocks_remain_accounted() {
    let owner = claim();
    assert_ne!(owner, other());
    charge(owner, 123_456, 1);
    retire(owner);
    {
        let _registry = lock(&REGISTRY);
        let mut cursor = HEAD.load(Ordering::Relaxed);
        while !cursor.is_null() {
            assert_ne!(cursor as usize, owner);
            // SAFETY: the registry lock keeps the live list alive while it is traversed.
            cursor = unsafe { (*cursor).next.load(Ordering::Relaxed) };
        }
    }
    let snapshot = held();
    let exited = snapshot
        .iter()
        .find(|account| account.name == "exited")
        .unwrap();
    assert!(exited.bytes >= 123_456 && exited.blocks >= 1);
    // This simulates the final block's header being freed on another thread after owner exit.
    std::thread::spawn(move || charge(owner, -123_456, -1))
        .join()
        .unwrap();
}

/// A real Tracking block must retain its owner through exit, address reuse and a later free.
#[test]
fn a_block_outlives_its_producer_and_many_later_threads() {
    let layout = Layout::from_size_align(4_096, 16).unwrap();
    let address = std::thread::spawn(move || {
        // SAFETY: a valid nonzero layout, retained and freed by the joining thread below.
        let block = unsafe { super::super::Tracking.alloc(layout) };
        assert!(!block.is_null());
        // SAFETY: the returned allocation holds 4096 writable bytes.
        unsafe {
            block.write_bytes(0x5a, 4_096);
        }
        block as usize
    })
    .join()
    .unwrap();
    for _ in 0..2 {
        std::thread::spawn(|| {
            std::hint::black_box(current_owner());
        })
        .join()
        .unwrap();
    }
    // SAFETY: the block remains allocated until this final deallocation on a different thread.
    unsafe {
        assert!((0..4_096).all(|i| (address as *const u8).add(i).read() == 0x5a));
        super::super::Tracking.dealloc(address as *mut u8, layout);
    }
}
