//! Folded point replays shared by the restarts of one search, keyed on built parameters.
//!
//! Refusals, risk checks and candidate offers stay with each request in the search. Only the
//! deterministic replay is reused; no deal, fill array or per-deal result is retained here.

use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::Hasher;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::closing::Replayed;
use super::{EntryParams, ExitParams};

/// Independent locks keep unrelated points from serializing the parallel restarts.
const SHARDS: usize = 16;
/// The memory every live search's folded replays may take together: enough to keep every
/// distinct point a long search of a large sample scores, so none is replayed twice, and one
/// budget however many searches run at once.
pub(super) const REPLAY_CACHE_BYTES: usize = 512 << 20;

/// A memory budget shared by the caches drawing on it, and what they hold of it now.
pub(super) struct Budget {
    bytes: usize,
    held: AtomicUsize,
}

impl Budget {
    pub(super) const fn new(bytes: usize) -> Self {
        Self {
            bytes,
            held: AtomicUsize::new(0),
        }
    }

    /// What the caches drawing on it hold now.
    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.held.load(Ordering::Relaxed)
    }
}

/// The process's: every search's cache draws on it.
static PROCESS_BUDGET: Budget = Budget::new(REPLAY_CACHE_BYTES);

/// One kept replay: the whole built key decides a hit; `used` orders the eviction.
struct Saved {
    params: Vec<(EntryParams, ExitParams)>,
    replayed: Replayed,
    used: u64,
    /// What it weighs against the budget.
    bytes: usize,
}

/// One shard: the kept replays by key hash, what they weigh, and the use clock.
#[derive(Default)]
struct Shard {
    saved: HashMap<u64, Vec<Saved>>,
    bytes: usize,
    clock: u64,
}

impl Shard {
    /// Let go of the shard's least recently used replays until `budget` holds at most `bytes`
    /// or the shard is empty, giving each one's bytes back.
    fn evict_to(&mut self, budget: &Budget, bytes: usize) {
        let mut stamps: Vec<(u64, u64)> = self
            .saved
            .iter()
            .flat_map(|(hash, kept)| kept.iter().map(move |s| (s.used, *hash)))
            .collect();
        stamps.sort_unstable();
        for (used, hash) in stamps {
            if budget.held.load(Ordering::Relaxed) <= bytes {
                break;
            }
            let Some(kept) = self.saved.get_mut(&hash) else {
                continue;
            };
            if let Some(at) = kept.iter().position(|s| s.used == used) {
                let gone = kept.swap_remove(at);
                self.bytes -= gone.bytes;
                budget.held.fetch_sub(gone.bytes, Ordering::Relaxed);
            }
            if kept.is_empty() {
                self.saved.remove(&hash);
            }
        }
    }
}

/// Recent replay results of ONE immutable training slice and its fixed base-to-deal mapping.
pub(super) struct ReplayCache {
    shards: [Mutex<Shard>; SHARDS],
    /// The budget it shares with every other live cache.
    budget: &'static Budget,
}

impl Default for ReplayCache {
    fn default() -> Self {
        Self::within(&PROCESS_BUDGET)
    }
}

impl Drop for ReplayCache {
    fn drop(&mut self) {
        let held: usize = self
            .shards
            .iter_mut()
            .map(|shard| {
                shard
                    .get_mut()
                    .unwrap_or_else(PoisonError::into_inner)
                    .bytes
            })
            .sum();
        self.budget.held.fetch_sub(held, Ordering::Relaxed);
    }
}

impl ReplayCache {
    /// A cache drawing on `budget`.
    pub(super) fn within(budget: &'static Budget) -> Self {
        Self {
            shards: Default::default(),
            budget,
        }
    }

    /// What one kept replay of `params` weighs, estimated off its key's length.
    fn entry_bytes(params: &[(EntryParams, ExitParams)]) -> usize {
        std::mem::size_of::<Saved>()
            + std::mem::size_of_val(params)
            + std::mem::size_of::<(u64, Vec<Saved>)>()
    }

    /// Read a folded replay of `params`, or compute and retain it on a miss.
    ///
    /// Args:
    ///     params: Entry and exit parameters in the search's fixed base order.
    ///     replay: The unchanged chronological fold over this cache's training slice.
    ///
    /// Replaying outside the lock permits concurrent misses for the same point. Waiting for
    /// another Rayon job while holding a lock could deadlock its nested deal replay instead.
    /// A miss that takes the budget over evicts this cache's own oldest replays of the shard
    /// it lands in, the new one last.
    pub(super) fn replay(
        &self,
        params: &[(EntryParams, ExitParams)],
        replay: impl FnOnce() -> Replayed,
    ) -> Replayed {
        let entry_bytes = Self::entry_bytes(params);
        if entry_bytes > self.budget.bytes / SHARDS {
            PROFILE.count(&PROFILE.uncached);
            return PROFILE.timed_replay(replay);
        }
        let hash = key_hash(params);
        let shard = &self.shards[hash as usize % SHARDS];
        {
            let mut shard = shard.lock().unwrap_or_else(PoisonError::into_inner);
            shard.clock += 1;
            let clock = shard.clock;
            if let Some(saved) = shard
                .saved
                .get_mut(&hash)
                .and_then(|kept| kept.iter_mut().find(|s| s.params == params))
            {
                saved.used = clock;
                PROFILE.count(&PROFILE.hits);
                return saved.replayed.clone();
            }
        }
        PROFILE.count(&PROFILE.misses);
        let replayed = PROFILE.timed_replay(replay);
        let mut shard = shard.lock().unwrap_or_else(PoisonError::into_inner);
        shard.clock += 1;
        let clock = shard.clock;
        let kept = shard.saved.entry(hash).or_default();
        // Another restart replayed the same point meanwhile: one copy is kept.
        if let Some(saved) = kept.iter_mut().find(|s| s.params == params) {
            saved.used = clock;
            return replayed;
        }
        kept.push(Saved {
            params: params.to_vec(),
            replayed: replayed.clone(),
            used: clock,
            bytes: entry_bytes,
        });
        shard.bytes += entry_bytes;
        let held = self.budget.held.fetch_add(entry_bytes, Ordering::Relaxed) + entry_bytes;
        if held > self.budget.bytes {
            // An eighth at a time, so a full budget does not sort a shard on every miss.
            shard.evict_to(self.budget, self.budget.bytes - self.budget.bytes / 8);
        }
        replayed
    }
}

