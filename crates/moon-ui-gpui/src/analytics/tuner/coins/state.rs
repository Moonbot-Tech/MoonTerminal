//! State of the "By coin" axis: the coin table's own view controls (search, sort,
//! list filter), the WORKING blacklist/whitelist the user edits, and the two background
//! results it renders from (per-coin aggregates + the "Fact vs v1" KPI).
//!
//! The tick boxes show the WORKING lists, not what the strategy has saved. The saved
//! union is kept beside them purely as the baseline a "Changed" view diffs against —
//! that is the whole point of the mode: pick coins, see the plan, then write it.
//!
//! Split from the rendering (`coins/mod.rs`) for the same reason the other axes are:
//! the reload paths write here, the render path only reads.

use std::collections::HashSet;

use gpui::Entity;
use moon_ui::{MoonInputState, MoonSliderState};

use super::super::COIN_DEFAULT_SORT;
use crate::load_state::LoadState;
use moon_core::db::analytics::GroupStat;
use moon_core::db::coin_lists::CoinListRows;
use moon_core::db::tuner::{VarStats, Variant};
use moon_core::db::ReadResult;

/// Which coins the table shows. A coin may sit in BOTH lists — the views are separate
/// lenses, not a partition, so nothing has to resolve that contradiction.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum CoinFilter {
    All,
    Black,
    White,
    /// Coins whose working membership differs from what the strategies have saved — the
    /// pending edit, before it is written anywhere.
    Changed,
}

/// One side of the coin lists: what is saved, and what the user is working with.
///
/// Membership is decided on the UPPER-CASED base token (`BTC_RP` → `BTC`), because the
/// report names a coin with its contract while a strategy field holds bare tokens in
/// whatever case the user typed (`mith,tribe`). Everything goes through [`coin_token`], so
/// the markers, the counters and the variant cannot answer "same coin?" differently.
///
/// Working in TOKENS, not report names, is what the strategy field itself does: listing
/// `BTC` covers `BTC_RP` and `BTC_1230` alike, so ticking either contract ticks the coin.
#[derive(Default)]
pub(in crate::analytics::tuner) struct CoinLists {
    pub(in crate::analytics::tuner) black: HashSet<String>,
    pub(in crate::analytics::tuner) white: HashSet<String>,
}

impl CoinLists {
    /// Build the tuner's saved-list baseline from the same fallible snapshot the picker renders.
    ///
    /// Args:
    ///     rows: Strategy-list rows read in one transaction.
    ///
    /// Returns:
    ///     Both list sides as normalized matching-token sets.
    pub(in crate::analytics::tuner) fn from_rows(rows: &CoinListRows) -> Self {
        Self {
            black: rows.black.iter().map(|row| row.coin.clone()).collect(),
            white: rows.white.iter().map(|row| row.coin.clone()).collect(),
        }
    }

    /// Parse raw `CoinsBlackList` / `CoinsWhiteList` field values into matchable tokens.
    #[cfg(test)]
    pub(in crate::analytics::tuner) fn parse(black: &str, white: &str) -> Self {
        Self {
            black: moon_core::symbol::parse_coin_list(black),
            white: moon_core::symbol::parse_coin_list(white),
        }
    }

    pub(in crate::analytics::tuner) fn is_empty(&self) -> bool {
        self.black.is_empty() && self.white.is_empty()
    }

    /// Return an independent copy used to seed or rebase a working list.
    ///
    /// Returns:
    ///     Both list sides with cloned token sets.
    pub(in crate::analytics::tuner) fn duplicate(&self) -> Self {
        Self {
            black: self.black.clone(),
            white: self.white.clone(),
        }
    }

    /// Replay the edit `was -> now` on top of `self`: add what the user added, remove what
    /// the user removed, and leave everything else to the new baseline.
    ///
    /// Applied to BOTH sides. The whitelist is edited by its own tick column and written by
    /// the same Save, so a removal lost here reaches a strategy just as a blacklist one does.
    ///
    /// Args:
    ///     was: Previous saved baseline against which the draft was made.
    ///     now: Working draft containing the user's additions and removals.
    ///
    /// Returns:
    ///     `self` with the draft delta replayed on its baseline.
    pub(in crate::analytics::tuner) fn with_delta(mut self, was: &Self, now: &Self) -> Self {
        fn replay(base: &mut HashSet<String>, was: &HashSet<String>, now: &HashSet<String>) {
            // The two difference sets are disjoint, so the order of these loops does not
            // matter. What makes a coin the user unticked stay UNTICKED even when the core
            // re-added it meanwhile is that the delta is taken against the OLD baseline: the
            // removal is still in it, and it is applied after the new baseline is laid down.
            for gone in was.difference(now) {
                base.remove(gone);
            }
            for added in now.difference(was) {
                base.insert(added.clone());
            }
        }
        replay(&mut self.black, &was.black, &now.black);
        replay(&mut self.white, &was.white, &now.white);
        self
    }
}

