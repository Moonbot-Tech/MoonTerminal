//! What a reload of the axis would read again, folded to one number.
//!
//! The report's generation moves on every live row update — an open position's price, a fee
//! booked — while the closed deals the axis replays stay what they were. A reload asked for by
//! such a move would read the same deals, publish the same rows and replay nothing new. The
//! closed deals' signature lets it see that before stages B and C: every field the report gives
//! a deal — its identity `(core_uid, report_uid)`, stamps, prices, the entry order's creation,
//! kind, coin, side, sell reason, deltas, a hook's stated take, a strike's detect, its money —
//! goes into it, so a same-uid correction of any model input or of the fact changes it. The
//! order the rows come in does not.
//!
//! Hashed through `Debug`, which prints every field: a field added to `Deal` joins the
//! signature without anyone remembering to list it here.

use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

use moon_core::db::tuner::ticks::Deal;

#[cfg(test)]
mod tests;

/// `fmt::Write` straight into a hasher: no string per deal.
struct HashWriter<'a>(&'a mut DefaultHasher);

impl std::fmt::Write for HashWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

/// Fold one value's `Debug` print into `hasher`.
pub(in crate::analytics::tuner) fn hash_debug(
    hasher: &mut DefaultHasher,
    value: &impl std::fmt::Debug,
) {
    // Writing into a hasher cannot fail.
    let _ = write!(HashWriter(hasher), "{value:?}");
    // A separator, so two adjacent values never print into a third's.
    hasher.write_u8(0);
}

/// The signature of the closed deals among `deals`, in any order — usable per strategy group
/// as well as for a whole scope. A deal without a close stamp is open and left out.
pub(in crate::analytics::tuner) fn signature_of<'a>(
    deals: impl IntoIterator<Item = &'a Deal>,
) -> u64 {
    let mut closed: Vec<&Deal> = deals.into_iter().filter(|d| d.close_ms > 0).collect();
    closed.sort_by_key(|d| (d.core_uid, d.report_uid));
    let mut hasher = DefaultHasher::new();
    closed.len().hash(&mut hasher);
    for deal in closed {
        hash_debug(&mut hasher, deal);
    }
    hasher.finish()
}

/// The settings a row's held read and its "missing" verdict were made under, beside the model's:
/// the shortest tail (the asked tail and the short-tail floor), the model's margin and the
/// long-position split (the window asked for).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) struct SamplePolicy {
    pub(in crate::analytics::tuner) min_tail_s: u32,
    pub(in crate::analytics::tuner) margin_ms: i64,
    pub(in crate::analytics::tuner) long_position_ms: i64,
}

impl SamplePolicy {
    /// The policy in force now.
    pub(in crate::analytics::tuner) fn current() -> Self {
        Self {
            min_tail_s: super::tail::effective_s(),
            margin_ms: moon_core::market::trade_replay::margin_ms(),
            long_position_ms: moon_core::market::trade_replay::long_position_ms(),
        }
    }
}

/// Everything a reload of the same scope would publish and replay from: the scope's signature
/// (its closed deals, the counts and baseline beside them, the strategies' values and spans),
/// its period, the model and sample policy, the station's refusals, the tape store's filing
/// revision and where each deal's prints live. Two equal keys publish the same rows.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics::tuner) struct ReloadKey {
    pub(in crate::analytics::tuner) scope: u64,
    pub(in crate::analytics::tuner) period: (i64, i64),
    pub(in crate::analytics::tuner) model: moon_core::db::tuner::ticks::ModelSettings,
    pub(in crate::analytics::tuner) sample: SamplePolicy,
    pub(in crate::analytics::tuner) refusal_revision: u64,
    pub(in crate::analytics::tuner) tape_rev: u64,
    pub(in crate::analytics::tuner) addresses: u64,
}

/// A background skip decision stays valid only for the same publication and clock interval.
pub(super) struct SkipCheck {
    pub(super) key: ReloadKey,
    pub(super) rows_rev: u64,
    pub(super) checked_at_ms: i64,
    pub(super) until_ms: i64,
}