/// Where every search of the process spent its point requests, summed over threads - for a
/// bench, which turns it on ([`SearchProfile::enable_timing`]); off, a search pays neither a
/// clock read nor a shared counter per point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchProfile {
    /// Requests answered from [`ReplayCache`].
    pub replay_hits: u64,
    /// Requests the cache replayed and kept.
    pub replay_misses: u64,
    /// Requests too wide for the cache, replayed every time.
    pub replay_uncached: u64,
    /// Time inside the folded replays of misses and uncached requests.
    pub replay_nanos: u64,
    /// Time reading each request's entry fills, cached or replayed (`fills`).
    pub fill_nanos: u64,
}

impl SearchProfile {
    /// Count and time the requests from now on - for a bench.
    pub fn enable_timing() {
        TIMING.store(true, Ordering::Relaxed);
    }

    /// The counters now.
    pub fn now() -> Self {
        PROFILE.snapshot()
    }

    /// What happened between `earlier` and `self`.
    pub fn since(&self, earlier: &Self) -> Self {
        Self {
            replay_hits: self.replay_hits - earlier.replay_hits,
            replay_misses: self.replay_misses - earlier.replay_misses,
            replay_uncached: self.replay_uncached - earlier.replay_uncached,
            replay_nanos: self.replay_nanos - earlier.replay_nanos,
            fill_nanos: self.fill_nanos - earlier.fill_nanos,
        }
    }
}

/// The process-wide counters behind [`SearchProfile`].
pub(super) struct Profile {
    hits: AtomicU64,
    misses: AtomicU64,
    uncached: AtomicU64,
    replay_nanos: AtomicU64,
    fill_nanos: AtomicU64,
}

pub(super) static PROFILE: Profile = Profile {
    hits: AtomicU64::new(0),
    misses: AtomicU64::new(0),
    uncached: AtomicU64::new(0),
    replay_nanos: AtomicU64::new(0),
    fill_nanos: AtomicU64::new(0),
};

fn nanos(took: Duration) -> u64 {
    u64::try_from(took.as_nanos()).unwrap_or(u64::MAX)
}

/// Whether the profile counts and times: off unless a bench turns it on
/// ([`SearchProfile::enable_timing`]), so a search pays no clock read or shared counter per
/// point.
static TIMING: AtomicBool = AtomicBool::new(false);

impl Profile {
    /// One request more on `counter`, when the profile is on.
    fn count(&self, counter: &AtomicU64) {
        if TIMING.load(Ordering::Relaxed) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn timed_replay(&self, replay: impl FnOnce() -> Replayed) -> Replayed {
        if !TIMING.load(Ordering::Relaxed) {
            return replay();
        }
        let started = Instant::now();
        let out = replay();
        self.replay_nanos
            .fetch_add(nanos(started.elapsed()), Ordering::Relaxed);
        out
    }

    /// Run a fill read, adding its time to the profile.
    pub(super) fn timed_fills<R>(&self, read: impl FnOnce() -> R) -> R {
        if !TIMING.load(Ordering::Relaxed) {
            return read();
        }
        let started = Instant::now();
        let out = read();
        self.fill_nanos
            .fetch_add(nanos(started.elapsed()), Ordering::Relaxed);
        out
    }

    fn snapshot(&self) -> SearchProfile {
        SearchProfile {
            replay_hits: self.hits.load(Ordering::Relaxed),
            replay_misses: self.misses.load(Ordering::Relaxed),
            replay_uncached: self.uncached.load(Ordering::Relaxed),
            replay_nanos: self.replay_nanos.load(Ordering::Relaxed),
            fill_nanos: self.fill_nanos.load(Ordering::Relaxed),
        }
    }
}

/// The hash of a whole built key, its debug spelling streamed into the hasher with no string
/// built: every field takes part, so points that differ only in fields a narrower hash skipped
/// do not pile into one bucket. Minus signs are left out, so `-0.0` and `0.0` (equal keys)
/// hash alike; a point and its sign-flipped twin share a bucket, and equality tells them apart.
fn key_hash(params: &[(EntryParams, ExitParams)]) -> u64 {
    struct Unsigned(DefaultHasher);
    impl std::fmt::Write for Unsigned {
        fn write_str(&mut self, part: &str) -> std::fmt::Result {
            for piece in part.split('-') {
                self.0.write(piece.as_bytes());
            }
            Ok(())
        }
    }
    let mut hash = Unsigned(DefaultHasher::new());
    let _ = std::fmt::Write::write_fmt(&mut hash, format_args!("{params:?}"));
    hash.0.finish()
}

#[cfg(test)]
mod tests;
