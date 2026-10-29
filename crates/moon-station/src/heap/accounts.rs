//! Allocation-free owner records and the live-thread registry used by heap reports.
//!
//! A block header retains its owner's record until its last block is freed. Thread exit removes
//! the record from the reporting list and folds its bytes into one exited bucket; only actual
//! retained blocks can keep an exited record alive. No historical thread table remains to scan.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, Ordering};

/// One owner's counters and immutable name; links are protected by the registry lock.
#[repr(align(64))]
pub(super) struct Account {
    pub(super) live: AtomicI64,
    pub(super) blocks: AtomicI64,
    name: [u8; 16],
    active: AtomicBool,
    lock: AtomicBool,
    next: AtomicPtr<Account>,
}

impl Account {
    /// Build a record without calling the global allocator.
    const fn new(name: [u8; 16]) -> Self {
        Self {
            live: AtomicI64::new(0),
            blocks: AtomicI64::new(0),
            name,
            active: AtomicBool::new(true),
            lock: AtomicBool::new(false),
            next: AtomicPtr::new(std::ptr::null_mut()),
        }
    }
}

static HEAD: AtomicPtr<Account> = AtomicPtr::new(std::ptr::null_mut());
static REGISTRY: AtomicBool = AtomicBool::new(false);
/// Late TLS allocations and a failed owner-record allocation remain visible under `other`.
static OTHER: Account = Account::new(*b"other\0\0\0\0\0\0\0\0\0\0\0");
static EXITED_BYTES: AtomicI64 = AtomicI64::new(0);
static EXITED_BLOCKS: AtomicI64 = AtomicI64::new(0);

thread_local! {
    /// Non-dropping TLS is set before destructor registration, which may itself allocate.
    static OWNER: Cell<usize> = const { Cell::new(0) };
    static RELEASE: Release = const { Release };
}

/// Register a destructor only after recursive allocator calls can see their owner.
struct Release;
impl Drop for Release {
    /// Late destructor allocations use the static account, never the retired record.
    fn drop(&mut self) {
        if let Ok(owner) = OWNER.try_with(|owner| owner.replace(other())) {
            if owner != 0 && owner != other() {
                retire(owner);
            }
        }
    }
}

/// A short, allocation-free lock; no OS or Rust allocation can occur while obtaining it.
struct Guard<'a>(&'a AtomicBool);
impl Drop for Guard<'_> {
    /// Release the counters or registry to its next reader.
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Serialize a few atomic operations without allocating a mutex's TLS bookkeeping.
fn lock(flag: &AtomicBool) -> Guard<'_> {
    while flag
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
    Guard(flag)
}

/// The static fallback's exposed address, suitable for the allocation header.
fn other() -> usize {
    &OTHER as *const Account as usize
}

/// Obtain the current thread's owner without recursion through global allocation.
pub(super) fn current_owner() -> usize {
    OWNER
        .try_with(|owner| {
            if owner.get() == 0 {
                let record = claim();
                owner.set(record);
                let _ = RELEASE.try_with(|_| {});
            }
            owner.get()
        })
        .unwrap_or_else(|_| other())
}

