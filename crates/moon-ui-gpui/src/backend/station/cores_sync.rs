//! Address-based reconciliation keeps station identities independent of terminal identities.

use moon_core::config::AppConfig;
use moon_core::station_api::{ListedCore, core_address, key_fingerprint};

/// An eligible terminal core without its secret key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalCore {
    pub(crate) uid: u64,
    pub(crate) name: String,
    pub(crate) address: Option<String>,
    pub(crate) key_fp: String,
}

/// The action needed after matching by address, with key changes taking precedence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowState {
    Same,
    OnlyHere,
    OnlyOnStation,
    NameDiffers,
    KeyDiffers,
}

/// One comparison row, ordered by station uid before terminal-only rows.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub(crate) state: RowState,
    pub(crate) name: String,
    pub(crate) station_name: Option<String>,
    pub(crate) address: Option<String>,
    pub(crate) terminal_uid: Option<u64>,
    pub(crate) station_uid: Option<u64>,
}

/// A selected local credential and its destination identity; adds must not overwrite credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Upsert {
    pub(crate) terminal_uid: u64,
    pub(crate) station_uid: u64,
    pub(crate) add: bool,
}

/// Summary for the comparison header and bulk action.
#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) on_station: usize,
    pub(crate) here: usize,
    pub(crate) same: usize,
    pub(crate) only_here: usize,
    pub(crate) differs: usize,
    pub(crate) only_station: usize,
    pub(crate) pushable: usize,
}

impl Counts {
    /// Count comparison states without treating station-only cores as pushable.
    pub(crate) fn from_rows(rows: &[Row]) -> Self {
        let mut counts = Self::default();
        for row in rows {
            counts.on_station += usize::from(row.state != RowState::OnlyHere);
            counts.here += usize::from(row.terminal_uid.is_some());
            match row.state {
                RowState::Same => counts.same += 1,
                RowState::OnlyHere => counts.only_here += 1,
                RowState::OnlyOnStation => counts.only_station += 1,
                RowState::NameDiffers | RowState::KeyDiffers => counts.differs += 1,
            }
        }
        counts.pushable = bulk(rows).len();
        counts
    }
}

/// Include only active real cores with credentials; no key is retained in comparison state.
pub(crate) fn local_cores(cfg: &AppConfig) -> Vec<LocalCore> {
    cfg.servers
        .iter()
        .filter(|s| eligible(s.active, s.synthetic, &s.key))
        .map(|s| local_core(s.uid, &s.name, &s.key))
        .collect()
}

/// Credentials without a real enabled core are never eligible for an explicit push.
pub(crate) fn eligible(active: bool, synthetic: bool, key: &moon_core::config::Secret) -> bool {
    active && !synthetic && !key.is_empty()
}

/// Derive the same secret-free identity for a saved key and a live config entry.
pub(crate) fn local_core(uid: u64, name: &str, key: &moon_core::config::Secret) -> LocalCore {
    LocalCore {
        uid,
        name: name.to_owned(),
        address: core_address(key.expose()),
        key_fp: key_fingerprint(key.expose()),
    }
}

/// History remains eligible after a real core is disabled locally.
pub(crate) fn trace_cores(cfg: &AppConfig) -> Vec<LocalCore> {
    cfg.servers
        .iter()
        .filter(|s| eligible(true, s.synthetic, &s.key))
        .map(|s| local_core(s.uid, &s.name, &s.key))
        .collect()
}

