//! The station's global allocator: musl's own, with every live block charged to the thread that
//! allocated it.
//!
//! Why it exists (#1081): a 70-core station grew by about a gigabyte a minute until the kernel
//! killed it, with SQLite's heap in the tens of megabytes, retention and the tape listings idle,
//! and half the process already in swap. RSS alone cannot say who holds the rest, and a heap
//! profiler is not something a user installs on his server. The resource line now ends with this
//! account, so the next such climb names its owner in the journal it already writes.
//!
//! How: each block carries a small header in front of it holding the slot of the thread that
//! allocated it; a free reads it back and takes the bytes off that slot, whichever thread frees.
//! A block handed to another thread through a queue therefore stays charged to its producer until
//! the consumer drops it — a queue nobody drains shows up under the thread that fills it. A slot
//! records its thread's name at the thread's first allocation; Linux gives a thread its creator's
//! name, which is why moonproto's threads are named `mp-N` where a feed builds its client.
//!
//! Cost: 16 bytes per live block (more only for the rare block aligned past 16) and one relaxed
//! atomic add per allocation and per free — musl's allocator takes a lock on each call anyway.
//! The report prints the live block count, so the header's share is read off the same line.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

#[cfg(test)]
mod tests;

/// Thread slots. A thread takes one for life at its first allocation; threads past this many
/// share the last one, reported as `other`. The station starts threads all its life — a listing
/// per core every hour, a client's threads on every reconnect — so the table is sized for about
/// a year of that, not for the threads alive at once. Zero-initialised, it costs address space
/// until claimed: every thread ever started keeps 64 resident bytes here for the life of the
/// process (a million threads would be 64 MiB), and a report reads claimed slots alone.
const SLOTS: usize = 1 << 20;
/// The shared slot of every thread past [`SLOTS`]` - 1`.
const OVERFLOW: u32 = SLOTS as u32 - 1;
/// Bytes kept in front of every block; the slot sits in its last eight. Sixteen keeps the block
/// itself on the 16-byte alignment musl's `malloc` gives.
const HEADER: usize = 16;
/// How many thread groups a report names.
const REPORTED_GROUPS: usize = 5;
const MIB: i64 = 1024 * 1024;

/// One thread's account. Cache-line sized, so two threads allocating at once do not contend on
/// one line.
#[repr(align(64))]
struct Slot {
    /// Bytes of the blocks this thread allocated that are still live.
    live: AtomicI64,
    /// How many such blocks.
    blocks: AtomicI64,
    /// The thread's name as the kernel had it at the first allocation, NUL-padded.
    name: [AtomicU64; 2],
}

impl Slot {
    const fn new() -> Self {
        Self {
            live: AtomicI64::new(0),
            blocks: AtomicI64::new(0),
            name: [AtomicU64::new(0), AtomicU64::new(0)],
        }
    }
}

static ACCOUNTS: [Slot; SLOTS] = [const { Slot::new() }; SLOTS];
/// The next unclaimed slot.
static NEXT: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// This thread's slot; `u32::MAX` until its first allocation. Const-initialised and without a
    /// destructor, so reading it neither allocates nor registers anything.
    static SLOT: Cell<u32> = const { Cell::new(u32::MAX) };
}

/// The global allocator: [`System`], with the accounting described in the module docs.
pub struct Tracking;

/// The calling thread's slot, claimed on first use. A thread whose locals are already torn down
/// is charged to the shared slot.
fn current_slot() -> u32 {
    SLOT.try_with(|slot| match slot.get() {
        u32::MAX => {
            let claimed = claim();
            slot.set(claimed);
            claimed
        }
        held => held,
    })
    .unwrap_or(OVERFLOW)
}

/// Take the next slot and record the calling thread's name in it.
fn claim() -> u32 {
    let slot = NEXT.fetch_add(1, Ordering::Relaxed).min(OVERFLOW);
    if slot < OVERFLOW {
        let name = thread_name();
        let account = &ACCOUNTS[slot as usize];
        account.name[0].store(
            u64::from_le_bytes(name[..8].try_into().unwrap_or_default()),
            Ordering::Relaxed,
        );
        account.name[1].store(
            u64::from_le_bytes(name[8..].try_into().unwrap_or_default()),
            Ordering::Relaxed,
        );
    }
    slot
}

