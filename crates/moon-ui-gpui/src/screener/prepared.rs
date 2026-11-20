//! Immutable market rows and compact identities prepared away from the UI thread.

use super::auto_figures::AUTO_FIGURES_KEY;
use super::table::{Entry, sort_indices};
use moon_core::market::listing::Listing;
use moon_core::session::CoreId;
use std::collections::HashMap;

/// A coherent source publication, including dense identities for cheap footer counts.
#[derive(Default)]
pub(super) struct Prepared {
    pub entries: Vec<Entry>,
    pub cores: Vec<CoreId>,
    slots: Vec<SelectionRow>,
    search_rows: Vec<usize>,
    core_rows: Vec<Vec<usize>>,
    groups: Vec<Vec<usize>>,
    markets: usize,
    sort_key: String,
    sort_desc: bool,
    sorted: Vec<usize>,
    /// Whether some row's Listed answer is still on its way (`Listing::Pending`).
    pub listing_pending: bool,
    /// When the last of those answers is expected, in Unix milliseconds; set by the publication
    /// that built this, from the listing book it signed the rows with.
    pub listing_settles_ms: Option<i64>,
}

/// Fields used by selection, kept together to avoid chasing market data in sorted order.
struct SelectionRow {
    core: usize,
    market: usize,
    search: usize,
    volume: f64,
    listing: Listing,
}

impl Prepared {
    /// Prepare identities and the current sort once; repeated member rows share a market slot.
    pub fn new(entries: Vec<Entry>, sort_key: &str, sort_desc: bool) -> Self {
        let mut cores = Vec::new();
        let mut core_slots = HashMap::new();
        let mut market_slots = HashMap::new();
        let mut searches = HashMap::new();
        let mut search_rows = Vec::new();
        let slots: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let next = cores.len();
                let core = *core_slots.entry(entry.row.core).or_insert_with(|| {
                    cores.push(entry.row.core);
                    next
                });
                let next = market_slots.len();
                let market = *market_slots
                    .entry((entry.row.provider, entry.row.market.as_str()))
                    .or_insert(next);
                let search = *searches
                    .entry((
                        entry.market_uc.as_ref(),
                        entry.coin_uc.as_ref(),
                        entry.row.identity.as_str(),
                    ))
                    .or_insert_with(|| {
                        search_rows.push(index);
                        search_rows.len() - 1
                    });
                SelectionRow {
                    core,
                    market,
                    search,
                    volume: entry.row.vol_24h,
                    listing: entry.row.listing,
                }
            })
            .collect();
        let markets = market_slots.len();
        let mut core_rows = vec![Vec::new(); cores.len()];
        for (index, row) in slots.iter().enumerate() {
            core_rows[row.core].push(index);
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
        let listing_pending = entries
            .iter()
            .any(|entry| entry.row.listing == moon_core::market::listing::Listing::Pending);
        Self {
            entries,
            cores,
            slots,
            search_rows,
            markets,
            core_rows,
            groups,
            sort_key: sort_key.to_string(),
            sort_desc,
            sorted,
            listing_pending,
            listing_settles_ms: None,
        }
    }

    /// Whether the Listed list ALONE hides a row whose answer is still on its way.
    ///
    /// What lets an empty table say "Listed is still being collected" and mean it: the row is on
    /// a core the table shows, passes the Market box, the Vol. minimum and the auto-figures
    /// filter, is waiting, and is
    /// kept out by nothing but the ticks. A table emptied by a text that matches nothing, or by a
    /// selection that admits waiting rows and found none, is simply empty.
    ///
    /// Args:
    ///     ranks: Per core of the publication, its display rank, or `None` when it is not shown.
    ///     coin: The Market box's query, carrying the Listed list's ticks.
    ///     min_vol: The Vol. minimum; zero or less for none.
    pub fn hides_waiting_rows(
        &self,
        ranks: &[Option<usize>],
        coin: &super::view::CoinQuery,
        min_vol: f64,
    ) -> bool {
        self.listing_pending
            && self.entries.iter().zip(&self.slots).any(|(entry, row)| {
                ranks[row.core].is_some()
                    && entry.row.listing == Listing::Pending
                    && !coin.admits_listing(entry.row.listing)
                    && passes_text_and_volume(entry, coin, min_vol)
            })
    }

    /// Apply the latest controls immediately, sorting indices rather than moving market rows.
    pub fn select(
        &self,
        ranks: &[Option<usize>],
        coin: &super::view::CoinQuery,
        min_vol: f64,
        key: &str,
        desc: bool,
    ) -> (Vec<usize>, usize, usize) {
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
        // The auto-figures column sorts by the latest press's answers, which a publication prepared
        // in the background never sees: it is sorted here, on every pass, and never from the cache.
        let cached_sort = key == self.sort_key
            && key != AUTO_FIGURES_KEY
            && desc == self.sort_desc
            && core_order.windows(2).all(|pair| pair[0] < pair[1]);
        let mut total = vec![false; self.markets];
        let mut shown = vec![false; self.markets];
        let mut indices = Vec::with_capacity(self.entries.len());
        let text_matches: Vec<_> = self
            .search_rows
            .iter()
            .map(|&index| {
                coin.is_empty() || super::view::matches_coin_filter(&self.entries[index], coin)
            })
            .collect();
        let volume_active = min_vol > 0.0;
        let mut admit = |index: usize| {
            let row = &self.slots[index];
            total[row.market] = true;
            if text_matches[row.search]
                && (!volume_active || row.volume >= min_vol)
                && coin.admits_listing(row.listing)
                && coin.admits_scan(&self.entries[index])
            {
                shown[row.market] = true;
                indices.push(index);
            }
        };
        if cached_sort {
            for &index in &self.sorted {
                if ranks[self.slots[index].core].is_some() {
                    admit(index);
                }
            }
        } else {
            for core in core_order {
                for &index in &self.core_rows[core] {
                    admit(index);
                }
            }
            if key == AUTO_FIGURES_KEY {
                coin.sort_by_scan(&self.entries, &mut indices, desc);
            } else {
                sort_indices(&self.entries, &mut indices, key, desc);
            }
        }
        (
            indices,
            total.into_iter().filter(|&v| v).count(),
            shown.into_iter().filter(|&v| v).count(),
        )
    }
}

/// Whether a row passes the Market box, the Vol. minimum and the auto-figures filter: every row
/// filter but the Listed list, which its two callers ask about separately.
fn passes_text_and_volume(entry: &Entry, coin: &super::view::CoinQuery, min_vol: f64) -> bool {
    coin.admits_scan(entry)
        && (coin.is_empty() || super::view::matches_coin_filter(entry, coin))
        && !(min_vol > 0.0 && entry.row.vol_24h < min_vol)
        && !(min_vol > 0.0 && entry.row.vol_24h.is_nan())
}

#[cfg(test)]
mod tests;
