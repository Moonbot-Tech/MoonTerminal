//! Compare station memberships and admit each automatic payload at most once per destination.

use std::collections::BTreeSet;

use moon_core::config::CoreGroup;
use moon_core::station_api::{Access, ListedCore};
use moon_remote::ssh::Target;
use moon_remote::station::bot::BotState;

use super::{cores_sync::LocalCore, job::Job, uid_translation::UidTranslation};

/// Ordered group names and sorted memberships form a collision-free payload signature.
type Signature = Vec<(String, Vec<u64>)>;

/// A signature belongs to one station address within the current station configuration.
type Attempt = (String, Signature);

/// Attempts survive failed jobs and later reads, so multiple terminals cannot fight in a loop.
#[derive(Default)]
pub(super) struct GroupsSync {
    attempted: BTreeSet<Attempt>,
    in_flight: Option<Attempt>,
}

impl GroupsSync {
    /// Defer while busy; admit only supported, complete, non-empty, changed payloads.
    pub(super) fn take_job(
        &mut self,
        running: bool,
        target: Option<Target>,
        access: Option<&Access>,
        ours: &[CoreGroup],
        here: &[LocalCore],
        listing: &[ListedCore],
    ) -> Option<Job> {
        if running || ours.is_empty() {
            return None;
        }
        let target = target?;
        let access = access?;
        access.groups.as_ref()?;
        let (translated, unmapped) = translated_groups(ours, here, listing);
        if unmapped > 0 || same_groups(&translated, &access.report_groups()) {
            return None;
        }
        let attempt = (target.addr(), signature(&translated));
        if !self.attempted.insert(attempt.clone()) {
            return None;
        }
        self.in_flight = Some(attempt);
        Some(Job::Groups {
            target,
            groups: ours.to_vec(),
        })
    }

    /// A fresh unmapped listing is a deferral, not a failed write; release only that admission.
    pub(super) fn translation_seen(&mut self, unmapped: usize) {
        if let Some(attempt) = self.in_flight.take()
            && unmapped > 0
        {
            self.attempted.remove(&attempt);
        }
    }
}

/// Automatic writes need the latest status, never the listing retained through a restart gap.
pub(super) fn listing(bot: Option<&BotState>) -> &[ListedCore] {
    bot.and_then(|bot| bot.station.as_ref())
        .and_then(|status| status.cores.as_deref())
        .unwrap_or_default()
}

/// Translate exactly as the manual payload does, retaining the omission count for admission.
pub(crate) fn translated_groups(
    ours: &[CoreGroup],
    here: &[LocalCore],
    listing: &[ListedCore],
) -> (Vec<CoreGroup>, usize) {
    let mut groups = ours.to_vec();
    let mut translation = UidTranslation::new(here, listing);
    translation.groups_to_station(&mut groups);
    (groups, translation.unmapped())
}

/// Compare in the station namespace; unresolved members keep the manual action available.
pub(crate) fn same_station_groups(
    ours: &[CoreGroup],
    theirs: &[CoreGroup],
    here: &[LocalCore],
    listing: &[ListedCore],
) -> bool {
    let (translated, unmapped) = translated_groups(ours, here, listing);
    unmapped == 0 && same_groups(&translated, theirs)
}

/// Group positions matter; member ordering does not affect the displayed memberships.
pub(crate) fn same_groups(a: &[CoreGroup], b: &[CoreGroup]) -> bool {
    signature(a) == signature(b)
}

/// Retain group order while normalizing only member order.
fn signature(groups: &[CoreGroup]) -> Signature {
    groups
        .iter()
        .map(|group| {
            let mut cores = group.cores.clone();
            cores.sort_unstable();
            (group.name.clone(), cores)
        })
        .collect()
}

#[cfg(test)]
mod tests;
