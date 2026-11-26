//! Immutable market rows and compact identities prepared away from the UI thread.

use super::auto_figures::AUTO_FIGURES_KEY;
use super::grouped::{Aggregate, LevSummary, MemberAcct, aggregate, chart_core, group_slots};
use super::table::{Entry, sort_indices};
use moon_core::feed::ExchangeId;
use moon_core::market::SharedScreenerRow;
use moon_core::market::listing::Listing;
use moon_core::session::CoreId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
    partial_groups: Mutex<Option<PartialGroups>>,
    /// Whether some row's Listed answer is still on its way (`Listing::Pending`).
    pub listing_pending: bool,
    /// When the last of those answers is expected, in Unix milliseconds; set by the publication
    /// that built this, from the listing book it signed the rows with.
    pub listing_settles_ms: Option<i64>,
}

/// The latest partial core scope and its most recently requested publication sort.
pub(super) struct PartialGroups {
    ranks: Vec<Option<usize>>,
    groups: Vec<Option<GroupView>>,
    sort: Option<(String, bool)>,
    sorted_leads: Vec<usize>,
}

impl PartialGroups {
    /// Reuse the same group views across sort changes and filter edits.
    fn sort(&mut self, prepared: &Prepared, key: &str, desc: bool) {
        if self.sort.as_ref().map(|(key, desc)| (key.as_str(), *desc)) == Some((key, desc)) {
            return;
        }
        self.sorted_leads.clear();
        self.sorted_leads
            .extend(self.groups.iter().flatten().map(|group| group.members[0]));
        sort_group_leads(
            &prepared.entries,
            &mut self.sorted_leads,
            key,
            desc,
            |lead| {
                self.groups[prepared.group_of[lead] as usize]
                    .as_ref()
                    .expect("lead belongs to a visible group")
            },
        );
        self.sort = Some((key.to_string(), desc));
    }
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

    /// Find the first member admitted by the current source scope.
    fn visible_lead(&self, visible: impl Fn(usize) -> bool) -> Option<usize> {
        self.members.iter().copied().find(|&index| visible(index))
    }
}

/// One display row. Its lead supplies market columns for a grouped venue market.
#[derive(Clone, Debug)]
pub(super) struct SelectedRow {
    pub lead: usize,
    pub group: Option<GroupView>,
}

impl SelectedRow {
    /// Resolve the member owning the chart and its native coin spelling.
    pub(super) fn chart_entry<'a>(&self, entries: &'a [Entry]) -> &'a Entry {
        let lead = &entries[self.lead];
        self.group
            .as_ref()
            .and_then(|group| {
                group
                    .members
                    .iter()
                    .map(|&index| &entries[index])
                    .find(|entry| entry.row.core == group.chart_core)
            })
            .unwrap_or(lead)
    }
}

/// The selected display rows and market and core counts for the footer.
pub(super) struct Selection {
    pub rows: Vec<SelectedRow>,
    pub total: usize,
    pub shown: usize,
    pub cores_shown: usize,
    pub filters: FilterMatches,
}

/// Evidence for what removed the last row, collected along the normal selection pass.
#[derive(Default)]
pub(super) struct FilterMatches {
    pub market: bool,
    pub without_volume: bool,
}