/// THE matching key of a coin — re-exported from `moon_core::symbol` so this module names
/// it once and nothing here re-spells the rule.
pub(in crate::analytics::tuner) use moon_core::symbol::coin_match_key as coin_token;

/// What the working lists come to when applied to a period: the KPI variants plus how
/// many coins each side actually REACHED in that period.
pub(in crate::analytics::tuner) struct CoinPlan {
    pub(in crate::analytics::tuner) variants: Vec<Variant>,
    pub(in crate::analytics::tuner) bl: usize,
    pub(in crate::analytics::tuner) wl: usize,
}

/// State of the "By coin" mode.
pub(in crate::analytics) struct CoinsState {
    /// Per-coin aggregates over the SELECTED strategies (all trades when nothing is
    /// selected) — the numbers each row shows.
    pub(in crate::analytics::tuner) stats: LoadState<Vec<GroupStat>>,
    /// "Fact vs v1" for the KPI matrix; v1 = the working lists applied.
    pub(in crate::analytics::tuner) kpi: LoadState<Vec<VarStats>>,
    /// The list sizes the CURRENT `kpi` was computed for. The v1 column label reads these
    /// rather than the live ones: while a rescan is in flight the matrix still shows the
    /// previous result, and the live counts would title it with an edit it never saw.
    pub(in crate::analytics::tuner) kpi_bl: usize,
    pub(in crate::analytics::tuner) kpi_wl: usize,
    /// The union of the SELECTED strategies' saved lists — the baseline "Changed" diffs
    /// against. Empty when nothing is selected or the strategy DB had nothing to say.
    pub(in crate::analytics::tuner) saved: CoinLists,
    /// What the user is working with. Edited by the tick boxes, and replayed onto each fresh
    /// baseline by [`CoinsState::adopt`] rather than re-seeded from it.
    pub(in crate::analytics::tuner) work: CoinLists,
    pub(in crate::analytics::tuner) search: String,
    pub(in crate::analytics::tuner) search_input: Option<Entity<MoonInputState>>,
    /// Hide coins whose trade count is below this SHARE of the busiest coin in the scope.
    ///
    /// A share, not an absolute count: the slider then has one fixed 0–100 scale whatever the
    /// period holds, so it neither needs rebuilding when the scope changes nor spends most of
    /// its travel on values that hide everything. The absolute figure it works out to is shown
    /// beside it, because that is what actually decides which rows go.
    pub(in crate::analytics::tuner) min_trades_pct: i64,
    /// What that share comes to in trades for the CURRENT scope — recomputed on render, and
    /// the value the row builder filters on.
    pub(in crate::analytics::tuner) min_trades: i64,
    /// Backing state of its slider (lazily created, like the search input).
    pub(in crate::analytics::tuner) min_trades_slider: Option<Entity<MoonSliderState>>,
    /// Coins the user clicked. Their purpose is the strategy list: it highlights whoever
    /// traded them, which is the "who is behind this coin?" the table cannot answer alone.
    pub(in crate::analytics::tuner) picked: HashSet<String>,
    /// Keys (`strategyid@core_uid`) of the strategies that traded `picked` — the rows the
    /// strategy list lights up. Empty while nothing is picked or the read is in flight.
    pub(in crate::analytics::tuner) picked_strats: HashSet<String>,
    pub(in crate::analytics::tuner) picked_seq: u64,
    /// `(column key, descending)` — never None, so the visible order is always one of the
    /// table's OWN columns and always carries its arrow.
    pub(in crate::analytics::tuner) sort: Option<(String, bool)>,
    pub(in crate::analytics::tuner) filter: CoinFilter,
    /// Bumped on every change to `work`/`saved`. The rendered row model is cached against
    /// it, so a repaint that changed no list (hovering a row, for one) reuses the model
    /// instead of rebuilding it over thousands of coins.
    pub(in crate::analytics::tuner) lists_rev: u64,
    pub(in crate::analytics::tuner) seq: u64,
    pub(in crate::analytics::tuner) kpi_seq: u64,
    /// The pending debounced KPI rescan. Held so the next edit REPLACES it — dropping a
    /// gpui `Task` cancels it, so a burst of ticks leaves one timer, not one per click.
    pub(in crate::analytics::tuner) kpi_task: Option<gpui::Task<()>>,
    /// The loaded numbers no longer match the current scope or report generation.
    pub(in crate::analytics::tuner) dirty: bool,
    /// The built row model, kept across repaints (see `super::rows`).
    pub(in crate::analytics::tuner) rows: Option<super::rows::RowsCache>,
}

