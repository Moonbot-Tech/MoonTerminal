//! State of the "By coin" axis: the coin table's own view controls (search, sort,
//! list filter), the WORKING blacklist/whitelist the user edits, and the two background
//! results it renders from (per-coin aggregates + the Fact / Manual / Auto KPI).
//!
//! The tick boxes show the WORKING lists, not what the strategy has saved. The saved
//! union is kept beside them purely as the baseline a "Changed" view diffs against —
//! that is the whole point of the mode: pick coins, see the plan, then write it.
//!
//! Split from the rendering (`coins/mod.rs`) for the same reason the other axes are:
//! the reload paths write here, the render path only reads.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::Entity;
use moon_ui::MoonInputState;

use super::super::{COIN_COLS, COIN_DEFAULT_SORT};
use crate::load_state::LoadState;
use moon_core::config::CoinsAxisLayout;
use moon_core::db::ReadResult;
use moon_core::db::analytics::GroupStat;
use moon_core::db::coin_lists::CoinListRows;
use moon_core::db::tuner::coin_pick::CoinPick;
use moon_core::db::tuner::{VarStats, Variant};

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
    /// Coins a finished automatic pick suggests for either list.
    Suggested,
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

/// Longest coin name a list field accepts; a longer paste is not a coin and would push the
/// chip's remove button out of the field.
pub(in crate::analytics::tuner) const MAX_TOKEN_LEN: usize = 32;

/// The pick's own defaults and range, named once in `moon_core`.
pub(in crate::analytics::tuner) use moon_core::db::tuner::coin_pick::{
    DEFAULT_HOLDOUT_PCT, DEFAULT_MIN_TRADES as DEFAULT_PICK_MIN_TRADES, HOLDOUT_PCT_RANGE,
};

/// The tokens a finished pick suggests for each list, as the table tags them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct SuggestedLists {
    pub(in crate::analytics::tuner) black: Vec<String>,
    pub(in crate::analytics::tuner) white: Vec<String>,
}

/// Parse the "fewer trades than" input: a whole number, blank meaning 0.
///
/// Args:
///     raw: The input's text.
///
/// Returns:
///     The threshold, or `None` for text that is not a non-negative whole number — the
///     caller then keeps the previous value.
pub(in crate::analytics::tuner) fn parse_min_trades(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(0);
    }
    raw.parse::<u32>().ok().map(i64::from)
}

/// The automatic pick's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct PickCfg {
    pub(in crate::analytics::tuner) min_trades: u32,
    pub(in crate::analytics::tuner) holdout_pct: u32,
    /// Suggest blacklist coins.
    pub(in crate::analytics::tuner) black: bool,
    /// Suggest whitelist coins.
    pub(in crate::analytics::tuner) white: bool,
}

impl Default for PickCfg {
    fn default() -> Self {
        Self {
            min_trades: DEFAULT_PICK_MIN_TRADES,
            holdout_pct: DEFAULT_HOLDOUT_PCT,
            black: true,
            white: false,
        }
    }
}

/// Where the automatic pick stands.
#[derive(Clone)]
pub(in crate::analytics::tuner) enum PickState {
    Idle,
    /// A pick request of generation `seq` is in flight.
    Running {
        seq: u64,
    },
    /// A finished pick, bound to the inputs it was published against. A later list edit
    /// or a later report publication leaves it standing but stale (`pick_stale`). A scope
    /// change (`invalidate`) drops it.
    Done {
        pick: Arc<CoinPick>,
        lists_rev: u64,
        /// `CoinsState::report_gen` at publication. A later report publication bumps
        /// that counter and the pick becomes stale without being dropped.
        report_gen: u64,
        /// The holdout share the pick ran with; the verdict names it even after the
        /// setting moved.
        holdout_pct: u32,
    },
    /// The pick's read failed.
    Failed(moon_core::db::ReadFail),
}

impl std::fmt::Debug for PickState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => f.write_str("Idle"),
            Self::Running { seq } => write!(f, "Running {{ seq: {seq} }}"),
            Self::Done {
                lists_rev,
                report_gen,
                holdout_pct,
                ..
            } => write!(
                f,
                "Done {{ lists_rev: {lists_rev}, report_gen: {report_gen}, holdout_pct: {holdout_pct} }}"
            ),
            Self::Failed(_) => f.write_str("Failed"),
        }
    }
}

