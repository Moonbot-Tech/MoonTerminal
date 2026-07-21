//! Canonical ordering for core (server) lists.
//!
//! [`CoreOrder`] ranks lists from the current [`CoreSortMode`] at render time. Pair lists can
//! carry the privately constructed [`OrderedCores`] marker; arbitrary row shapes use
//! [`CoreOrder::sort_by`].

use std::collections::HashMap;

use moon_core::config::{AppConfig, CoreSortMode};
use moon_core::session::CoreId;

/// Cores in canonical order, paired with the name to display.
///
/// Its private field prevents callers from presenting an unranked pair list as canonical.
pub(crate) struct OrderedCores(Vec<(CoreId, String)>);

// Expose the slice API while keeping construction inside this module.
impl std::ops::Deref for OrderedCores {
    type Target = [(CoreId, String)];

    /// Expose the ordered pairs as a slice without exposing construction.
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoIterator for OrderedCores {
    type Item = (CoreId, String);
    type IntoIter = std::vec::IntoIter<(CoreId, String)>;

    /// Consume the canonical list in order.
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Insertion-order key, shared by both `Added*` modes so they cannot drift apart.
///
/// A server added in the Settings draft has no uid yet, so it ranks as the newest possible one —
/// last in oldest-first and first in newest-first, which is what both modes promise. `id` breaks
/// ties between several unsaved rows, which makes the key INJECTIVE: distinct servers always get
/// distinct keys, so `Reverse` produces an exact mirror rather than something that depends on
/// whether the sort happens to be stable.
fn insertion_key(s: &moon_core::config::ServerConfig) -> (u64, u64) {
    (if s.uid == 0 { u64::MAX } else { s.uid }, s.id)
}

/// A rank table built from the current config for one render pass.
///
/// Rebuilding is cheap for these short lists and prevents stale order.
pub(crate) struct CoreOrder {
    rank: HashMap<CoreId, u32>,
}

impl CoreOrder {
    /// Rank every configured core according to the user's chosen sort mode.
    ///
    /// Inactive cores are ranked too: they hold their place so that switching one off and
    /// back on returns it to the same position instead of the end of the list.
    pub(crate) fn new(cfg: &AppConfig) -> Self {
        let mut ordered: Vec<&moon_core::config::ServerConfig> = cfg.servers.iter().collect();
        match cfg.core_sort {
            // Lexicographic order of lowercase Unicode names, matching the group sort. Cache
            // each key on render; uid makes equal names independent of the servers Vec order.
            CoreSortMode::Name => {
                ordered.sort_by_cached_key(|s| (s.name.to_lowercase(), s.uid));
            }
            CoreSortMode::AddedOldest => ordered.sort_by_key(|s| insertion_key(s)),
            // Reversing is well-defined WITHOUT relying on sort stability because
            // `insertion_key` is injective — see its doc comment.
            CoreSortMode::AddedNewest => {
                ordered.sort_by_key(|s| std::cmp::Reverse(insertion_key(s)))
            }
        }
        let rank = ordered
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i as u32))
            .collect();
        Self { rank }
    }

    /// Rank of one core; cores the config does not know rank last.
    ///
    /// Private so callers order through this module rather than duplicating rank logic.
    fn rank(&self, id: CoreId) -> u32 {
        self.rank.get(&id).copied().unwrap_or(u32::MAX)
    }

    /// Order the live sessions kept by `keep` (group / scope filter).
    ///
    /// The predicate controls membership; this method only canonicalizes the retained rows.
    pub(crate) fn from_sessions<F>(
        &self,
        sessions: &[moon_core::session::CoreSession],
        keep: F,
    ) -> OrderedCores
    where
        F: Fn(&moon_core::session::CoreSession) -> bool,
    {
        let mut cores: Vec<(CoreId, String)> = sessions
            .iter()
            .filter(|s| keep(s))
            .map(|s| (s.id, s.name.clone()))
            .collect();
        cores.sort_by_key(|(id, _)| self.rank(*id));
        OrderedCores(cores)
    }

    /// Order rows that came from the reports database, whose names are the DB's own.
    ///
    /// Database-only cores whose server is absent from the current config share the `u32::MAX`
    /// rank `rank` hands out for anything unknown, so they land after every configured core and
    /// keep the query order — `sort_by_key` is a STABLE sort.
    pub(crate) fn from_db(&self, mut rows: Vec<(CoreId, String)>) -> OrderedCores {
        rows.sort_by_key(|(id, _)| self.rank(*id));
        OrderedCores(rows)
    }

    /// Sort any slice whose items carry a `CoreId` into canonical order.
    ///
    /// Use this for row shapes that cannot be represented as [`OrderedCores`].
    pub(crate) fn sort_by<T>(&self, rows: &mut [T], key: impl Fn(&T) -> CoreId) {
        rows.sort_by_key(|row| self.rank(key(row)));
    }
}

#[cfg(test)]
mod tests;
