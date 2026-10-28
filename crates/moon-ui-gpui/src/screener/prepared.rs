//! Immutable market rows and compact identities prepared away from the UI thread.

use super::table::{Entry, sort_indices};
use moon_core::session::CoreId;
use std::collections::HashMap;

/// A coherent source publication, including dense identities for cheap footer counts.
#[derive(Default)]
pub(super) struct Prepared {
    pub entries: Vec<Entry>,
    pub cores: Vec<CoreId>,
    slots: Vec<(usize, usize)>,
    core_rows: Vec<Vec<usize>>,
    groups: Vec<Vec<usize>>,
    markets: usize,
    sort_key: String,
    sort_desc: bool,
    sorted: Vec<usize>,
}

impl Prepared {
    /// Prepare identities and the current sort once; repeated member rows share a market slot.
    pub fn new(entries: Vec<Entry>, sort_key: &str, sort_desc: bool) -> Self {
        let mut cores = Vec::new();
        let mut core_slots = HashMap::new();
        let mut market_slots = HashMap::new();
        let slots: Vec<(usize, usize)> = entries
            .iter()
            .map(|entry| {
                let next = cores.len();
                let core = *core_slots.entry(entry.row.core).or_insert_with(|| {
                    cores.push(entry.row.core);
                    next
                });
                let next = market_slots.len();
                let market = *market_slots
                    .entry((entry.row.provider, entry.row.market.as_str()))
                    .or_insert(next);
                (core, market)
            })
            .collect();
        let markets = market_slots.len();
        let mut core_rows = vec![Vec::new(); cores.len()];
        for (index, &(core, _)) in slots.iter().enumerate() {
            core_rows[core].push(index);
        }
        let mut providers = HashMap::new();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (core, rows) in core_rows.iter().enumerate() {
            let provider = entries[rows[0]].row.provider;
            let next = groups.len();
            let group = *providers.entry(provider).or_insert_with(|| {
                groups.push(Vec::new());
                next
            });
            groups[group].push(core);
        }
        let mut sorted: Vec<_> = (0..entries.len()).collect();
        sort_indices(&entries, &mut sorted, sort_key, sort_desc);
        Self {
            entries,
            cores,
            slots,
            markets,
            core_rows,
            groups,
            sort_key: sort_key.to_string(),
            sort_desc,
            sorted,
        }
    }

    /// Apply the latest controls immediately, sorting indices rather than moving market rows.
    pub fn select(
        &self,
        ranks: &[Option<usize>],
        coin: &str,
        min_vol: f64,
        key: &str,
        desc: bool,
    ) -> (Vec<usize>, usize, usize) {
        let mut total = vec![false; self.markets];
        let mut shown = vec![false; self.markets];
        let mut indices = Vec::with_capacity(self.entries.len());
        // A group's position comes from its FIRST VISIBLE member, not a hidden provider.
        // Rebuild only this small core ordering; walking each core's indices keeps market order.
        let mut groups: Vec<_> = self
            .groups
            .iter()
            .filter_map(|members| {
                let mut members: Vec<_> = members
                    .iter()
                    .filter_map(|&core| ranks[core].map(|rank| (rank, core)))
                    .collect();
                members.sort_by_key(|&(rank, _)| rank);
                Some((members.first()?.0, members))
            })
            .collect();
        groups.sort_by_key(|(rank, _)| *rank);
        // Filtering a stable sort keeps ties correct while the visible core order remains
        // a subsequence of the publication's order. Other control changes use index sorting.
        let core_order: Vec<_> = groups
            .iter()
            .flat_map(|(_, members)| members.iter().map(|&(_, core)| core))
            .collect();
        let cached_sort = key == self.sort_key
            && desc == self.sort_desc
            && core_order.windows(2).all(|pair| pair[0] < pair[1]);
        for (_, members) in groups {
            for (_, core) in members {
                for &index in &self.core_rows[core] {
                    let entry = &self.entries[index];
                    let market = self.slots[index].1;
                    total[market] = true;
                    if (!coin.is_empty() && !super::view::matches_coin_filter(entry, coin))
                        || (min_vol > 0.0 && entry.row.vol_24h < min_vol)
                        || (min_vol > 0.0 && entry.row.vol_24h.is_nan())
                    {
                        continue;
                    }
                    shown[market] = true;
                    indices.push(index);
                }
            }
        }
        if cached_sort {
            let mut selected = vec![false; self.entries.len()];
            for &index in &indices {
                selected[index] = true;
            }
            indices = self
                .sorted
                .iter()
                .copied()
                .filter(|&index| selected[index])
                .collect();
        } else {
            sort_indices(&self.entries, &mut indices, key, desc);
        }
        (
            indices,
            total.into_iter().filter(|&v| v).count(),
            shown.into_iter().filter(|&v| v).count(),
        )
    }
}

#[cfg(test)]
mod tests;
