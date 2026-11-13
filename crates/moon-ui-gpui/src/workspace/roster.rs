//! How the Auto rail's roster is split into server blocks and exchange sections.
//!
//! Two independent switches from the rail's ⚙ popup decide the shape. Server grouping is the OUTER
//! level and is off by default; exchange grouping is the inner level and is on by default, so a
//! profile that never touched the popup sees the exchange-only list it always had.

use std::collections::HashMap;
use std::net::IpAddr;

use moon_core::venue::CoreVenue;

use super::{WorkspaceRosterInput, WorkspaceRosterRow};

/// Which levels of grouping the rail draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RosterGrouping {
    /// Group cores by the machine they run on, outermost.
    pub(crate) by_server: bool,
    /// Group cores by exchange, inside a server block when `by_server` is on too.
    pub(crate) by_exchange: bool,
}

impl Default for RosterGrouping {
    /// The rail's shape before this switch existed: exchange headings only.
    fn default() -> Self {
        Self {
            by_server: false,
            by_exchange: true,
        }
    }
}

/// The machine one server block stands for.
///
/// Ordered so the derived `Ord` puts known addresses first, in address order, and the unknown block
/// last. Core Status ranks its `Server N` ordinals in address order too, so while every machine has
/// reported, the rail's blocks read Server 1, Server 2, … in sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum RosterServer {
    /// Cores on this address — the one their feed reported, else the one their configuration
    /// names. Two cores on one address are one machine even when their ports differ, the rule Core
    /// Status groups by.
    Address(IpAddr),
    /// Cores neither a feed nor their configuration places on an IP: a key without an address, or
    /// a host name the feed has not resolved yet. Last, because it is a missing fact rather than an identity
    /// defect that needs attention the way an unidentified exchange does.
    Unknown,
}

/// What heads one section of rows inside a server block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RosterSectionHeading {
    /// Exchange grouping is off: the rows follow their server heading (or Overview) directly.
    None,
    /// An exchange heading; `None` inside is the explicit "exchange not identified" section.
    Exchange(Option<CoreVenue>),
}

/// One section of rows in the all-core roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceRosterSection {
    pub(crate) heading: RosterSectionHeading,
    pub(crate) rows: Vec<WorkspaceRosterRow>,
}

/// One server block. With server grouping off there is a single block, and it draws no heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceRosterServer {
    /// The machine this block stands for, or `None` when server grouping is off.
    pub(crate) server: Option<RosterServer>,
    pub(crate) sections: Vec<WorkspaceRosterSection>,
}

/// Split already-derived rows into server blocks and exchange sections.
///
/// Args:
///     inputs: Configured cores in canonical member order.
///     rows: The derived row of each input, index for index.
///     grouping: Which levels to draw.
///
/// Returns:
///     Server blocks in address order (unknown last), each holding its exchange sections in
///     directory order. Canonical input order is kept inside every section. Empty input yields no
///     block at all, matching the exchange-only roster's behaviour.
pub(crate) fn group_roster_rows(
    inputs: &[WorkspaceRosterInput],
    rows: Vec<WorkspaceRosterRow>,
    grouping: RosterGrouping,
) -> Vec<WorkspaceRosterServer> {
    debug_assert_eq!(inputs.len(), rows.len());
    if inputs.is_empty() {
        return Vec::new();
    }
    let blocks: Vec<(Option<RosterServer>, Vec<usize>)> = if grouping.by_server {
        // Indexed by key rather than scanned: the rail rebuilds this on every render, and a large
        // fleet spread over many machines would make a linear scan quadratic.
        let mut at: HashMap<RosterServer, usize> = HashMap::new();
        let mut blocks: Vec<(RosterServer, Vec<usize>)> = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            let key = input
                .server
                .map_or(RosterServer::Unknown, RosterServer::Address);
            let slot = *at.entry(key).or_insert_with(|| {
                blocks.push((key, Vec::new()));
                blocks.len() - 1
            });
            blocks[slot].1.push(index);
        }
        blocks.sort_by_key(|(key, _)| *key);
        blocks
            .into_iter()
            .map(|(key, members)| (Some(key), members))
            .collect()
    } else {
        vec![(None, (0..inputs.len()).collect())]
    };

    let mut rows: Vec<Option<WorkspaceRosterRow>> = rows.into_iter().map(Some).collect();
    let mut take = |indices: Vec<usize>| -> Vec<WorkspaceRosterRow> {
        indices
            .into_iter()
            .filter_map(|index| rows.get_mut(index).and_then(Option::take))
            .collect()
    };
    blocks
        .into_iter()
        .map(|(server, members)| {
            let sections = if grouping.by_exchange {
                moon_core::session::core_order::exchange_sections(
                    members
                        .iter()
                        .map(|&index| (index, inputs[index].venue.as_ref())),
                )
                .into_iter()
                .map(|(venue, indices)| WorkspaceRosterSection {
                    heading: RosterSectionHeading::Exchange(venue.cloned()),
                    rows: take(indices),
                })
                .collect()
            } else {
                vec![WorkspaceRosterSection {
                    heading: RosterSectionHeading::None,
                    rows: take(members),
                }]
            };
            WorkspaceRosterServer { server, sections }
        })
        .collect()
}

#[cfg(test)]
mod tests;