/// Two states are equal when they are the same state of the same pick: a finished pick is
/// compared by identity, a failure by kind alone.
impl PartialEq for PickState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Idle, Self::Idle) | (Self::Failed(_), Self::Failed(_)) => true,
            (Self::Running { seq: a }, Self::Running { seq: b }) => a == b,
            (
                Self::Done {
                    pick: a,
                    lists_rev: ra,
                    report_gen: ga,
                    holdout_pct: ha,
                },
                Self::Done {
                    pick: b,
                    lists_rev: rb,
                    report_gen: gb,
                    holdout_pct: hb,
                },
            ) => Arc::ptr_eq(a, b) && ra == rb && ga == gb && ha == hb,
            _ => false,
        }
    }
}

/// The unsaved edit of both lists, per side, each part sorted by token.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct PendingDelta {
    pub(in crate::analytics::tuner) added_bl: Vec<String>,
    pub(in crate::analytics::tuner) removed_bl: Vec<String>,
    pub(in crate::analytics::tuner) added_wl: Vec<String>,
    pub(in crate::analytics::tuner) removed_wl: Vec<String>,
}

/// What the working lists come to when applied to a period: the KPI variants plus how
/// many coins each side actually REACHED in that period.
pub(in crate::analytics::tuner) struct CoinPlan {
    pub(in crate::analytics::tuner) variants: Vec<Variant>,
    pub(in crate::analytics::tuner) bl: usize,
    pub(in crate::analytics::tuner) wl: usize,
    /// The Auto variant's reach `(bl, wl)`, when a finished pick added that third column.
    pub(in crate::analytics::tuner) auto: Option<(usize, usize)>,
}

/// State of the "By coin" mode.
pub(in crate::analytics) struct CoinsState {
    /// Per-coin aggregates over the SELECTED strategies (all trades when nothing is
    /// selected) — the numbers each row shows.
    pub(in crate::analytics::tuner) stats: LoadState<Vec<GroupStat>>,
    /// Fact / Manual / Auto for the KPI matrix: Manual = the working lists applied, Auto = Manual plus the suggestion.
    pub(in crate::analytics::tuner) kpi: LoadState<Vec<VarStats>>,
    /// The list sizes the CURRENT `kpi` was computed for. The Manual column label reads these
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
    /// Hide coins with fewer trades than this (persisted in `WindowLayout::analytics_coins`).
    pub(in crate::analytics::tuner) min_trades: i64,
    /// Hide coins without a single trade (persisted beside `min_trades`).
    pub(in crate::analytics::tuner) hide_zero: bool,
    /// The automatic pick's settings (persisted beside `min_trades`).
    pub(in crate::analytics::tuner) pick_cfg: PickCfg,
    /// Where the automatic pick stands.
    pub(in crate::analytics::tuner) pick: PickState,
    /// Generation of the pick request; a reply carrying another one is dropped.
    pub(in crate::analytics::tuner) pick_seq: u64,
    /// Typing fields of the two list editors (lazily created, like the search input).
    pub(in crate::analytics::tuner) black_input: Option<Entity<MoonInputState>>,
    pub(in crate::analytics::tuner) white_input: Option<Entity<MoonInputState>>,
    /// The "fewer trades than" input of the filter bar (lazily created).
    pub(in crate::analytics::tuner) min_trades_input: Option<Entity<MoonInputState>>,
    /// What the last finished pick suggests; `None` while no pick has finished. Read only
    /// through [`CoinsState::suggested`].
    pub(in crate::analytics::tuner) suggestion: Option<SuggestedLists>,
    /// Bumped whenever `suggestion` changes; the row model is cached against it.
    pub(in crate::analytics::tuner) pick_rev: u64,
    /// The pick card's two number inputs (lazily created).
    pub(in crate::analytics::tuner) pick_min_input: Option<Entity<MoonInputState>>,
    pub(in crate::analytics::tuner) pick_holdout_input: Option<Entity<MoonInputState>>,
    /// The suggestion's reach the CURRENT `kpi` was scored under: `(bl, wl)` of the Auto
    /// column, `None` while the matrix has no Auto column.
    pub(in crate::analytics::tuner) kpi_auto: Option<(usize, usize)>,
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
    /// instead of rebuilding it over thousands of coins. A finished pick stores the value
    /// it was launched against; differing values make `pick_stale` true.
    pub(in crate::analytics::tuner) lists_rev: u64,
    /// Bumped on every report publication. A finished pick stores the value it was
    /// published under; `pick_stale` is true when they differ. Same binding as `lists_rev`.
    pub(in crate::analytics::tuner) report_gen: u64,
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
            report_gen: 0,
            search: String::new(),
            search_input: None,
            min_trades: 0,
            hide_zero: true,
            pick_cfg: PickCfg::default(),
            pick: PickState::Idle,
            pick_seq: 0,
            black_input: None,
            white_input: None,
            min_trades_input: None,
            suggestion: None,
            pick_rev: 0,
            pick_min_input: None,
            pick_holdout_input: None,
            kpi_auto: None,
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

/// Which input moved after a finished pick was published.
///
/// Lists wins when both moved: the user's own edit is the note that matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum PickStaleCause {
    /// The working lists were edited after the pick.
    Lists,
    /// Only the report data refreshed after the pick.
    Report,
}

