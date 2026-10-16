//! Translate payload memberships only through uniquely address-matched core identities.

use std::collections::BTreeSet;

use moon_core::config::CoreGroup;
use moon_core::station_api::{Access, ListedCore};

use super::cores_sync::{LocalCore, station_uid_for};

/// A payload's proven identity pairs and distinct source cores omitted from its memberships.
pub(crate) struct UidTranslation {
    pairs: Vec<(u64, u64)>,
    missing: BTreeSet<u64>,
}

impl UidTranslation {
    /// Derive the same unique address matches used by station history requests.
    /// An absent or unsupported listing is an empty mapping, never a numeric fallback.
    pub(crate) fn new(here: &[LocalCore], listing: &[ListedCore]) -> Self {
        Self {
            pairs: here
                .iter()
                .filter_map(|core| {
                    station_uid_for(core.uid, here, listing).map(|uid| (core.uid, uid))
                })
                .collect(),
            missing: BTreeSet::new(),
        }
    }

    /// Replace terminal memberships with station identities, preserving group names and order.
    pub(crate) fn groups_to_station(&mut self, groups: &mut [CoreGroup]) {
        for group in groups {
            group.cores = self.uids(&group.cores, false);
        }
    }

    /// Preserve paired chats and roles while translating every explicit viewer grant.
    /// Empty translated lists continue to grant no data; they never become owner access.
    pub(crate) fn access(&mut self, access: &mut Access, to_terminal: bool) {
        for chat in &mut access.chat_access {
            chat.core_uids = self.uids(&chat.core_uids, to_terminal);
        }
    }

    /// Translate one list in the requested direction, dropping unknown or ambiguous identities.
    fn uids(&mut self, uids: &[u64], reverse: bool) -> Vec<u64> {
        uids.iter()
            .filter_map(|uid| {
                let mapped = self.pairs.iter().find_map(|(local, remote)| {
                    let (source, destination) = if reverse {
                        (remote, local)
                    } else {
                        (local, remote)
                    };
                    (source == uid).then_some(*destination)
                });
                if mapped.is_none() {
                    self.missing.insert(*uid);
                }
                mapped
            })
            .collect()
    }

    /// Count distinct omitted source identities across every translated list in the payload.
    pub(crate) fn unmapped(&self) -> usize {
        self.missing.len()
    }
}

#[cfg(test)]
mod tests;