/// Match duplicate addresses by ascending uid, consuming each local core at most once.
/// Unmatched terminal cores receive proposed uids above both sets and the retirement floor.
/// An exhausted TOML uid range leaves their station uid absent, so no add can be selected.
pub(crate) fn reconcile(
    here: &[LocalCore],
    station: &[ListedCore],
    high_water: Option<u64>,
) -> Vec<Row> {
    let mut here: Vec<_> = here.iter().collect();
    here.sort_by_key(|core| core.uid);
    let mut station: Vec<_> = station.iter().collect();
    station.sort_by_key(|core| core.uid);
    let mut matched = vec![false; here.len()];
    let mut largest = here
        .iter()
        .map(|core| core.uid)
        .chain(station.iter().map(|core| core.uid))
        .max()
        .unwrap_or(0)
        .max(high_water.unwrap_or(0));
    let mut rows = Vec::new();
    for remote in station {
        let local = remote.address.as_ref().and_then(|address| {
            here.iter()
                .enumerate()
                .find(|(index, core)| !matched[*index] && core.address.as_ref() == Some(address))
        });
        if let Some((index, local)) = local {
            matched[index] = true;
            rows.push(Row {
                state: if remote.key_fp.as_ref() != Some(&local.key_fp) {
                    RowState::KeyDiffers
                } else if remote.name != local.name {
                    RowState::NameDiffers
                } else {
                    RowState::Same
                },
                name: local.name.clone(),
                station_name: Some(remote.name.clone()),
                address: local.address.clone(),
                terminal_uid: Some(local.uid),
                station_uid: Some(remote.uid),
            });
        } else {
            rows.push(Row {
                state: RowState::OnlyOnStation,
                name: remote.name.clone(),
                station_name: Some(remote.name.clone()),
                address: remote.address.clone(),
                terminal_uid: None,
                station_uid: Some(remote.uid),
            });
        }
    }
    for (index, local) in here.into_iter().enumerate() {
        if matched[index] {
            continue;
        }
        let uid = largest
            .checked_add(1)
            .filter(|uid| *uid <= i64::MAX as u64)
            .inspect(|uid| largest = *uid);
        rows.push(Row {
            state: RowState::OnlyHere,
            name: local.name.clone(),
            station_name: None,
            address: local.address.clone(),
            terminal_uid: Some(local.uid),
            station_uid: uid,
        });
    }
    rows
}

/// Select adds and name/key changes without ever removing a station-only core.
pub(crate) fn bulk(rows: &[Row]) -> Vec<Upsert> {
    rows.iter()
        .filter_map(|row| match row.state {
            RowState::OnlyHere | RowState::NameDiffers | RowState::KeyDiffers => Some(Upsert {
                terminal_uid: row.terminal_uid?,
                station_uid: row.station_uid?,
                add: row.state == RowState::OnlyHere,
            }),
            RowState::Same | RowState::OnlyOnStation => None,
        })
        .collect()
}

/// Translate traces only when both address namespaces have exactly one matching identity.
pub(crate) fn station_uid_for(
    local_uid: u64,
    here: &[LocalCore],
    listing: &[ListedCore],
) -> Option<u64> {
    let address = here
        .iter()
        .find(|core| core.uid == local_uid)?
        .address
        .as_ref()?;
    if here
        .iter()
        .filter(|core| core.address.as_ref() == Some(address))
        .count()
        != 1
    {
        return None;
    }
    let mut matches = listing
        .iter()
        .filter(|core| core.address.as_ref() == Some(address));
    let uid = matches.next()?.uid;
    matches.next().is_none().then_some(uid)
}

/// An unread listing never authorizes a request; only a confirmed old station uses local uids.
pub(crate) fn trace_uid(
    local_uid: u64,
    here: &[LocalCore],
    seen: Option<Option<&[ListedCore]>>,
) -> Option<u64> {
    match seen {
        None => None,
        Some(None) => Some(local_uid),
        Some(Some(listing)) => station_uid_for(local_uid, here, listing),
    }
}

/// Revalidate selected operations and existing destinations against a fresh comparison.
/// Omit updates already applied, refresh proposed add uids, and return `None` for stale selections.
pub(crate) fn selected_changes(wanted: &[Upsert], fresh: &[Row]) -> Option<Vec<Upsert>> {
    let changes = bulk(fresh);
    wanted
        .iter()
        .filter_map(|wanted| {
            let row = fresh
                .iter()
                .find(|row| row.terminal_uid == Some(wanted.terminal_uid));
            if row.is_some_and(|row| {
                row.state == RowState::Same
                    && !wanted.add
                    && row.station_uid == Some(wanted.station_uid)
            }) {
                return None;
            }
            Some(
                changes
                    .iter()
                    .find(|change| change.terminal_uid == wanted.terminal_uid)
                    .filter(|change| {
                        change.add == wanted.add
                            && (change.add || change.station_uid == wanted.station_uid)
                    })
                    .cloned(),
            )
        })
        .collect()
}

/// The station-only identity at click time must still have the same name and address.
pub(crate) fn removal_matches(
    uids: &[u64],
    names: &[String],
    addresses: &[Option<String>],
    fresh: &[Row],
) -> bool {
    uids.len() == names.len()
        && uids.len() == addresses.len()
        && uids
            .iter()
            .zip(names)
            .zip(addresses)
            .all(|((uid, name), address)| {
                fresh.iter().any(|row| {
                    row.state == RowState::OnlyOnStation
                        && row.station_uid == Some(*uid)
                        && &row.name == name
                        && &row.address == address
                })
            })
}

#[cfg(test)]
mod tests;