impl Default for CoinsState {
    fn default() -> Self {
        Self {
            stats: LoadState::default(),
            kpi: LoadState::default(),
            kpi_bl: 0,
            kpi_wl: 0,
            saved: CoinLists::default(),
            work: CoinLists::default(),
            lists_rev: 0,
            search: String::new(),
            search_input: None,
            min_trades_pct: 0,
            min_trades: 0,
            min_trades_slider: None,
            picked: HashSet::new(),
            picked_strats: HashSet::new(),
            picked_seq: 0,
            sort: Some((COIN_DEFAULT_SORT.to_string(), true)),
            filter: CoinFilter::All,
            seq: 0,
            kpi_seq: 0,
            kpi_task: None,
            dirty: true,
            rows: None,
        }
    }
}

impl CoinsState {
    /// Mark report-derived coin statistics stale while preserving working lists and picks.
    ///
    /// Report commits can change the table, KPI, and picked-strategy highlight. They cannot change
    /// the strategy fields that form `saved`/`work`, so clearing those drafts here would turn live
    /// refresh into data loss.
    ///
    /// The method has no return value; callers subsequently reload the active report-derived view.
    pub(in crate::analytics) fn mark_report_stale(&mut self) {
        self.dirty = true;
    }

    /// The scope changed (period, filters, selected strategies): every number and both
    /// coin lists belong to the previous scope now.
    ///
    /// Advances all three request generations even when no replacement load starts here —
    /// otherwise a
    /// request already in flight for the old scope passes its own sequence check on
    /// arrival and publishes the previous strategies' numbers and lists, clearing `dirty`
    /// on the way out so nothing ever recomputes them. (Same reason
    /// `TunerState::invalidate` advances its three.)
    ///
    /// The method has no return value; callers start or defer replacement reads.
    pub(in crate::analytics) fn invalidate(&mut self) {
        self.dirty = true;
        self.seq = self.seq.wrapping_add(1);
        self.kpi_seq = self.kpi_seq.wrapping_add(1);
        self.picked_seq = self.picked_seq.wrapping_add(1);
        self.kpi_task = None;
        self.lists_rev = self.lists_rev.wrapping_add(1);
        // `stats`/`kpi` are guarded by `LoadState`, but the lists are plain sets: left
        // standing they would mark rows with the PREVIOUS strategies' lists under the new
        // selection's name until the background read lands.
        self.saved = CoinLists::default();
        self.work = CoinLists::default();
        // The picked coins and the strategies behind them belong to the previous scope too.
        self.picked.clear();
        self.picked_strats.clear();
        self.settle_filter();
    }

    /// Whether entering the mode requires a recomputation. Mirrors
    /// `TunerState::needs_reload`: a load that FAILED left no data, and only asking
    /// `dirty` would never retry it.
    pub(in crate::analytics) fn needs_reload(&self) -> bool {
        self.stats.data().is_none() || self.dirty
    }

    /// Adopt the strategies' saved lists as the new baseline, REPLAYING whatever the user
    /// has edited on top of it.
    ///
    /// The edit is carried as a delta against the OLD baseline — the coins added since, and
    /// the coins removed since — and re-applied to the new one. Nothing else reconstructs the
    /// user's intent correctly:
    ///
    /// - unioning the baseline in (what this used to do) keeps additions but RESURRECTS every
    ///   removal, because a removal is the absence of a coin and a union cannot see it. An
    ///   untick landing while a reload was in flight came back ticked, and the next Save wrote
    ///   it to the strategy — a user's deletion silently undone on the write path;
    /// - overwriting with the baseline drops the additions instead.
    ///
    /// After a scope change there is nothing to replay: `invalidate` empties both sets, so the
    /// delta is empty and the new baseline is adopted whole. That is why no "did an edit land
    /// mid-flight?" flag is needed any more — the delta is the answer to that question.
    pub(in crate::analytics::tuner) fn adopt(&mut self, saved: CoinLists) {
        let work = saved.duplicate().with_delta(&self.saved, &self.work);
        // Bumped only when something ACTUALLY moved. `send_bulk_changes` schedules echo
        // re-reads 1500/3500 ms after every write, and an unconditional bump made those
        // app-scheduled reloads look like a user edit — which silently dropped a second Save
        // clicked inside that window, and rebuilt the field for nothing on every one of them.
        let moved = work.black != self.work.black
            || work.white != self.work.white
            || saved.black != self.saved.black
            || saved.white != self.saved.white;
        if moved {
            self.lists_rev = self.lists_rev.wrapping_add(1);
        }
        self.work = work;
        self.saved = saved;
    }