impl SkipCheck {
    /// Validate the background pass without walking the rows again on the UI thread.
    pub(super) fn holds(&self, key: &ReloadKey, rows_rev: u64, now_ms: i64) -> bool {
        self.key == *key
            && self.rows_rev == rows_rev
            && self.checked_at_ms <= now_ms
            && now_ms < self.until_ms
    }
}

/// Whether a report-driven reload may keep the published rows as they stand: the last load
/// settled under a key equal to this one, the clock has not reached `valid_until_ms` — the
/// first instant a covered row's sample bucket moves ([`settled_until`]) — every missing row
/// still reads as it did when judged (`missing_holds`: the retention edge and the short-tail
/// admission, asked again at `now_ms`), and no row waits for a publication deadline that has
/// passed (`refiled.rs` re-reads those).
pub(in crate::analytics::tuner) fn can_skip<'a>(
    stored: Option<&ReloadKey>,
    now: &ReloadKey,
    rows: impl IntoIterator<Item = &'a super::state::DealRow> + Clone,
    now_ms: i64,
    valid_until_ms: i64,
    missing_holds: impl Fn(&super::state::DealRow) -> bool,
) -> bool {
    use moon_core::market::trade_replay::TickStatus;
    stored == Some(now)
        && now_ms < valid_until_ms
        && rows
            .clone()
            .into_iter()
            .filter(|r| r.tape == super::state::TapeStatus::Missing)
            .all(missing_holds)
        && !rows.into_iter().any(|r| {
            matches!(r.tape,
                super::state::TapeStatus::Refused(TickStatus::ArchivePending { retry_at_ms })
                    if retry_at_ms <= now_ms)
        })
}

/// The first instant after `now_ms` at which a row's verdict can move with the clock alone —
/// a retention edge of a covered short tail or of a missing row
/// ([`super::rows::next_verdict_flip`]); `i64::MAX` when none can.
pub(in crate::analytics::tuner) fn settled_until<'a>(
    rows: impl IntoIterator<Item = &'a super::state::DealRow>,
    min_tail_s: u32,
    now_ms: i64,
) -> i64 {
    rows.into_iter()
        .filter_map(|row| super::rows::next_verdict_flip(row, min_tail_s, now_ms))
        .min()
        .unwrap_or(i64::MAX)
}

/// The resolved addresses, folded in key order.
pub(in crate::analytics::tuner) fn addresses_signature(
    addresses: &std::collections::HashMap<
        (u64, String),
        Option<std::sync::Arc<super::state::RowAddress>>,
    >,
) -> u64 {
    let mut keys: Vec<_> = addresses.iter().collect();
    keys.sort_by(|a, b| a.0.cmp(b.0));
    let mut hasher = DefaultHasher::new();
    for (key, address) in keys {
        hash_debug(&mut hasher, &(key, address));
    }
    hasher.finish()
}

/// Fold keyed value maps into `hasher` in key order, each map's entries sorted too.
pub(in crate::analytics::tuner) fn hash_sorted_maps<'a, K: Ord + std::fmt::Debug>(
    hasher: &mut DefaultHasher,
    entries: impl IntoIterator<
        Item = (
            K,
            &'a std::sync::Arc<std::collections::HashMap<String, String>>,
        ),
    >,
) {
    hash_sorted(
        hasher,
        entries.into_iter().map(|(key, values)| {
            let mut values: Vec<_> = values.iter().collect();
            values.sort();
            (key, values)
        }),
    );
}

/// Fold `entries` into `hasher` in key order.
pub(in crate::analytics::tuner) fn hash_sorted<K: Ord + std::fmt::Debug, V: std::fmt::Debug>(
    hasher: &mut DefaultHasher,
    entries: impl IntoIterator<Item = (K, V)>,
) {
    let mut entries: Vec<(K, V)> = entries.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for entry in &entries {
        hash_debug(hasher, entry);
    }
}
