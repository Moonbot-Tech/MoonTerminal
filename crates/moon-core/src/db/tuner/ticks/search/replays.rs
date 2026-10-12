//! Folded point replays shared by the restarts of one search, keyed on built parameters.
//!
//! Refusals, risk checks and candidate offers stay with each request in the search. Only the
//! deterministic replay is reused; no deal, fill array or per-deal result is retained here.

use std::collections::{VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, PoisonError};

use super::closing::Replayed;
use super::{EntryParams, ExitParams};

/// Independent locks keep unrelated points from serializing the parallel restarts.
const SHARDS: usize = 16;
/// At most 1,024 folded scores across the shards, even for a one-strategy sample.
const KEPT_PER_SHARD: usize = 64;
/// At most 4,096 parameter pairs across the shards, regardless of the sample's base count.
const PARAMS_PER_SHARD: usize = 256;

/// A fingerprint narrows comparisons; the entire built key always decides a hit.
struct Saved {
    fingerprint: u64,
    params: Vec<(EntryParams, ExitParams)>,
    replayed: Replayed,
}

/// Recent replay results of ONE immutable training slice and its fixed base-to-deal mapping.
#[derive(Default)]
pub(super) struct ReplayCache {
    recent: [Mutex<VecDeque<Saved>>; SHARDS],
}

impl ReplayCache {
    /// Read a folded replay of `params`, or compute and retain it on a miss.
    ///
    /// Args:
    ///     params: Entry and exit parameters in the search's fixed base order.
    ///     replay: The unchanged chronological fold over this cache's training slice.
    ///
    /// Replaying outside the lock permits concurrent misses for the same point. Waiting for
    /// another Rayon job while holding a lock could deadlock its nested deal replay instead.
    pub(super) fn replay(
        &self,
        params: &[(EntryParams, ExitParams)],
        replay: impl FnOnce() -> Replayed,
    ) -> Replayed {
        let capacity = KEPT_PER_SHARD.min(PARAMS_PER_SHARD / params.len().max(1));
        if capacity == 0 {
            return replay();
        }
        let fingerprint = fingerprint(params);
        let shard = &self.recent[fingerprint as usize % SHARDS];
        let matches = |saved: &Saved| saved.fingerprint == fingerprint && saved.params == params;
        {
            let mut recent = shard.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(saved) = recent
                .iter()
                .position(matches)
                .and_then(|at| recent.remove(at))
            {
                let replayed = saved.replayed.clone();
                recent.push_front(saved);
                return replayed;
            }
        }
        let replayed = replay();
        let mut recent = shard.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = recent.iter().position(matches) {
            recent.remove(at);
        }
        recent.push_front(Saved {
            fingerprint,
            params: params.to_vec(),
            replayed: replayed.clone(),
        });
        recent.truncate(capacity);
        replayed
    }
}

/// Cheap routing from a few built fields; omitted fields are checked by full key equality.
/// Signed zero is normalized because the built structs compare it as equal.
fn fingerprint(params: &[(EntryParams, ExitParams)]) -> u64 {
    let mut hash = DefaultHasher::new();
    params.len().hash(&mut hash);
    let bits = |v: f64| if v == 0.0 { 0 } else { v.to_bits() };
    for (entry, exit) in params {
        match entry {
            EntryParams::Fact => 0u8.hash(&mut hash),
            EntryParams::MoonShot(entry) => {
                1u8.hash(&mut hash);
                bits(entry.price_pct).hash(&mut hash);
                bits(entry.price_min_pct).hash(&mut hash);
            }
        }
        bits(exit.sell_price_pct).hash(&mut hash);
        bits(exit.stop_loss_pct).hash(&mut hash);
        bits(exit.price_down_timer_s).hash(&mut hash);
    }
    hash.finish()
}

#[cfg(test)]
mod tests;