    /// Adopt a saved-list read only when the strategies snapshot was available.
    ///
    /// Args:
    ///     result: Fallible saved-list baseline read.
    ///
    /// A failed read deliberately leaves both baseline and draft unchanged.
    pub(in crate::analytics::tuner) fn adopt_if_ready(&mut self, result: ReadResult<CoinLists>) {
        if let Ok(saved) = result {
            self.adopt(saved);
        }
    }

    /// Throw the edit away and go back to what the strategies have saved.
    pub(in crate::analytics::tuner) fn revert(&mut self) {
        self.lists_rev = self.lists_rev.wrapping_add(1);
        self.work = self.saved.duplicate();
        // Same duty as `toggle`: the view the user is on may no longer have anything behind it.
        self.settle_filter();
    }

    /// The absolute trade threshold this share comes to against `busiest`.
    ///
    /// Rounded UP, so any non-zero share actually excludes something: at 1% of 40 trades a
    /// truncating conversion would land on 0 and quietly do nothing.
    pub(in crate::analytics::tuner) fn trades_floor(&self, busiest: i64) -> i64 {
        if self.min_trades_pct <= 0 || busiest <= 0 {
            return 0;
        }
        // Manual ceiling: `div_ceil` on integers is still unstable.
        (busiest * self.min_trades_pct + 99) / 100
    }

    /// Click a coin: plain click selects it alone (and clicking the sole selection clears it),
    /// Ctrl adds or removes — the same rule the strategy list above already teaches.
    pub(in crate::analytics::tuner) fn pick_coin(&mut self, coin: String, multi: bool) {
        if multi {
            if !self.picked.remove(&coin) {
                self.picked.insert(coin);
            }
        } else if self.picked.len() == 1 && self.picked.contains(&coin) {
            self.picked.clear();
        } else {
            self.picked.clear();
            self.picked.insert(coin);
        }
        // The old answer describes the old selection; drop it rather than leave the strategy
        // list lit up for coins that are no longer chosen.
        self.picked_strats.clear();
    }

    /// Is this coin's membership different from what is saved?
    pub(in crate::analytics::tuner) fn is_changed(&self, token: &str) -> bool {
        self.work.black.contains(token) != self.saved.black.contains(token)
            || self.work.white.contains(token) != self.saved.white.contains(token)
    }

    /// Any pending edit at all — drives the revert affordance and the "Changed" view.
    pub(in crate::analytics::tuner) fn has_changes(&self) -> bool {
        self.work.black != self.saved.black || self.work.white != self.saved.white
    }

    /// Tick / untick a coin on one side.
    ///
    /// A blank token is refused rather than stored: it matches no row, so it would sit in
    /// the list inflating every count and marking the panel "changed" against a coin that
    /// does not exist.
    pub(in crate::analytics::tuner) fn toggle(&mut self, black_side: bool, token: String) {
        if token.is_empty() {
            return;
        }
        self.lists_rev = self.lists_rev.wrapping_add(1);
        let set = if black_side {
            &mut self.work.black
        } else {
            &mut self.work.white
        };
        if !set.remove(&token) {
            set.insert(token);
        }
        // The view the user is on may no longer have anything behind it.
        self.settle_filter();
    }

    /// How many coins carry an unsaved edit, over the WHOLE working set.
    ///
    /// Not the row count: the badge sits beside a globally-scoped "revert", so counting only
    /// the coins the search happens to show would report "0 changed" next to a live revert.
    pub(in crate::analytics::tuner) fn changed_count(&self) -> usize {
        self.work
            .black
            .symmetric_difference(&self.saved.black)
            .chain(self.work.white.symmetric_difference(&self.saved.white))
            .collect::<HashSet<_>>()
            .len()
    }

    /// Is a view backed by anything? The chips read this to decide whether they are
    /// clickable and `settle_filter` to decide whether to fall back — one rule, so an
    /// enabled chip can never lead to a view that empties itself.
    pub(in crate::analytics::tuner) fn filter_available(&self, f: CoinFilter) -> bool {
        match f {
            CoinFilter::All => true,
            CoinFilter::Black => !self.work.black.is_empty(),
            CoinFilter::White => !self.work.white.is_empty(),
            CoinFilter::Changed => self.has_changes(),
        }
    }