impl FilterMatches {
    /// Record the Market match before the other filters, and the candidate before Vol.
    fn admits(
        &mut self,
        row: &SelectionRow,
        entry: &Entry,
        text_matches: &[bool],
        query: &super::view::CoinQuery,
        min_vol: f64,
    ) -> bool {
        let market = text_matches[row.search];
        self.market |= market;
        let volume = min_vol <= 0.0 || min_vol.is_nan() || row.volume >= min_vol;
        if !market || (!volume && self.without_volume) {
            return false;
        }
        let without_volume = query.admits_listing(row.listing) && query.admits_scan(entry);
        self.without_volume |= without_volume;
        without_volume && volume
    }
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
            partial_groups: Mutex::default(),
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
                        .visible_lead(|index| ranks[self.slots[index].core].is_some())
                        .is_some_and(waits)
                })
            } else {
                self.slots
                    .iter()
                    .enumerate()
                    .any(|(index, row)| ranks[row.core].is_some() && waits(index))
            }
    }

    /// Apply the latest controls immediately, sorting indices and tracking grouped UI stalls.
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
            let _stall = crate::diag::stall::span(
                crate::diag::stall::StallKind::Input,
                "screener:select-grouped",
            );
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
        let text_matches = self.text_matches(coin);
        let mut filters = FilterMatches::default();
        let mut admit = |index: usize| {
            let row = &self.slots[index];
            total[row.market] = true;
            if filters.admits(row, &self.entries[index], &text_matches, coin, min_vol) {
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
            filters,
        }
    }

    /// Match each distinct search identity once per query, even if many cores publish it.
    fn text_matches(&self, coin: &super::view::CoinQuery) -> Vec<bool> {
        self.search_rows
            .iter()
            .map(|&index| {
                coin.is_empty() || super::view::matches_coin_filter(&self.entries[index], coin)
            })
            .collect()
    }

    /// Prepare the current partial scope off-thread with the next market publication.
    pub fn prime_partial_groups(&self, visible: &HashMap<CoreId, Option<usize>>) {
        let ranks = self.ranks_for(visible);
        if self.needs_scope_prime(&ranks, visible.len() > 1) {
            self.install_partial_groups(self.prepare_partial_groups(ranks));
        }
    }

    /// Map the admitted live sessions to the ranks of the cores in this publication.
    pub(super) fn ranks_for(&self, visible: &HashMap<CoreId, Option<usize>>) -> Vec<Option<usize>> {
        self.cores
            .iter()
            .map(|core| visible.get(core).copied().flatten())
            .collect()
    }

    /// A full scope is already prepared, and an empty scope has no groups to build.
    pub(super) fn needs_scope_prime(&self, ranks: &[Option<usize>], grouped: bool) -> bool {
        let count = ranks.iter().flatten().count();
        grouped
            && (1..self.cores.len()).contains(&count)
            && self
                .partial_groups
                .lock()
                .expect("partial group cache")
                .as_ref()
                .is_none_or(|scope| scope.ranks != ranks)
    }

    /// Build a replacement without holding the cache lock on the published rows.
    pub(super) fn prepare_partial_groups(&self, ranks: Vec<Option<usize>>) -> PartialGroups {
        let mut scope = self.build_partial_groups(ranks);
        if self.sort_key != AUTO_FIGURES_KEY {
            scope.sort(self, &self.sort_key, self.sort_desc);
        }
        scope
    }

    /// Install the completed membership after the caller checks its scope is still current.
    pub(super) fn install_partial_groups(&self, scope: PartialGroups) {
        *self.partial_groups.lock().expect("partial group cache") = Some(scope);
    }

    /// Cache visible groups for the current core scope, bounded to one scope per publication.
    fn partial_group_cache(
        &self,
        ranks: &[Option<usize>],
    ) -> std::sync::MutexGuard<'_, Option<PartialGroups>> {
        let mut cache = self.partial_groups.lock().expect("partial group cache");
        if cache.as_ref().is_none_or(|scope| scope.ranks != ranks) {
            *cache = Some(self.build_partial_groups(ranks.to_vec()));
        }
        cache
    }

    /// Reaggregate only groups that lost members to the current scope.
    fn build_partial_groups(&self, ranks: Vec<Option<usize>>) -> PartialGroups {
        let visible = |index: usize| ranks[self.slots[index].core].is_some();
        let groups = self
            .market_groups
            .iter()
            .map(|full| {
                if full.members.iter().copied().all(visible) {
                    return Some(full.clone());
                }
                let members: Vec<_> = full
                    .members
                    .iter()
                    .copied()
                    .filter(|&index| visible(index))
                    .collect();
                (!members.is_empty()).then(|| GroupView::new(&self.entries, members.into()))
            })
            .collect();
        PartialGroups {
            ranks,
            groups,
            sort: None,
            sorted_leads: Vec::new(),
        }
    }

    /// Filter venue markets on their first visible member and reuse scope totals and stable sorts.
    fn select_grouped(
        &self,
        ranks: &[Option<usize>],
        coin: &super::view::CoinQuery,
        min_vol: f64,
        key: &str,
        desc: bool,
    ) -> Selection {
        let allowed = ranks.iter().filter(|rank| rank.is_some()).count();
        if allowed == 0 {
            return Selection {
                rows: Vec::new(),
                total: 0,
                shown: 0,
                cores_shown: 0,
                filters: FilterMatches::default(),
            };
        }
        let all_visible = allowed == self.cores.len();
        let scan_sort = key == AUTO_FIGURES_KEY;
        let cached_sort = !scan_sort && key == self.sort_key && desc == self.sort_desc;
        let mut partial_cache = (!all_visible).then(|| self.partial_group_cache(ranks));
        if !scan_sort && let Some(cache) = partial_cache.as_mut() {
            cache
                .as_mut()
                .expect("partial groups cached")
                .sort(self, key, desc);
        }
        let scope = partial_cache
            .as_ref()
            .map(|cache| cache.as_ref().expect("partial groups cached"));
        let unsorted_leads;
        let (leads, already_sorted) = match scope {
            Some(scope) if !scan_sort => (scope.sorted_leads.as_slice(), true),
            Some(scope) => {
                unsorted_leads = scope
                    .groups
                    .iter()
                    .flatten()
                    .map(|group| group.members[0])
                    .collect::<Vec<_>>();
                (unsorted_leads.as_slice(), false)
            }
            None if cached_sort => (self.sorted_groups.as_slice(), true),
            None => {
                unsorted_leads = self
                    .market_groups
                    .iter()
                    .map(|group| group.members[0])
                    .collect();
                (unsorted_leads.as_slice(), false)
            }
        };
        let group_for = |lead: usize| {
            let slot = self.group_of[lead] as usize;
            match scope {
                Some(scope) => scope.groups[slot]
                    .as_ref()
                    .expect("lead belongs to a visible group"),
                None => &self.market_groups[slot],
            }
        };
        let text_matches = self.text_matches(coin);
        let mut filters = FilterMatches::default();
        let mut cores_shown = vec![false; self.cores.len()];
        let mut core_count = 0;
        let mut filtered: Vec<_> = leads
            .iter()
            .copied()
            .filter(|&lead| {
                let row = &self.slots[lead];
                filters.admits(row, &self.entries[lead], &text_matches, coin, min_vol)
            })
            .collect();
        if scan_sort {
            coin.sort_by_scan(&self.entries, &mut filtered, desc);
        } else if !already_sorted {
            sort_group_leads(&self.entries, &mut filtered, key, desc, group_for);
        }
        let rows = filtered
            .into_iter()
            .map(|lead| {
                let group = group_for(lead);
                if core_count < allowed {
                    for &index in group.members.iter() {
                        let shown = &mut cores_shown[self.slots[index].core];
                        if !*shown {
                            *shown = true;
                            core_count += 1;
                        }
                    }
                }
                SelectedRow {
                    lead,
                    group: Some(group.clone()),
                }
            })
            .collect::<Vec<_>>();
        Selection {
            shown: rows.len(),
            rows,
            total: leads.len(),
            cores_shown: core_count,
            filters,
        }
    }
}

/// Keep named venues separate from the fallback identity for unidentified providers.
fn market_group_key(entry: &Entry) -> (Result<ExchangeId, CoreId>, &str) {
    let venue = entry.venue.id.ok_or(entry.row.provider);
    (venue, entry.row.market.as_str())
}

/// Read the core-specific fields from the shared Screener row.
pub(super) fn account(row: &SharedScreenerRow) -> MemberAcct {
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
    use std::cmp::Ordering;

    if key == "session_profit" {
        leads.sort_by(
            |&a, &b| match (group(a).agg.session, group(b).agg.session) {
                (Some(a), Some(b)) => {
                    if desc {
                        b.total_cmp(&a)
                    } else {
                        a.total_cmp(&b)
                    }
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        );
        return;
    }
    let compare: fn(&GroupView, &GroupView) -> Ordering = match key {
        "core" => |a, b| a.members.len().cmp(&b.members.len()),
        "orders" => |a, b| a.agg.orders.cmp(&b.agg.orders),
        "pnl" => |a, b| a.agg.pnl.total_cmp(&b.agg.pnl),
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
