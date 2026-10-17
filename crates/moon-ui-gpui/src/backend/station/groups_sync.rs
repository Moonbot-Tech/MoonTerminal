//! Compare station memberships and admit each automatic payload at most once per destination.

use std::collections::BTreeSet;

use moon_core::config::CoreGroup;
use moon_core::station_api::{Access, ListedCore};
use moon_remote::ssh::Target;

use super::{cores_sync::LocalCore, job::Job, uid_translation::UidTranslation};

/// Ordered group names and sorted memberships form a collision-free payload signature.
type Signature = Vec<(String, Vec<u64>)>;

/// Attempts survive failed jobs and later reads, so multiple terminals cannot fight in a loop.
#[derive(Default)]
pub(super) struct GroupsSync {
    attempted: BTreeSet<(String, Signature)>,
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
        if !self
            .attempted
            .insert((target.addr(), signature(&translated)))
        {
            return None;
        }
        Some(Job::Groups {
            target,
            groups: ours.to_vec(),
        })
    }
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