/// The calling thread's kernel name, NUL-padded. Elsewhere than Linux: all zeros.
#[cfg(not(target_os = "linux"))]
fn thread_name() -> [u8; 16] {
    [0; 16]
}

/// The calling thread's kernel name, NUL-padded. A system call into a stack buffer: nothing here
/// may allocate.
#[cfg(target_os = "linux")]
fn thread_name() -> [u8; 16] {
    let mut buf = [0u8; 16];
    // SAFETY: PR_GET_NAME writes at most 16 bytes, NUL included, into a buffer of that size.
    unsafe {
        libc::prctl(
            libc::PR_GET_NAME,
            buf.as_mut_ptr() as libc::c_ulong,
            0,
            0,
            0,
        );
    }
    buf
}

/// The layout actually asked of [`System`] for `layout`, and how far the caller's block sits
/// past its start. `None` when the header would overflow the size.
fn outer(layout: Layout) -> Option<(Layout, usize)> {
    let align = layout.align().max(HEADER);
    let size = layout.size().checked_add(align)?;
    Layout::from_size_align(size, align)
        .ok()
        .map(|outer| (outer, align))
}

/// Charge `bytes` in one block to `slot`, or release them with negative values.
fn charge(slot: u32, bytes: i64, blocks: i64) {
    let account = &ACCOUNTS[slot as usize];
    account.live.fetch_add(bytes, Ordering::Relaxed);
    account.blocks.fetch_add(blocks, Ordering::Relaxed);
}

/// Write the owner's slot in front of the caller's block.
///
/// # Safety
/// `user` is a block this allocator returned, at least [`HEADER`] bytes past its start.
unsafe fn stamp(user: *mut u8, slot: u32) {
    // SAFETY: the eight bytes before `user` are inside the header and 8-aligned, since `user` is
    // aligned to at least 16.
    unsafe { user.sub(8).cast::<u64>().write(u64::from(slot)) }
}

/// The owner's slot written by [`stamp`].
///
/// # Safety
/// As for [`stamp`], and the block has been stamped.
unsafe fn owner(user: *mut u8) -> u32 {
    // SAFETY: see `stamp`.
    unsafe { user.sub(8).cast::<u64>().read() as u32 }
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
    let slot = current_slot();
    // SAFETY: `base` holds `pad + layout.size()` bytes, so `user` and its header are inside it.
    let user = unsafe { base.add(pad) };
    unsafe { stamp(user, slot) };
    charge(slot, layout.size() as i64, 1);
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
            let slot = current_slot();
            stamp(user, slot);
            charge(previous, -(layout.size() as i64), -1);
            charge(slot, new_size as i64, 1);
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
/// Only slots still holding a block are read out: the slots of the many threads that exited
/// clean would otherwise cost a name string each, every report.
pub fn report() -> String {
    let used = (NEXT.load(Ordering::Relaxed) as usize).min(SLOTS);
    let held: Vec<Held> = ACCOUNTS[..used]
        .iter()
        .enumerate()
        .filter_map(|(slot, account)| {
            let blocks = account.blocks.load(Ordering::Relaxed);
            (blocks != 0).then(|| Held {
                name: slot_name(slot as u32, account),
                bytes: account.live.load(Ordering::Relaxed),
                blocks,
            })
        })
        .collect();
    summarize(&held)
}

/// The recorded name of one slot; `other` for the shared overflow slot, `unnamed` for a thread
/// the kernel gave no name.
fn slot_name(slot: u32, account: &Slot) -> String {
    if slot == OVERFLOW {
        return "other".to_owned();
    }
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&account.name[0].load(Ordering::Relaxed).to_le_bytes());
    bytes[8..].copy_from_slice(&account.name[1].load(Ordering::Relaxed).to_le_bytes());
    let len = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    match len {
        0 => "unnamed".to_owned(),
        _ => String::from_utf8_lossy(&bytes[..len]).into_owned(),
    }
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
