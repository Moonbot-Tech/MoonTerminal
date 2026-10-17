//! Saved core-group memberships for detailed and summary reports.
//!
//! The Profit monitor's rules (`moon-ui-gpui` `analytics/profit_monitor/sections.rs`), which this
//! crate cannot import from a binary crate, so the report reads the same way the window does:
//! - a core saved into two groups is listed under BOTH;
//! - groups keep the host's saved order, and the cores in none come last;
//! - a section keeps the order the cores were handed, never the order a group saved them in;
//! - the list stays flat when sectioning would put ONE caption over everything — no saved group
//!   holds a listed core, or a single group holds every one of them.
//!
//! The Profit monitor gives a section of one core no subtotal. Here every section's header row
//! carries its total, and the caller, which reads the totals, reuses that core's own for it.
//! Summary rows use the same membership but retain even a single section covering every core.

use std::collections::HashSet;

use moon_core::config::CoreGroup;

/// One section of the report by cores.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Section<'a> {
    /// The saved group, or `None` for the cores in no group.
    pub(super) group: Option<&'a CoreGroup>,
    /// Positions in the listed cores, ascending.
    pub(super) members: Vec<usize>,
}

/// The sections of `cores` under `groups`, or `None` when the list stays flat.
///
/// Args:
///     cores: The listed cores' ids, in display order.
///     groups: The bot's core groups in resolved display order.
pub(super) fn sections<'a>(cores: &[u64], groups: &'a [CoreGroup]) -> Option<Vec<Section<'a>>> {
    let out = summary_sections(cores, groups);
    (out.len() >= 2).then_some(out)
}

/// Saved groups and ungrouped membership, including a single section covering every core.
/// Unlike core section headings, a summary row remains useful when it is the only row.
pub(super) fn summary_sections<'a>(cores: &[u64], groups: &'a [CoreGroup]) -> Vec<Section<'a>> {
    let mut out: Vec<Section<'a>> = groups
        .iter()
        .map(|group| Section {
            group: Some(group),
            members: (0..cores.len())
                .filter(|&index| group.cores.contains(&cores[index]))
                .collect(),
        })
        .filter(|section| !section.members.is_empty())
        .collect();
    let grouped: HashSet<u64> = groups
        .iter()
        .flat_map(|group| group.cores.iter().copied())
        .collect();
    let loose: Vec<usize> = (0..cores.len())
        .filter(|&index| !grouped.contains(&cores[index]))
        .collect();
    if !loose.is_empty() {
        out.push(Section {
            group: None,
            members: loose,
        });
    }
    out
}

#[cfg(test)]
mod tests;
