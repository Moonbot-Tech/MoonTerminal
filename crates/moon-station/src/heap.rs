//! The station's global allocator: musl's own, with every live block charged to the thread that
//! allocated it.
//!
//! Why it exists (#1081): a 70-core station grew by about a gigabyte a minute until the kernel
//! killed it, with SQLite's heap in the tens of megabytes, retention and the tape listings idle,
//! and half the process already in swap. RSS alone cannot say who holds the rest, and a heap
//! profiler is not something a user installs on his server. The resource line now ends with this
//! account, so the next such climb names its owner in the journal it already writes.
//!
//! How: each block carries a small header pointing to the account of the thread that
//! allocated it; a free reads it back and takes the bytes off that account, whichever thread frees.
//! A block handed to another thread through a queue therefore stays charged to its producer until
//! the consumer drops it — a queue nobody drains shows up under the thread that fills it. An account
//! records its thread's name at the thread's first allocation; Linux gives a thread its creator's
//! name, which is why moonproto's threads are named `mp-N` where a feed builds its client.
//!
//! Cost: 16 bytes per live block (more only for the rare block aligned past 16), and a small
//! owner record while a thread or one of its blocks lives. Reports visit only live threads;
//! exited threads' surviving allocations share one bucket.
//! The report prints the live block count, so the header's share is read off the same line.

use std::alloc::{GlobalAlloc, Layout, System};
mod accounts;
use accounts::{charge, current_owner};

#[cfg(test)]
mod tests;

/// Bytes kept in front of every block; the owner address sits in its last eight. Sixteen keeps the block
/// itself on the 16-byte alignment musl's `malloc` gives.
const HEADER: usize = 16;
/// How many thread groups a report names.
const REPORTED_GROUPS: usize = 5;
const MIB: i64 = 1024 * 1024;

/// The global allocator: [`System`], with the accounting described in the module docs.
pub struct Tracking;

/// The layout actually asked of [`System`] for `layout`, and how far the caller's block sits
/// past its start. `None` when the header would overflow the size.
fn outer(layout: Layout) -> Option<(Layout, usize)> {
    let align = layout.align().max(HEADER);
    let size = layout.size().checked_add(align)?;
    Layout::from_size_align(size, align)
        .ok()
        .map(|outer| (outer, align))
}

/// Write the exposed address of the block's owner record into its header.
///
/// # Safety
/// `user` is a block this allocator returned, at least [`HEADER`] bytes past its start.
unsafe fn stamp(user: *mut u8, account: usize) {
    // SAFETY: the eight bytes before `user` are inside the header and 8-aligned, since `user` is
    // aligned to at least 16.
    unsafe { user.sub(8).cast::<u64>().write(account as u64) }
}

/// The exposed owner-record address written by [`stamp`].
///
/// # Safety
/// As for [`stamp`], and the block has been stamped.
unsafe fn owner(user: *mut u8) -> usize {
    // SAFETY: see `stamp`.
    unsafe { user.sub(8).cast::<u64>().read() as usize }
}

/// Allocate through `raw` with the header in front, and charge the calling thread.
///
/// # Safety
/// As for [`GlobalAlloc::alloc`].
unsafe fn tracked(layout: Layout, raw: impl FnOnce(Layout) -> *mut u8) -> *mut u8 {
    let Some((full, pad)) = outer(layout) else {
        return std::ptr::null_mut();
    };
    let base = raw(full);
    if base.is_null() {
        return base;
    }
    let account = current_owner();
    // SAFETY: `base` holds `pad + layout.size()` bytes, so `user` and its header are inside it.
    let user = unsafe { base.add(pad) };
    unsafe { stamp(user, account) };
    charge(account, layout.size() as i64, 1);
    user
}

// SAFETY: every method hands `System` a layout it accepts and returns a block `HEADER` bytes or
// one alignment past the start of what `System` returned; frees and reallocations undo exactly that
// offset with the same layout, recomputed from the caller's.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded under the caller's contract.
        unsafe { tracked(layout, |full| System.alloc(full)) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded under the caller's contract.
        unsafe { tracked(layout, |full| System.alloc_zeroed(full)) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // The layout is the one the block was allocated with, so `outer` succeeded then.
        let Some((full, pad)) = outer(layout) else {
            return;
        };
        // SAFETY: `ptr` came from `tracked`, which stamped it `pad` bytes into `System`'s block.
        unsafe {
            charge(owner(ptr), -(layout.size() as i64), -1);
            System.dealloc(ptr.sub(pad), full);
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Some((full, pad)) = outer(layout) else {
            return std::ptr::null_mut();
        };
        // The same bound `outer` puts on an allocation: `System` takes only a valid layout.
        let Some(new_full) = new_size
            .checked_add(pad)
            .filter(|size| Layout::from_size_align(*size, full.align()).is_ok())
        else {
            return std::ptr::null_mut();
        };
        // SAFETY: as for `dealloc`; the header moves with the block, and the new size keeps the
        // same alignment, so the same offset holds in the new block.
        unsafe {
            let previous = owner(ptr);
            let base = System.realloc(ptr.sub(pad), full, new_full);
            if base.is_null() {
                return base;
            }
            let user = base.add(pad);
            let account = current_owner();
            stamp(user, account);
            charge(previous, -(layout.size() as i64), -1);
            charge(account, new_size as i64, 1);
            user
        }
    }
}

/// One thread's live bytes and blocks, read for a report.
struct Held {
    name: String,
    bytes: i64,
    blocks: i64,
}

/// The heap account for the resource line: the live total, then the thread groups holding most.
///
/// Only live threads and the shared exited bucket are visited, regardless of historical churn.
pub fn report() -> String {
    summarize(&accounts::held())
}

/// Threads of one kind share a name up to their number (`feed-17`, `mp-17`): grouped as `feed-#`.
fn group_of(name: &str) -> String {
    let mut group = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !group.ends_with('#') {
                group.push('#');
            }
        } else {
            group.push(c);
        }
    }
    group
}

/// The report text for `held`: total live bytes and blocks, then the [`REPORTED_GROUPS`] groups
/// holding most, each with its thread count and its largest thread.
fn summarize(held: &[Held]) -> String {
    struct Group<'a> {
        bytes: i64,
        blocks: i64,
        threads: usize,
        largest: &'a Held,
    }
    let mut groups: Vec<(String, Group)> = Vec::new();
    for thread in held.iter().filter(|t| t.blocks != 0) {
        let key = group_of(&thread.name);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => {
                group.bytes += thread.bytes;
                group.blocks += thread.blocks;
                group.threads += 1;
                if thread.bytes > group.largest.bytes {
                    group.largest = thread;
                }
            }
            None => groups.push((
                key,
                Group {
                    bytes: thread.bytes,
                    blocks: thread.blocks,
                    threads: 1,
                    largest: thread,
                },
            )),
        }
    }
    groups.sort_by_key(|(_, group)| std::cmp::Reverse(group.bytes));
    let (bytes, blocks) = groups
        .iter()
        .fold((0, 0), |(b, n), (_, g)| (b + g.bytes, n + g.blocks));
    let top = groups
        .iter()
        .take(REPORTED_GROUPS)
        .map(|(key, g)| {
            let mut text = format!("{key} {} MiB/{}k", g.bytes / MIB, g.blocks / 1000);
            if g.threads > 1 {
                text.push_str(&format!(
                    " ({} threads, max {} {} MiB)",
                    g.threads,
                    g.largest.name,
                    g.largest.bytes / MIB
                ));
            }
            text
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Rust heap {} MiB in {}k blocks, by thread: {top}",
        bytes / MIB,
        blocks / 1000
    )
}