impl CoinsState {
    /// Build coin-table state from its saved sort and settings.
    ///
    /// Args:
    ///     preference: Sort persisted in layout.toml (`table_sorts`); its direction uses
    ///         MoonUI's `ascending` convention.
    ///     settings: The axis' persisted filter and pick settings, if any.
    ///
    /// Returns:
    ///     Default coin state carrying the saved `(key, descending)` sort when the key is current
    ///     and the saved settings, clamped to their ranges.
    pub(in crate::analytics) fn load(
        preference: Option<moon_core::config::TableSortPreference>,
        settings: Option<&CoinsAxisLayout>,
    ) -> Self {
        let mut state = Self::default();
        if let Some(settings) = settings {
            state.restore(settings);
        }
        if let Some(preference) = preference.filter(|preference| {
            preference.column == super::super::SORT_NAME
                || COIN_COLS
                    .iter()
                    .any(|column| column.key == preference.column)
        }) {
            state.sort = Some((preference.column, !preference.ascending));
        }
        state
    }

    /// Mark report-derived coin statistics stale while preserving working lists and the pick.
    ///
    /// Report commits can change the table, KPI, and picked-strategy highlight. They cannot change
    /// the strategy fields that form `saved`/`work`, so clearing those drafts here would turn live
    /// refresh into data loss. A finished pick stays on screen, still adoptable, and
    /// [`Self::pick_stale`] becomes true. The note names the report, unless a list edit is
    /// also pending, in which case it names the lists. An in-flight
    /// pick is not retired either: the report commits every few seconds, and dropping it would
    /// mean a slow pick never lands. Its reply publishes bound to the generation it was launched
    /// under, so it arrives already stale. Only [`Self::invalidate`] drops the pick.
    ///
    /// The method has no return value; callers subsequently reload the active report-derived view.
    pub(in crate::analytics) fn mark_report_stale(&mut self) {
        self.dirty = true;
        self.report_gen = self.report_gen.wrapping_add(1);
    }

    /// Take the persisted settings of the axis (`WindowLayout::analytics_coins`); a missing
    /// field keeps its default and an out-of-range one is clamped.
    pub(in crate::analytics::tuner) fn restore(&mut self, saved: &CoinsAxisLayout) {
        let d = PickCfg::default();
        if let Some(n) = saved.min_trades {
            self.min_trades = i64::from(n);
        }
        if let Some(on) = saved.hide_zero {
            self.hide_zero = on;
        }
        self.pick_cfg = PickCfg {
            min_trades: saved.pick_min_trades.unwrap_or(d.min_trades),
            holdout_pct: saved.pick_holdout_pct.map_or(d.holdout_pct, |v| {
                v.clamp(*HOLDOUT_PCT_RANGE.start(), *HOLDOUT_PCT_RANGE.end())
            }),
            black: saved.pick_black.unwrap_or(d.black),
            white: saved.pick_white.unwrap_or(d.white),
        };
    }

    /// The axis' settings as they are written to the layout.
    pub(in crate::analytics::tuner) fn to_layout(&self) -> CoinsAxisLayout {
        CoinsAxisLayout {
            min_trades: Some(u32::try_from(self.min_trades.max(0)).unwrap_or(u32::MAX)),
            hide_zero: Some(self.hide_zero),
            pick_min_trades: Some(self.pick_cfg.min_trades),
            pick_holdout_pct: Some(self.pick_cfg.holdout_pct),
            pick_black: Some(self.pick_cfg.black),
            pick_white: Some(self.pick_cfg.white),
        }
    }

