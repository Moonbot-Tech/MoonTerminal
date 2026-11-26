//! Immutable market rows and compact identities prepared away from the UI thread.

use super::auto_figures::AUTO_FIGURES_KEY;
use super::grouped::{Aggregate, LevSummary, MemberAcct, aggregate, chart_core, group_slots};
use super::table::{Entry, sort_indices};
use moon_core::market::SharedScreenerRow;
use moon_core::market::listing::Listing;
use moon_core::session::CoreId;
use std::collections::HashMap;
use std::sync::Arc;

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
    market_groups: Vec<GroupView>,
    group_of: Vec<u32>,
    sorted_groups: Vec<usize>,
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

/// The member indices and account values for one venue market in the current scope.
#[derive(Clone, Debug)]
pub(super) struct GroupView {
    pub members: Arc<[usize]>,
    pub agg: Aggregate,
    /// Preferred chart destination for the grouped row's open gesture.
    pub chart_core: CoreId,
}

impl GroupView {
    /// Compute the account facts for a nonempty set of members in publication order.
    fn new(entries: &[Entry], members: Arc<[usize]>) -> Self {
        let accounts = || members.iter().map(|&index| account(&entries[index].row));
        let agg = aggregate(accounts());
        let chart_core = chart_core(accounts()).expect("a market group has members");
        Self {
            members,
            agg,
            chart_core,
        }
    }
}

/// One display row. Its lead supplies market columns for a grouped venue market.
#[derive(Debug)]
pub(super) struct SelectedRow {
    pub lead: usize,
    pub group: Option<GroupView>,
}

/// The selected display rows and market and core counts for the footer.
pub(super) struct Selection {
    pub rows: Vec<SelectedRow>,
    pub total: usize,
    pub shown: usize,
    pub cores_shown: usize,
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
        let mut group_of = vec![0; entries.len()];
        let market_groups: Vec<_> = group_slots(entries.iter().map(market_group_key))
            .into_iter()
            .enumerate()
            .map(|(group, members)| {
                for &index in &members {
                    group_of[index] = group as u32;
                }
                GroupView::new(&entries, members.into())
            })
            .collect();
        let mut sorted_groups: Vec<_> =
            market_groups.iter().map(|group| group.members[0]).collect();
        if sort_key != AUTO_FIGURES_KEY {
            sort_group_leads(&entries, &mut sorted_groups, sort_key, sort_desc, |lead| {
                &market_groups[group_of[lead] as usize]
            });
        }
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
            market_groups,
            group_of,
            sorted_groups,
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
    ///     grouped: Whether only the lead of each visible market contributes a Listed answer.
    pub fn hides_waiting_rows(
        &self,
        ranks: &[Option<usize>],
        coin: &super::view::CoinQuery,
        min_vol: f64,
        grouped: bool,
    ) -> bool {
        let waits = |index: usize| {
            let entry = &self.entries[index];
            entry.row.listing == Listing::Pending
                && !coin.admits_listing(entry.row.listing)
                && passes_text_and_volume(entry, coin, min_vol)
        };
        self.listing_pending
            && if grouped {
                self.market_groups.iter().any(|group| {
                    group
                        .members
                        .iter()
                        .copied()
                        .find(|&index| ranks[self.slots[index].core].is_some())
                        .is_some_and(waits)
                })
            } else {
                self.slots
                    .iter()
                    .enumerate()
                    .any(|(index, row)| ranks[row.core].is_some() && waits(index))
            }
    }

    /// Apply the latest controls immediately, sorting indices rather than moving market rows.
    pub fn select(
        &self,
        ranks: &[Option<usize>],
        coin: &super::view::CoinQuery,
        min_vol: f64,
        key: &str,
        desc: bool,
        grouped: bool,
    ) -> Selection {
        if grouped {
            return self.select_grouped(ranks, coin, min_vol, key, desc);
        }
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
        let mut cores_shown = vec![false; self.cores.len()];
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
                cores_shown[row.core] = true;
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
        Selection {
            rows: indices
                .into_iter()
                .map(|lead| SelectedRow { lead, group: None })
                .collect(),
            total: total.into_iter().filter(|&v| v).count(),
            shown: shown.into_iter().filter(|&v| v).count(),
            cores_shown: cores_shown.into_iter().filter(|&v| v).count(),
        }
    }