/// Allocate one record directly from System and add it to the live-thread list.
fn claim() -> usize {
    // SAFETY: a nonzero valid layout; this record is freed only after exit and its last block.
    let record = unsafe { System.alloc(Layout::new::<Account>()).cast::<Account>() };
    if record.is_null() {
        return other();
    }
    // SAFETY: the allocation above provides aligned, writable storage for Account.
    unsafe {
        record.write(Account::new(thread_name()));
    }
    let _registry = lock(&REGISTRY);
    // SAFETY: registry ownership keeps every linked record alive and excludes link mutations.
    unsafe {
        (*record)
            .next
            .store(HEAD.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    HEAD.store(record, Ordering::Relaxed);
    record as usize
}

/// Remove a thread from reports and transfer all of its surviving blocks to the exited bucket.
fn retire(owner: usize) {
    let record = owner as *mut Account;
    let registry = lock(&REGISTRY);
    let mut link = &HEAD;
    let mut cursor = link.load(Ordering::Relaxed);
    while !cursor.is_null() {
        // SAFETY: the registry lock keeps the live list and all of its records alive.
        let account = unsafe { &*cursor };
        if cursor == record {
            link.store(account.next.load(Ordering::Relaxed), Ordering::Relaxed);
            break;
        }
        link = &account.next;
        cursor = link.load(Ordering::Relaxed);
    }
    // SAFETY: the exiting thread still owns this record; only this call can retire it.
    let account = unsafe { &*record };
    let guard = lock(&account.lock);
    account.active.store(false, Ordering::Relaxed);
    EXITED_BYTES.fetch_add(account.live.load(Ordering::Relaxed), Ordering::Relaxed);
    let blocks = account.blocks.load(Ordering::Relaxed);
    EXITED_BLOCKS.fetch_add(blocks, Ordering::Relaxed);
    drop(guard);
    drop(registry);
    if blocks == 0 {
        // SAFETY: removed from the live list, with no remaining header referring to it.
        unsafe {
            System.dealloc(record.cast(), Layout::new::<Account>());
        }
    }
}

/// Charge a block to its producer; a free after producer exit also debits the exited bucket.
pub(super) fn charge(owner: usize, bytes: i64, blocks: i64) {
    let record = owner as *mut Account;
    // SAFETY: the current thread or the block being freed keeps this owner record alive.
    let account = unsafe { &*record };
    let guard = lock(&account.lock);
    account.live.fetch_add(bytes, Ordering::Relaxed);
    let remaining = account.blocks.fetch_add(blocks, Ordering::Relaxed) + blocks;
    let exited = !account.active.load(Ordering::Relaxed);
    if exited {
        EXITED_BYTES.fetch_add(bytes, Ordering::Relaxed);
        EXITED_BLOCKS.fetch_add(blocks, Ordering::Relaxed);
    }
    drop(guard);
    if exited && remaining == 0 {
        // SAFETY: only the last freeing block can reach zero after its owner's retirement.
        unsafe {
            System.dealloc(record.cast(), Layout::new::<Account>());
        }
    }
}

/// Snapshot live owners and the two shared buckets without walking historical threads.
pub(super) fn held() -> Vec<super::Held> {
    // Registration must precede the list lock: allocating the output must not claim a new owner.
    let _ = current_owner();
    let _registry = lock(&REGISTRY);
    let mut held = Vec::new();
    let mut cursor = HEAD.load(Ordering::Relaxed);
    while !cursor.is_null() {
        // SAFETY: the registry lock excludes removal and destruction of live records.
        let account = unsafe { &*cursor };
        append(&mut held, account);
        cursor = account.next.load(Ordering::Relaxed);
    }
    append(&mut held, &OTHER);
    let blocks = EXITED_BLOCKS.load(Ordering::Relaxed);
    if blocks != 0 {
        held.push(super::Held {
            name: "exited".into(),
            bytes: EXITED_BYTES.load(Ordering::Relaxed),
            blocks,
        });
    }
    held
}

/// Read counters under their lock, then allocate the report text after releasing it.
fn append(held: &mut Vec<super::Held>, account: &Account) {
    let guard = lock(&account.lock);
    let (bytes, blocks) = (
        account.live.load(Ordering::Relaxed),
        account.blocks.load(Ordering::Relaxed),
    );
    drop(guard);
    if blocks == 0 {
        return;
    }
    let len = account
        .name
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(16);
    let name = if len == 0 {
        "unnamed".into()
    } else {
        String::from_utf8_lossy(&account.name[..len]).into_owned()
    };
    held.push(super::Held {
        name,
        bytes,
        blocks,
    });
}

/// Non-Linux test builds have no kernel thread-name query.
#[cfg(not(target_os = "linux"))]
fn thread_name() -> [u8; 16] {
    [0; 16]
}

/// Linux kernel name, read into stack storage without invoking the global allocator.
#[cfg(target_os = "linux")]
fn thread_name() -> [u8; 16] {
    let mut name = [0; 16];
    // SAFETY: PR_GET_NAME writes at most 16 bytes into this 16-byte buffer.
    unsafe {
        libc::prctl(
            libc::PR_GET_NAME,
            name.as_mut_ptr() as libc::c_ulong,
            0,
            0,
            0,
        );
    }
    name
}

#[cfg(test)]
mod tests;