    /// Retire the current pick: a reply in flight is dropped and the card returns to Idle.
    ///
    /// The KPI matrix draws whatever column `kpi` still holds. Leaving the Auto column
    /// there after the suggestion is gone shows numbers for a pick the card no longer has,
    /// and a later reload may keep that snapshot when the next report arrives first. Clearing
    /// it here is what makes the matrix and the card agree on nothing.
    pub(in crate::analytics::tuner) fn drop_pick(&mut self) {
        self.pick_seq = self.pick_seq.wrapping_add(1);
        self.pick = PickState::Idle;
        if self.suggestion.take().is_some() {
            self.pick_rev = self.pick_rev.wrapping_add(1);
        }
        self.kpi_auto = None;
        if let Some(stats) = self.kpi.data_mut()
            && stats.len() > 2
        {
            stats.truncate(2);
        }
        self.settle_filter();
    }

    /// Start a pick: retire any earlier request and return the new generation.
    pub(in crate::analytics::tuner) fn begin_pick(&mut self) -> u64 {
        self.drop_pick();
        self.pick = PickState::Running { seq: self.pick_seq };
        self.pick_seq
    }

    /// Publish a pick reply.
    ///
    /// Args:
    ///     seq: The generation the request was launched under.
    ///     lists_rev: Working-list revision at launch.
    ///     report_gen: Report generation at launch. A publication during the run leaves
    ///         the landed pick stale instead of current.
    ///     holdout_pct: Holdout share the pick ran with.
    ///     result: The background read's answer.
    ///
    /// Returns:
    ///     Whether the reply landed; one whose generation was retired is dropped.
    pub(in crate::analytics::tuner) fn land_pick(
        &mut self,
        seq: u64,
        lists_rev: u64,
        report_gen: u64,
        holdout_pct: u32,
        result: moon_core::db::ReadResult<CoinPick>,
    ) -> bool {
        if seq != self.pick_seq || self.pick != (PickState::Running { seq }) {
            return false;
        }
        match result {
            Ok(pick) => {
                // An empty answer suggests nothing: no Auto variant is scored for it.
                if !pick.black.is_empty() || !pick.white.is_empty() {
                    self.suggestion = Some(SuggestedLists {
                        black: pick.black.iter().map(|c| c.token.clone()).collect(),
                        white: pick.white.iter().map(|c| c.token.clone()).collect(),
                    });
                    self.pick_rev = self.pick_rev.wrapping_add(1);
                }
                self.pick = PickState::Done {
                    pick: Arc::new(pick),
                    lists_rev,
                    report_gen,
                    holdout_pct,
                };
            }
            Err(e) => self.pick = PickState::Failed(e),
        }
        true
    }

    /// The finished pick no longer matches the inputs it was published against.
    ///
    /// Returns:
    ///     Whether [`Self::pick_stale_cause`] is `Some`. The card still shows the pick.
    ///     Adopting the suggestion still applies it; the note stays when the pick was already stale.
    pub(in crate::analytics::tuner) fn pick_stale(&self) -> bool {
        self.pick_stale_cause().is_some()
    }

