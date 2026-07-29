//! The durable uid counter, constructible only by naming the optional durable-store floor.
//!
//! A core's uid keys its rows in `reports.sqlite` and `strategies.sqlite`, and those rows outlive
//! the core's deletion from the config. A counter that starts below a uid some store still
//! remembers therefore hands a NEW core a deleted one's trades, P&L and strategy versions.
//!
//! The observed floor has to be considered on every path that builds a config, and it originally
//! shipped applied on one of five. This type exists so that omission is a compile error: the field
//! is private and [`UidCounter::new`] is the only constructor, so a load path that never names a
//! floor cannot produce a counter at all.
//!
//! Four things must never be added here, because each one restores a way to obtain a counter
//! without naming a floor:
//!
//! - `Default` — `..Default::default()` names no floor. That is precisely the hole this type
//!   closes.
//! - `Serialize` / `Deserialize` — serde's derives construct field-wise, bypassing `new`
//!   entirely. `SettingsFile::next_uid` stays the only serde-visible representation.
//! - `From<u64>` or `Deref` — either turns a bare number back into a counter.
//! - `Copy` — a silent stale copy could be written back over a raised counter.
//!
//! `tests.rs` next to this file enforces those four and the field's privacy by reading this
//! source, the way `theme_contract` enforces the UI bans. A comment alone would not survive
//! a refactor.

/// Next uid to issue, raised clear of any representable floor supplied to its constructor.
///
/// Deliberately not "clear of every uid a store has recorded": the floor is best-effort. An
/// unreadable store contributes nothing and is indistinguishable from an absent one, so this
/// type guarantees the floor was NAMED, not that it was complete. See [`UidCounter::new`].
#[derive(Clone, Debug)]
pub struct UidCounter {
    /// Next free uid: the value about to be handed out, not the last one issued.
    next: u64,
}

impl UidCounter {
    /// Build a counter from the persisted value and an optional durable-store floor.
    ///
    /// The single place the floor is folded in, so a config load path that skips it is a visible
    /// omission rather than a silently different rule. The counter may only ever RISE: a store
    /// that reports a lower maximum — a truncated or partially synced replica — must not drag the
    /// mark back over uids already issued.
    ///
    /// A floor of `u64::MAX` has no representable successor and therefore violates the counter's
    /// next-free-uid invariant. `checked_add` detects that corrupt state; the counter is left
    /// alone and the damage is logged rather than wrapped around into reissuing a live uid.
    ///
    /// `uid_floor` is `None` when no durable store contributed a uid, whether because the stores
    /// were absent, empty, or unreadable.
    pub(super) fn new(persisted: u64, uid_floor: Option<u64>) -> Self {
        let Some(seen) = uid_floor else {
            return Self { next: persisted };
        };
        let next = match seen.checked_add(1) {
            Some(raised) => persisted.max(raised),
            None => {
                log::error!(
                    "uid floor: хранилище содержит uid без преемника ({seen}) — данные повреждены, \
                     счётчик uid не поднят"
                );
                persisted
            }
        };
        Self { next }
    }

    /// Hand out the next uid and advance.
    ///
    /// Issuance remains infallible so callers such as config saving do not gain a new error path.
    /// Under the shipped profiles, a persisted counter already at `u64::MAX` wraps to zero; the
    /// constructor guards an unrepresentable floor but cannot distinguish a corrupt persisted
    /// counter from an intentional value.
    pub(super) fn issue(&mut self) -> u64 {
        let uid = self.next;
        self.next += 1;
        uid
    }

    /// Raise the counter to at least `at_least`, never lowering it.
    ///
    /// Takes an already-computed bound rather than computing one, leaving validation and overflow
    /// handling for that bound with the caller.
    pub(super) fn raise_to(&mut self, at_least: u64) {
        self.next = self.next.max(at_least);
    }

    /// The value to persist in `SettingsFile::next_uid`.
    pub(super) fn get(&self) -> u64 {
        self.next
    }
}

#[cfg(test)]
mod tests;