    /// Filter venue markets on their first visible member, reusing totals for fully visible groups.
    fn select_grouped(
        &self,
        ranks: &[Option<usize>],
        coin: &super::view::CoinQuery,
        min_vol: f64,
        key: &str,
        desc: bool,
    ) -> Selection {
        let allowed = ranks.iter().filter(|rank| rank.is_some()).count();
        let all_visible = allowed == self.cores.len();
        let cached_sort = all_visible
            && key == self.sort_key
            && desc == self.sort_desc
            && key != AUTO_FIGURES_KEY;
        let mut total = 0;
        let mut cores_shown = vec![false; self.cores.len()];
        let mut core_count = 0;
        let mut leads = Vec::with_capacity(self.market_groups.len());
        let mut selected = vec![None; self.market_groups.len()];
        let visible = |index: usize| ranks[self.slots[index].core].is_some();
        let mut admit = |slot: usize| {
            let full = &self.market_groups[slot];
            let lead = if all_visible {
                Some(full.members[0])
            } else {
                full.members.iter().copied().find(|&index| visible(index))
            };
            let Some(lead) = lead else { return };
            total += 1;
            let entry = &self.entries[lead];
            if !coin.admits_listing(entry.row.listing)
                || !passes_text_and_volume(entry, coin, min_vol)
            {
                return;
            }
            let group = if all_visible || full.members.iter().copied().all(visible) {
                full.clone()
            } else {
                let members = full
                    .members
                    .iter()
                    .copied()
                    .filter(|&index| visible(index))
                    .collect();
                GroupView::new(&self.entries, members)
            };
            if core_count < allowed {
                for &index in group.members.iter() {
                    let shown = &mut cores_shown[self.slots[index].core];
                    if !*shown {
                        *shown = true;
                        core_count += 1;
                    }
                }
            }
            leads.push(lead);
            selected[slot] = Some(group);
        };
        if cached_sort {
            for &lead in &self.sorted_groups {
                admit(self.group_of[lead] as usize);
            }
        } else {
            for slot in 0..self.market_groups.len() {
                admit(slot);
            }
            if key == AUTO_FIGURES_KEY {
                coin.sort_by_scan(&self.entries, &mut leads, desc);
            } else {
                sort_group_leads(&self.entries, &mut leads, key, desc, |lead| {
                    selected[self.group_of[lead] as usize]
                        .as_ref()
                        .expect("selected group")
                });
            }
        }
        Selection {
            shown: leads.len(),
            rows: leads
                .into_iter()
                .map(|lead| SelectedRow {
                    lead,
                    group: selected[self.group_of[lead] as usize].take(),
                })
                .collect(),
            total,
            cores_shown: core_count,
        }
    }
}

/// Keep named venues separate from the fallback identity for unidentified providers.
fn market_group_key(entry: &Entry) -> (Result<&str, CoreId>, &str) {
    let venue = if entry.venue.label.is_empty() {
        Err(entry.row.provider)
    } else {
        Ok(entry.venue.label.as_ref())
    };
    (venue, entry.row.market.as_str())
}

/// Read the core-specific fields from the shared Screener row.
fn account(row: &SharedScreenerRow) -> MemberAcct {
    MemberAcct {
        core: row.core,
        leverage_x: row.leverage_x,
        max_leverage: row.max_leverage,
        isolated: row.isolated,
        pnl: row.core_pnl,
        session: row.session,
        pos: row.pos_size,
        orders: row.orders,
    }
}

/// Sort account columns by the visible group's totals and market columns by its lead.
fn sort_group_leads<'a>(
    entries: &[Entry],
    leads: &mut [usize],
    key: &str,
    desc: bool,
    group: impl Fn(usize) -> &'a GroupView,
) {
    let compare: fn(&GroupView, &GroupView) -> std::cmp::Ordering = match key {
        "core" => |a, b| a.members.len().cmp(&b.members.len()),
        "orders" => |a, b| a.agg.orders.cmp(&b.agg.orders),
        "pnl" => |a, b| a.agg.pnl.total_cmp(&b.agg.pnl),
        "session_profit" => |a, b| {
            a.agg
                .session
                .unwrap_or(0.0)
                .total_cmp(&b.agg.session.unwrap_or(0.0))
        },
        "pos" => |a, b| a.agg.pos.total_cmp(&b.agg.pos),
        "lev" => |a, b| leverage_order(a.agg.lev).cmp(&leverage_order(b.agg.lev)),
        _ => return sort_indices(entries, leads, key, desc),
    };
    leads.sort_by(|&a, &b| {
        let order = compare(group(a), group(b));
        if desc { order.reverse() } else { order }
    });
}

/// Compare the highest active leverage first, then the highest stated maximum.
fn leverage_order(lev: LevSummary) -> (i32, i32) {
    match lev {
        LevSummary::None => (0, 0),
        LevSummary::Max(max) => (0, max),
        LevSummary::Same { x, max, .. } => (x, max),
        LevSummary::Range { hi, max, .. } => (hi, max),
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