    /// Which input moved after the finished pick was published.
    ///
    /// A list edit bumps `lists_rev`. A report publication bumps `report_gen`. Either one
    /// leaves the pick standing. When both moved, the cause is [`PickStaleCause::Lists`].
    ///
    /// Returns:
    ///     The cause the stale note should name, or `None` when the pick is current or not finished.
    pub(in crate::analytics::tuner) fn pick_stale_cause(&self) -> Option<PickStaleCause> {
        match &self.pick {
            PickState::Done {
                lists_rev,
                report_gen,
                ..
            } => {
                let lists = *lists_rev != self.lists_rev;
                let report = *report_gen != self.report_gen;
                if lists {
                    Some(PickStaleCause::Lists)
                } else if report {
                    Some(PickStaleCause::Report)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Apply the finished pick's own suggestion for one list to the working list, exactly as
    /// the Auto column scored it: blacklist coins are added, a suggested whitelist REPLACES
    /// the working one.
    ///
    /// Adopting the pick's own answer is not an edit the pick missed, so it is re-bound to the
    /// new list revision and shows no stale note — unless it was stale already.
    ///
    /// Returns:
    ///     How many coins were not on that list yet.
    pub(in crate::analytics::tuner) fn adopt_suggestion(&mut self, black_side: bool) -> usize {
        let Some(s) = self.suggestion.as_ref() else {
            return 0;
        };
        let tokens = if black_side { &s.black } else { &s.white };
        let raw = tokens.join(",");
        let was_current = !self.pick_stale();
        let added = if black_side {
            self.add_tokens(true, &raw)
        } else {
            let white: HashSet<String> = moon_core::symbol::parse_coin_list(&raw)
                .into_iter()
                .collect();
            let added = white.difference(&self.work.white).count();
            if white != self.work.white {
                self.work.white = white;
                self.lists_rev = self.lists_rev.wrapping_add(1);
                self.settle_filter();
            }
            added
        };
        if was_current && let PickState::Done { lists_rev, .. } = &mut self.pick {
            *lists_rev = self.lists_rev;
        }
        added
    }

    /// What the last finished pick suggests, for the Auto column and the Suggested view.
    pub(in crate::analytics::tuner) fn suggested(&self) -> Option<&SuggestedLists> {
        self.suggestion.as_ref()
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
        self.drop_pick();
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
        // Bumped only when something ACTUALLY moved. `reload_active_tuner` fires both
        // immediately after a bulk write and again once its edit-resolution watch settles, and
        // an unconditional bump made those app-scheduled reloads look like a user edit — which
        // silently dropped a second Save clicked inside that window, and rebuilt the field for
        // nothing on every one of them.
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
        let set = self.work_side(black_side);
        if !set.remove(&token) {
            set.insert(token);
        }
        // The view the user is on may no longer have anything behind it.
        self.settle_filter();
    }

    /// Add every coin named in `raw` (typed or pasted, any separator) to one working list.
    ///
    /// Args:
    ///     black_side: `true` for the blacklist, `false` for the whitelist.
    ///     raw: Free text in the strategy field's own syntax.
    ///
    /// Returns:
    ///     How many coins were not on that list yet.
    pub(in crate::analytics::tuner) fn add_tokens(&mut self, black_side: bool, raw: &str) -> usize {
        let set = self.work_side(black_side);
        let added = moon_core::symbol::parse_coin_list(raw)
            .into_iter()
            .filter(|token| token.chars().count() <= MAX_TOKEN_LEN)
            .filter(|token| set.insert(token.clone()))
            .count();
        if added > 0 {
            self.lists_rev = self.lists_rev.wrapping_add(1);
            self.settle_filter();
        }
        added
    }

    /// Take one coin off a working list (the chip's x).
    pub(in crate::analytics::tuner) fn remove_token(&mut self, black_side: bool, token: &str) {
        if self.work_side(black_side).remove(token) {
            self.lists_rev = self.lists_rev.wrapping_add(1);
            self.settle_filter();
        }
    }

    /// Undo one removal: put a coin back on its working list.
    pub(in crate::analytics::tuner) fn restore_token(&mut self, black_side: bool, token: &str) {
        if !token.is_empty() && self.work_side(black_side).insert(token.to_string()) {
            self.lists_rev = self.lists_rev.wrapping_add(1);
            self.settle_filter();
        }
    }

    /// Empty one working list; the saved list stays the baseline, so Revert brings it back.
    pub(in crate::analytics::tuner) fn clear(&mut self, black_side: bool) {
        let set = self.work_side(black_side);
        if !set.is_empty() {
            set.clear();
            self.lists_rev = self.lists_rev.wrapping_add(1);
            self.settle_filter();
        }
    }

    /// The pending edit per side, each part sorted by token.
    pub(in crate::analytics::tuner) fn pending_delta(&self) -> PendingDelta {
        fn sorted<'a>(it: impl Iterator<Item = &'a String>) -> Vec<String> {
            let mut v: Vec<String> = it.cloned().collect();
            v.sort();
            v
        }
        PendingDelta {
            added_bl: sorted(self.work.black.difference(&self.saved.black)),
            removed_bl: sorted(self.saved.black.difference(&self.work.black)),
            added_wl: sorted(self.work.white.difference(&self.saved.white)),
            removed_wl: sorted(self.saved.white.difference(&self.work.white)),
        }
    }

    fn work_side(&mut self, black_side: bool) -> &mut HashSet<String> {
        if black_side {
            &mut self.work.black
        } else {
            &mut self.work.white
        }
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
            CoinFilter::Suggested => self
                .suggested()
                .is_some_and(|s| !s.black.is_empty() || !s.white.is_empty()),
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

    /// Variants for the KPI matrix: `[Fact, Manual]`, plus Auto when a pick suggests anything.
    ///
    /// Manual is the counterfactual the mode exists to answer — the same trades WITH these
    /// lists applied: keep only whitelisted coins (when a whitelist exists), then drop the
    /// blacklisted ones, exactly as a strategy evaluates its two lists.
    ///
    /// The lists are tokens while the scan filters raw report names, so they are expanded
    /// here against `universe` — the very coin set the table draws from. A token naming no
    /// traded coin contributes nothing rather than becoming an empty restriction.
    pub(in crate::analytics::tuner) fn plan(&self, universe: &[GroupStat]) -> CoinPlan {
        CoinPlan::build(&self.work, self.suggested(), universe)
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
        auto: Option<&SuggestedLists>,
        universe: &[GroupStat],
    ) -> CoinPlan {
        let work = fresh_saved
            .duplicate()
            .with_delta(previous_saved, previous_work);
        CoinPlan::build(&work, auto, universe)
    }

    /// Build the plan from BARE lists, so the background reload can score the lists it just
    /// read without waiting for them to reach the view first.
    ///
    /// Variants: `[Fact, Manual]`, plus `Auto` while a finished pick suggests something —
    /// the working lists with the suggestion applied: its blacklist added, its whitelist (when
    /// it has one) in place of the working one.
    ///
    /// Args:
    ///     work: The working lists (Manual).
    ///     auto: The finished pick's suggestion, if any.
    ///     universe: Report coin rows used to expand list tokens.
    ///
    /// Returns:
    ///     The variants and the coins each list reached.
    pub(in crate::analytics::tuner) fn build(
        work: &CoinLists,
        auto: Option<&SuggestedLists>,
        universe: &[GroupStat],
    ) -> CoinPlan {
        // Manual ALWAYS exists — it is the column the whole mode is read against, and a matrix
        // that grows a column the moment the first coin is ticked shifts every number under
        // it. Empty, it restricts nothing and simply reproduces the fact.
        let mut plan = CoinPlan {
            variants: vec![Variant::default(), Variant::default()],
            bl: 0,
            wl: 0,
            auto: None,
        };
        let auto_lists = auto.map(|s| {
            let mut lists = work.duplicate();
            lists.black.extend(s.black.iter().cloned());
            if !s.white.is_empty() {
                lists.white = s.white.iter().cloned().collect();
            }
            lists
        });
        // An EMPTY universe is "the coin set is not known yet", not "no coin qualifies":
        // planning against it would turn a live whitelist into `Some(empty)` — the "keeps
        // nothing" case — and publish a column of zero trades that no list produced.
        if universe.is_empty() {
            return plan;
        }
        (plan.variants[1], plan.bl, plan.wl) = expand(work, universe);
        if let Some(lists) = auto_lists {
            let (variant, bl, wl) = expand(&lists, universe);
            plan.variants.push(variant);
            plan.auto = Some((bl, wl));
        }
        plan
    }
}

/// Expand list tokens against the coin set into a scan variant, plus how many coins each list
/// actually REACHED in the period (a list may name coins that never traded, and labelling a
/// column with them would claim a restriction the numbers were not scored under).
fn expand(lists: &CoinLists, universe: &[GroupStat]) -> (Variant, usize, usize) {
    if lists.is_empty() {
        return (Variant::default(), 0, 0);
    }
    let white_on = !lists.white.is_empty();
    let (mut coins_in, mut coins_out) = (Vec::new(), Vec::new());
    let (mut bl_tok, mut wl_tok) = (HashSet::new(), HashSet::new());
    for g in universe {
        if g.name.is_empty() {
            continue;
        }
        let token = coin_token(&g.name);
        if white_on && lists.white.contains(&token) {
            coins_in.push(g.name.clone());
            wl_tok.insert(token.clone());
        }
        if lists.black.contains(&token) {
            coins_out.push(g.name.clone());
            bl_tok.insert(token);
        }
    }
    // Stable order: the variant is part of a background request's identity.
    coins_in.sort();
    coins_out.sort();
    let variant = Variant {
        // `Some` whenever a whitelist is switched on, even when nothing matched: that case
        // keeps NO trades, and an empty `Vec` would read as "no whitelist".
        coins_in: white_on.then_some(coins_in),
        coins_out,
        ..Default::default()
    };
    (variant, bl_tok.len(), wl_tok.len())
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own
// `test` shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