    /// Retire the current KPI request and start a new generation. Returns the new one.
    pub(in crate::analytics::tuner) fn bump_kpi(&mut self) -> u64 {
        self.kpi_seq = self.kpi_seq.wrapping_add(1);
        self.kpi.begin();
        self.kpi_seq
    }

    /// Fall back to "All" when the current view has nothing behind it. Without this,
    /// deselecting a strategy leaves the table stuck on an empty "blacklist" view that
    /// reads as "no coins matched" when the truth is "no blacklist is known here".
    pub(in crate::analytics::tuner) fn settle_filter(&mut self) {
        if !self.filter_available(self.filter) {
            self.filter = CoinFilter::All;
        }
    }

    /// Variants for the KPI matrix: `[Fact]`, plus v1 when the working lists say anything.
    ///
    /// v1 is the counterfactual the mode exists to answer — the same trades WITH these
    /// lists applied: keep only whitelisted coins (when a whitelist exists), then drop the
    /// blacklisted ones, exactly as a strategy evaluates its two lists.
    ///
    /// The lists are tokens while the scan filters raw report names, so they are expanded
    /// here against `universe` — the very coin set the table draws from. A token naming no
    /// traded coin contributes nothing rather than becoming an empty restriction.
    pub(in crate::analytics::tuner) fn plan(&self, universe: &[GroupStat]) -> CoinPlan {
        CoinPlan::build(&self.work, universe)
    }
}

impl CoinPlan {
    /// Build a plan from a fresh saved baseline with the request-start draft replayed.
    ///
    /// Args:
    ///     fresh_saved: Saved lists read by the current background request.
    ///     previous_saved: Saved baseline visible when the request started.
    ///     previous_work: Working draft visible when the request started.
    ///     universe: Report coin rows used to expand list tokens.
    ///
    /// Returns:
    ///     A plan matching the working lists that `CoinsState::adopt` will publish.
    pub(in crate::analytics::tuner) fn build_rebased(
        fresh_saved: &CoinLists,
        previous_saved: &CoinLists,
        previous_work: &CoinLists,
        universe: &[GroupStat],
    ) -> CoinPlan {
        let work = fresh_saved
            .duplicate()
            .with_delta(previous_saved, previous_work);
        CoinPlan::build(&work, universe)
    }

    /// Build the plan from BARE lists, so the background reload can score the lists it just
    /// read without waiting for them to reach the view first. Reading them from state there
    /// is what left v1 missing until the user clicked something.
    pub(in crate::analytics::tuner) fn build(work: &CoinLists, universe: &[GroupStat]) -> CoinPlan {
        // v1 ALWAYS exists — it is the column the whole mode is read against, and a matrix
        // that grows a column the moment the first coin is ticked shifts every number under
        // it. Empty, it restricts nothing and simply reproduces the fact, which is the honest
        // reading of "nothing applied yet".
        let mut plan = CoinPlan {
            variants: vec![Variant::default(), Variant::default()],
            bl: 0,
            wl: 0,
        };
        // An EMPTY universe is "the coin set is not known yet", not "no coin qualifies":
        // planning against it would turn a live whitelist into `Some(empty)` — the "keeps
        // nothing" case — and publish a v1 of zero trades that no list actually produced.
        if work.is_empty() || universe.is_empty() {
            return plan;
        }
        let white_on = !work.white.is_empty();
        let (mut coins_in, mut coins_out) = (Vec::new(), Vec::new());
        let (mut bl_tok, mut wl_tok) = (HashSet::new(), HashSet::new());
        for g in universe {
            if g.name.is_empty() {
                continue;
            }
            let token = coin_token(&g.name);
            if white_on && work.white.contains(&token) {
                coins_in.push(g.name.clone());
                wl_tok.insert(token.clone());
            }
            if work.black.contains(&token) {
                coins_out.push(g.name.clone());
                bl_tok.insert(token);
            }
        }
        // Stable order: the variant is part of a background request's identity.
        coins_in.sort();
        coins_out.sort();
        // Reported as the coins the plan actually REACHED, not as the list sizes: a list
        // may name coins that never traded in this period, and labelling the column with
        // them would claim a restriction the numbers underneath were not scored under.
        plan.bl = bl_tok.len();
        plan.wl = wl_tok.len();
        plan.variants[1] = Variant {
            // `Some` whenever a whitelist is switched on, even when nothing matched: that
            // case keeps NO trades, and an empty `Vec` would read as "no whitelist".
            coins_in: white_on.then_some(coins_in),
            coins_out,
            ..Default::default()
        };
        plan
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own
// `test` shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
