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
            // The Vec order IS the manual order — nothing to sort.
            CoreSortMode::Manual => {}
            // Lexicographic order of lowercase Unicode names, matching the group sort. Cache
            // each key on render; uid makes equal names independent of manual order.
            CoreSortMode::Name => {
                ordered.sort_by_cached_key(|s| (s.name.to_lowercase(), s.uid));
            }
            // Durable uid order is insertion order. Unsaved uid zero ranks newest, and id
            // breaks ties between multiple unsaved servers without relying on sort stability.
            CoreSortMode::Added => {
                ordered.sort_by_key(|s| (if s.uid == 0 { u64::MAX } else { s.uid }, s.id));
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
    /// Database-only cores have no config rank, so they receive distinct tail ranks in input
    /// order. Their order therefore does not depend on sort stability.
    pub(crate) fn from_db(&self, rows: Vec<(u64, String)>) -> OrderedCores {
        let known = self.rank.len() as u32;
        let mut unknown_seen = 0u32;
        let mut keyed: Vec<(u32, (CoreId, String))> = rows
            .into_iter()
            .map(|(id, name)| {
                let rank = match self.rank.get(&id) {
                    Some(rank) => *rank,
                    None => {
                        let rank = known.saturating_add(unknown_seen);
                        unknown_seen += 1;
                        rank
                    }
                };
                (rank, (id, name))
            })
            .collect();
        keyed.sort_by_key(|(rank, _)| *rank);
        OrderedCores(keyed.into_iter().map(|(_, core)| core).collect())
    }

    /// Sort any slice whose items carry a `CoreId` into canonical order.
    ///
    /// Use this for row shapes that cannot be represented as [`OrderedCores`].
    pub(crate) fn sort_by<T>(&self, rows: &mut [T], key: impl Fn(&T) -> CoreId) {
        rows.sort_by_key(|row| self.rank(key(row)));
    }
}

#[cfg(test)]
mod tests {
    //! Canonical-order edge-case tests.

    use super::*;
    use moon_core::config::Secret;
    use moon_core::config::{FeedFlags, ServerConfig};

    /// Build a server fixture with explicit runtime and durable ids.
    fn server(id: u64, uid: u64, name: &str) -> ServerConfig {
        ServerConfig {
            id,
            uid,
            name: name.to_string(),
            active: true,
            show_window: true,
            feed: FeedFlags::default(),
            key: Secret::default(),
            group: "default".to_string(),
            market: "Binance".to_string(),
            color: [0, 0, 0],
            synthetic: false,
            chart_bundle: String::new(),
            order_sizes: None,
            order_size_sel: None,
            default_alert_strategy: 0,
        }
    }

    /// Build an app config for one ordering mode.
    fn config(mode: CoreSortMode, servers: Vec<ServerConfig>) -> AppConfig {
        AppConfig {
            servers,
            core_sort: mode,
            ..Default::default()
        }
    }

    /// Cores that only exist in `reports.sqlite` (their server was deleted) must land AFTER
    /// every configured core and keep the order the query returned them in.
    ///
    /// Protects `CoreOrder::from_db`: assigning one shared rank to unknown cores lets an
    /// unstable sort reorder the historical tail.
    #[test]
    fn historical_cores_form_a_stable_tail_after_the_configured_ones() {
        let order = CoreOrder::new(&config(
            CoreSortMode::Manual,
            vec![server(1, 1, "Alpha"), server(2, 2, "Bravo")],
        ));
        let ordered = order.from_db(vec![
            (77, "Deleted-B".to_string()),
            (2, "Bravo".to_string()),
            (66, "Deleted-A".to_string()),
            (1, "Alpha".to_string()),
        ]);
        let names: Vec<&str> = ordered.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["Alpha", "Bravo", "Deleted-B", "Deleted-A"]);
    }

    /// A server added in the Settings draft has no uid yet. It must rank newest, not oldest —
    /// ranking `0` as a real uid would jump an unsaved row to the top of every list.
    #[test]
    fn an_unsaved_server_sorts_newest_in_added_mode() {
        let order = CoreOrder::new(&config(
            CoreSortMode::Added,
            vec![
                server(9, 0, "Unsaved"),
                server(2, 2, "Second"),
                server(1, 1, "First"),
            ],
        ));
        assert!(order.rank(1) < order.rank(2), "uid 1 predates uid 2");
        assert!(order.rank(2) < order.rank(9), "an unsaved server is newest");
    }

    /// Equal names use uid order so Name mode remains independent of manual server order.
    #[test]
    fn duplicate_names_rank_deterministically_by_uid() {
        let order = CoreOrder::new(&config(
            CoreSortMode::Name,
            vec![server(5, 5, "Same"), server(3, 3, "Same")],
        ));
        assert!(order.rank(3) < order.rank(5));
    }
}
