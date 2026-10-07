//! The variant column and the search of the "Entry/Exit" axis: the edits behind В1, their
//! debounced rescore over the replayable rows, the search that fills В1 over the grids the
//! ranges resolve to (`ranges.rs`), and the write of В1 through the shared confirmation dialog.
//!
//! Every score here is a replay — `variant_tally` over the fit rows whose tape is in memory
//! (`TicksData::replayable`) — so the columns describe the SAME subset the "Fact · fit" column
//! describes, never the whole scope. The captions say "by N" for that reason.
//!
//! A variant's values are laid over each deal's OWN strategy as it stands now
//! (`TicksData::own`), not over what the selected strategies agree on: a field they disagree on
//! and the variant leaves alone runs every deal at its strategy's value.
//!
//! A replay leans on what each trade's own record proves wherever a variant keeps the trade's
//! own settings: the entry fills where the report says, and the stop fires when and where the
//! core's did (`record::StopAnchor`) — the book the tape does not carry, answered by the fact.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::*;
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::state::{CutCache, CutKey, SuggState};
use super::tape::{PendingDeal, prepare_sample};
use crate::analytics::bg::ReadLane;
use moon_core::db::tuner::VarStats;
use moon_core::db::tuner::threshold_search::SearchHandle;
use moon_core::db::tuner::ticks::params::range::Grids;
use moon_core::db::tuner::ticks::params::{self, ParamGroup};
use moon_core::db::tuner::ticks::search::{
    DEFAULT_MAX_PASSES, MIN_HOLDOUT, MIN_SEARCH_DEALS, SearchMiss, SearchParams, Suggested,
    check_corridors, comparable, default_min_n, sample_floor, suggest_with_candidate, train_len,
    variant_tally_by_deal,
};
use moon_core::db::tuner::ticks::{fact_stats, stats_of};

mod probe;
pub(super) use probe::painted as probe_painted;

#[cfg(test)]
mod tests;

/// How long a burst of cell edits may keep coalescing before the columns are rescored.
const VARIANT_DEBOUNCE: Duration = Duration::from_millis(350);

/// How often a running search's row is looked at for a moved restart count.
const SEARCH_POLL: Duration = Duration::from_millis(250);

/// Restarts the search runs with, out of the box's text: the default when empty or
/// unreadable, clamped to a sane range.
pub(super) fn restarts_of(text: &str) -> usize {
    text.trim()
        .parse::<usize>()
        .unwrap_or(DEFAULT_RESTARTS)
        .clamp(1, 10_000)
}

/// One variant scored over the comparable deal set, with the fact over that same set.
struct Scored {
    stats: VarStats,
    plan: HashMap<i64, (f64, f64)>,
    open: usize,
    open_profit: f64,
    untraded: usize,
    n: usize,
    /// The cut the score was taken over, fresh or reused.
    cut: CutCache,
}

/// A variant's score over the kept deals of `cut`, as the column takes it.
fn scored_of(
    score: moon_core::db::tuner::ticks::search::VariantScore,
    money: moon_core::db::tuner::ticks::search::DealResults,
    n: usize,
    cut: CutCache,
) -> Scored {
    let plan: HashMap<i64, (f64, f64)> = money
        .into_iter()
        .filter_map(|(uid, value)| Some((uid, value?)))
        .collect();
    Scored {
        stats: stats_of(score.tally, score.spent),
        plan,
        open: score.open,
        open_profit: score.open_profit,
        untraded: score.untraded,
        n,
        cut,
    }
}

/// Restarts when the box is empty.
pub(super) const DEFAULT_RESTARTS: usize = 20;

/// Passes per restart the search runs with, out of the box's text: the search's default when
/// empty or unreadable, clamped to a sane range.
pub(super) fn passes_of(text: &str) -> usize {
    text.trim()
        .parse::<usize>()
        .unwrap_or(DEFAULT_MAX_PASSES)
        .clamp(1, 1_000)
}

impl AnalyticsView {
    /// The replayable rows as the search and the columns take them, their tapes still packed:
    /// the caller hands them to [`prepare_sample`] off the UI thread, which unpacks them and
    /// cuts every tape at the sample's one exit horizon.
    pub(super) fn prepared_deals(&self) -> Vec<PendingDeal> {
        self.ticks
            .data
            .data()
            .map(|d| d.replayable().filter_map(|row| d.prepared(row)).collect())
            .unwrap_or_default()
    }

    /// Arm a debounced rescore of the variant column — every edit of a cell, every row that
    /// joins the replayable set, goes through here.
    pub(in crate::analytics::tuner) fn arm_ticks_variants(&mut self, cx: &mut Context<Self>) {
        if !self.ticks.tape_reading
            && self.ticks.data.data().is_some_and(|d| d.fit() > 0)
            && probe::fire()
        {
            probe::dump_schema(self.backend.read(cx).session.store());
            self.ticks_start_search(None, cx);
        }
        self.latest_reads.cancel(&[ReadLane::TicksVariants]);
        self.ticks.var_seq = self.ticks.var_seq.wrapping_add(1);
        // Nothing to score: an untouched column costs no clone of the rows and no replay — a
        // fetch over hundreds of rows re-arms this once per row.
        if self.ticks.variant_changes().is_empty() {
            self.ticks.clear_var_score();
            self.set_ticks_plan(HashMap::new());
            // The trade pane's modelled trades go with the columns.
            self.ticks_refresh_model_trades(cx);
            return;
        }
        let req = self.ticks.var_seq;
        self.ticks.var_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(VARIANT_DEBOUNCE).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.ticks.var_seq == req {
                        this.run_ticks_variants(req, cx);
                    }
                });
            });
        }));
    }

    /// Score the touched variant over the replayable rows the strategies as they stand close
    /// (`search::comparable`), and the fact over the same rows: one deal set for both columns.
    fn run_ticks_variants(&mut self, req: u64, cx: &mut Context<Self>) {
        let pending = self.prepared_deals();
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let kind = data.single_kind().unwrap_or_default().to_string();
        let values = self.ticks.variant_changes();
        let defaults = self.filter_defaults(cx);
        let model = super::model_cfg::current();
        let mut sorted_defaults: Vec<(String, f64)> =
            defaults.iter().map(|(k, v)| (k.clone(), *v)).collect();
        sorted_defaults.sort_by(|a, b| a.0.cmp(&b.0));
        let key = CutKey {
            rows_rev: self.ticks.rows_rev,
            deals: pending
                .iter()
                .map(|p| (p.deal.report_uid, p.trail_ms))
                .collect(),
            kind: kind.clone(),
            model,
            defaults: sorted_defaults,
        };
        let cached = self.ticks.cut_cache.clone().filter(|c| c.key == key);
        self.spawn_latest_db(
            &[ReadLane::TicksVariants],
            false,
            cx,
            move || {
                let deals = prepare_sample(pending);
                if values.is_empty() || deals.is_empty() {
                    return None;
                }
                // The cut depends on the sample alone: a cached one is reused as it is.
                if let Some(cut) = cached {
                    let deals: Vec<_> = deals
                        .into_iter()
                        .filter(|d| cut.kept.contains(&d.deal.report_uid))
                        .collect();
                    let (score, money) =
                        variant_tally_by_deal(&deals, &defaults, &kind, &values, model);
                    return Some(scored_of(score, money, deals.len(), cut));
                }
                // The base is the strategies as they stand: nothing held, nothing searched.
                let (held, locked, grids) = (HashMap::new(), Default::default(), Grids::default());
                let base = SearchParams {
                    held: &held,
                    defaults: &defaults,
                    kind: &kind,
                    vary_entry: false,
                    vary_exit: false,
                    locked: &locked,
                    grids: &grids,
                    restarts: 1,
                    min_n: None,
                    seed: None,
                    train_frac: 1.0,
                    max_passes: 1,
                    model,
                    keep_corridor: false,
                    risk: Default::default(),
                    screen_entry: false,
                };
                let (deals, base_open) = comparable(&deals, &base);
                let cut = CutCache {
                    key,
                    kept: Arc::new(deals.iter().map(|d| d.deal.report_uid).collect()),
                    base_open,
                    fact: fact_stats(deals.iter().map(|d| &d.deal)),
                };
                let (score, money) =
                    variant_tally_by_deal(&deals, &defaults, &kind, &values, model);
                Some(scored_of(score, money, deals.len(), cut))
            },
            move |this, scored, cx| {
                if this.ticks.var_seq != req {
                    return;
                }
                this.ticks.clear_var_score();
                match scored {
                    Some(s) => {
                        this.ticks.var_stats = Some(s.stats);
                        this.ticks.var_open = s.open;
                        this.ticks.var_open_profit = s.open_profit;
                        this.ticks.var_untraded = s.untraded;
                        this.ticks.set_fact = Some(s.cut.fact.clone());
                        this.ticks.base_open = s.cut.base_open;
                        this.ticks.var_n = s.n;
                        this.ticks.cut_cache = Some(s.cut);
                        this.set_ticks_plan(s.plan);
                    }
                    None => this.set_ticks_plan(HashMap::new()),
                }
                // The trade pane draws what the columns now count.
                this.ticks_refresh_model_trades(cx);
                cx.notify();
            },
        );
    }

    /// Take the variant's per-deal results; a table sorted by the plan column is re-sorted.
    fn set_ticks_plan(&mut self, plan: HashMap<i64, (f64, f64)>) {
        self.ticks.plan = plan;
        if self
            .ticks
            .sort
            .as_ref()
            .is_some_and(|(key, _)| key == super::columns::COL_PLAN)
        {
            self.ticks.order = None;
        }
    }

    /// One cell of the variant changed: store it and rescore.
    pub(in crate::analytics::tuner) fn set_ticks_variant(
        &mut self,
        key: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        self.ticks.set_variant(key, value);
        self.arm_ticks_variants(cx);
    }

    /// Clear the variant column.
    pub(in crate::analytics::tuner) fn ticks_clear_variant(&mut self, cx: &mut Context<Self>) {
        self.ticks.variant.clear();
        self.ticks.clear_var_score();
        self.ticks.plan.clear();
        self.ticks_reset_variant_inputs();
        self.arm_ticks_variants(cx);
        cx.notify();
    }

    /// Drop the input boxes of the variant column so they are recreated from the stored values.
    fn ticks_reset_variant_inputs(&mut self) {
        self.ticks
            .inputs
            .retain(|id, _| !id.starts_with(super::grid::VARIANT_INPUT_PREFIX));
    }

    /// Whether a group may be searched ([`super::state::TicksData::group_searchable`]): the
    /// kind's support and a reproduced trade to learn from — the share gate only warns.
    pub(in crate::analytics::tuner) fn ticks_group_searchable(&self, group: ParamGroup) -> bool {
        self.ticks
            .data
            .data()
            .is_some_and(|data| data.group_searchable(group))
    }

    /// The tooltips of "Search" and "Search all": what each varies and where its answer lands —
    /// the selected field by name, the number of ticked fields of the searchable groups, and on
    /// both the groups searched under the share gate ([`Self::ticks_gate_warnings`]) — the grid
    /// heading that says so can be scrolled away by the time the button is pressed. A scope of
    /// several kinds, which neither searches, says so on both.
    pub(super) fn ticks_search_tips(&self) -> (String, String) {
        let data = self.ticks.data.data();
        if data.is_some_and(|d| !d.kinds.is_empty() && d.single_kind().is_none()) {
            let refused = t!("analytics.ticks.sugg_one_kind").to_string();
            return (refused.clone(), refused);
        }
        if let Some(n) = self.ticks.search_too_few() {
            let refused = miss_note(SearchMiss::TooFew { n }, 0);
            return (refused.clone(), refused);
        }
        let one = match self.ticks.sel_field {
            Some(key) => t!("analytics.ticks.suggest_one_tip", field = key).to_string(),
            None => t!("analytics.ticks.suggest_one_none").to_string(),
        };
        let entry_on = data.is_some_and(|d| d.entry_modelled());
        let ticked = data.map_or(0, |d| {
            // Once per group, not per knob: `group_searchable` counts the fit rows, and the
            // tooltips are built on every paint of the search row.
            let (entry_ok, exit_ok) = (
                d.group_searchable(ParamGroup::Entry),
                d.group_searchable(ParamGroup::Exit),
            );
            d.grid
                .iter()
                .flat_map(super::sections::GridSection::knobs)
                .filter(|k| super::grid::knob_ticks(k, entry_on))
                .filter(|k| !self.ticks.locked.contains(k.key))
                .filter(|k| match k.group {
                    ParamGroup::Entry => entry_ok,
                    ParamGroup::Exit => exit_ok,
                })
                .count()
        });
        let all = t!("analytics.ticks.suggest_all_tip", n = ticked).to_string();
        let gated = self.ticks_gate_warnings([ParamGroup::Entry, ParamGroup::Exit]);
        if gated.is_empty() {
            return (one, all);
        }
        let with = |tip: String| format!("{tip}\n\n{}", gated.join("\n"));
        (with(one), with(all))
    }

    /// Per group of `groups` the share of deals the model reproduces (not-judged ones counted
    /// against), plus a line for a group under the share gate, none reproduced included, and one
    /// when the last search's point loses to the fact out of sample. The search
    /// learns on the fit trades alone (`TicksData::under_gate`). The search's tooltips and the
    /// write dialogs carry it — the gate no longer locks such a group out, so a variant searched
    /// on a small share of the fact can reach a live strategy, and the dialog is the last place
    /// to say so.
    fn ticks_gate_warnings(&self, groups: impl IntoIterator<Item = ParamGroup>) -> Vec<String> {
        let Some(data) = self.ticks.data.data() else {
            return Vec::new();
        };
        let gate = self.ticks.gate();
        let mut seen = Vec::new();
        let mut warns: Vec<String> = groups
            .into_iter()
            .filter(|group| {
                let first = !seen.contains(group);
                seen.push(*group);
                first
            })
            .flat_map(|group| {
                let name = match group {
                    ParamGroup::Entry => t!("analytics.ticks.group_entry"),
                    ParamGroup::Exit => t!("analytics.ticks.group_exit"),
                };
                // What the search answers for, always — not-judged deals count against it.
                let modelled = group != ParamGroup::Entry || data.entry_modelled();
                let (hits, n) = data.share_of(group);
                let answers = (modelled && n > 0).then(|| {
                    format!(
                        "{name}: {}",
                        t!("analytics.ticks.answers_for", hits = hits, n = n)
                    )
                });
                let under = data.under_gate(group, gate).map(|(hits, n)| {
                    t!(
                        "analytics.ticks.gate_warn",
                        group = name,
                        hits = hits,
                        n = n,
                        gate = (gate * 100.0).round() as i64
                    )
                    .to_string()
                });
                answers.into_iter().chain(under)
            })
            .collect();
        // Only while the column still is the search's point: a hand edit since is some other.
        if let Some(r) = self.ticks.current_result() {
            if r.holdout_loses {
                warns.push(t!("analytics.ticks.warn_holdout_loses").to_string());
            }
            if r.holdout.as_ref().is_none_or(|h| h.n < MIN_HOLDOUT) {
                warns.push(t!("analytics.ticks.warn_whole_period").to_string());
            }
        }
        warns
    }

    /// The groups a variant's changes move — the gate's warning at write time is about them.
    fn changed_groups(changes: &[(String, String)]) -> Vec<ParamGroup> {
        changes
            .iter()
            .filter_map(|(key, _)| {
                moon_core::db::tuner::ticks::TICK_PARAMS
                    .iter()
                    .find(|f| f.key == key.as_str())
                    .map(|f| f.group)
            })
            .collect()
    }

    /// "Search all": every ticked field of the searchable groups, each from the
    /// strategies, the unticked ones held at В1's value; the answer goes into В1
    /// ([`land_answer`]).
    pub(in crate::analytics::tuner) fn ticks_suggest(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ticks_run_search(None, window, cx);
    }

    /// "Search": the selected field alone, ticked or not, from the strategies, the rest of В1 held
    /// as it stands; the answer goes into that cell of В1 — emptied when it is the strategies'
    /// own — and the cells of the values it completed ([`land_answer`]).
    pub(in crate::analytics::tuner) fn ticks_suggest_one(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self.ticks.sel_field {
            self.ticks_run_search(Some(key), window, cx);
        }
    }

    /// A search asked for: first the question when it is estimated long (`estimate.rs`), then
    /// the warning when a strategy it runs on switches on an exit field the model does not have
    /// (`unmodelled.rs`) — the search then starts on their answers.
    fn ticks_run_search(
        &mut self,
        only: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.ticks.sugg, SuggState::Running { .. }) {
            return;
        }
        if !self.ticks_confirm_long_search(only, window, cx) {
            self.ticks_search_confirmed(only, window, cx);
        }
    }

    /// A search past the question of its length: the warning, then the start.
    pub(super) fn ticks_search_confirmed(
        &mut self,
        only: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ticks_warn_before_search(only, window, cx) {
            self.ticks_start_search(only, cx);
        }
    }

    /// Say why a search did not start.
    fn ticks_search_refused(&mut self, key: &str, cx: &mut Context<Self>) {
        log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks search: refused, {key}, shares entry {:?} exit {:?}",
            self.ticks.data.data().map(|d| d.entry_share),
            self.ticks.data.data().map(|d| d.exit_share)
        );
        self.ticks.sugg_note = Some(t!(key).to_string());
        cx.notify();
    }

    /// Run the search into В1: over every ticked field (`only` = `None`), or over one field with
    /// every other held — at the strategies' value, or at В1's where В1 changes it. A searched
    /// field starts from the strategies whatever В1 holds for it.
    pub(super) fn ticks_start_search(
        &mut self,
        only: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.ticks.sugg, SuggState::Running { .. }) {
            return;
        }
        let pending = self.prepared_deals();
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let Some(kind) = data.single_kind().map(String::from) else {
            return self.ticks_search_refused("analytics.ticks.sugg_one_kind", cx);
        };
        let model = super::model_cfg::current();
        // Laid over every deal's own strategy before the search's point: В1 as it stands. The
        // fields the search leaves alone run at В1's value; the searched ones start from the
        // strategies whatever В1 holds for them — the search sets their values aside
        // (`SearchParams::held`, LinKvo 2026-09-25).
        let held: HashMap<String, String> = self.ticks.variant_changes().into_iter().collect();
        let super::estimate::Scope {
            vary_entry,
            vary_exit,
            locked,
        } = match self.ticks_search_scope(only) {
            Ok(scope) => scope,
            Err(Some(key)) => return self.ticks_search_refused(key, cx),
            Err(None) => return,
        };
        if pending.is_empty() {
            return self.ticks_search_refused("analytics.ticks.sugg_no_tape", cx);
        }
        if let Err(miss) = sample_floor(pending.len()) {
            self.ticks.sugg_note = Some(miss_note(miss, 0));
            cx.notify();
            return;
        }
        let defaults = self.filter_defaults(cx);
        let restarts = probe::restarts().unwrap_or_else(|| restarts_of(&self.ticks.iters));
        let max_passes = passes_of(&self.ticks.passes);
        let seed = self.ticks.seed.trim().parse::<u64>().ok();
        let min_n = self
            .ticks
            .min_trades
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0);
        let train_frac = super::super::filter::state::train_frac(self.ticks.train_pct);
        let keep_corridor = self.ticks.keep_corridor;
        let risk = self.ticks.risk_limits();
        let screen_entry = self.ticks.screen_entry;
        // The grids are resolved now, from the ranges as they stand: a range edited while the
        // search runs is the next search's.
        let (grids, set_aside) = self.ticks_search_grids();
        // A floor over the slice the search fits on no point can keep: say so before a run that
        // can only come back empty. Counted on every replayable row: the search then drops the
        // deals the strategies as they stand leave open (`closing::closable_at_base`), so a floor
        // this lets through can still fail there, and the search says so itself.
        let closes: Vec<i64> = pending.iter().map(|d| d.deal.close_ms).collect();
        let train_n = train_len(&closes, train_frac);
        if let Some(n) = min_n.filter(|n| *n > train_n as i64) {
            self.ticks.sugg_note =
                Some(t!("analytics.ticks.sugg_floor_sample", n = n, m = train_n).to_string());
            cx.notify();
            return;
        }
        let handle = SearchHandle::new();
        self.ticks.sugg = SuggState::Running {
            handle: handle.clone(),
            total: restarts,
        };
        self.ticks.sugg_seq = self.ticks.sugg_seq.wrapping_add(1);
        // A typed range the search set aside for the automatic one is said, not swallowed.
        self.ticks.sugg_note = (!set_aside.is_empty()).then(|| {
            t!(
                "analytics.ticks.range_set_aside",
                fields = set_aside.join(", ")
            )
            .to_string()
        });
        let seq = self.ticks.sugg_seq;
        log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks search: start #{seq}, {restarts} restart(s) x {max_passes} pass(es) over {} deal(s), entry {vary_entry}, exit {vary_exit}, {} steps per field, typed ranges set aside {set_aside:?}",
            pending.len(),
            self.ticks.steps_per_param()
        );
        probe::watch(handle.clone(), restarts, seq);
        self.poll_ticks_search(handle.clone(), seq, cx);
        let started = std::time::Instant::now();
        // What the point cost the answer brings is measured under (`estimate.rs`).
        let cost_key = self.ticks_cost_key();
        self.spawn_latest_db(
            &[ReadLane::TicksSearch],
            false,
            cx,
            move || {
                let deals = prepare_sample(pending);
                let params = SearchParams {
                    held: &held,
                    defaults: &defaults,
                    kind: &kind,
                    vary_entry,
                    vary_exit,
                    locked: &locked,
                    grids: &grids,
                    restarts,
                    min_n,
                    seed,
                    train_frac,
                    max_passes,
                    model,
                    keep_corridor,
                    risk,
                    screen_entry,
                };
                // The search alone is timed for the point cost: the queue and the unpacking of
                // the tapes above are no point's.
                let searching = std::time::Instant::now();
                // The floor holds over the set the search scored: the deals the strategies as
                // they stand leave open are cut from it first.
                let Suggested {
                    result,
                    candidate,
                    searched,
                } = suggest_with_candidate(&deals, &params, &handle);
                let result = result.and_then(|found| {
                    sample_floor(deals.len().saturating_sub(found.stats.left_open))
                        .map(|()| found)
                });
                // A candidate is laid over В1 only beside a result the sample may be searched for.
                let candidate = candidate.filter(|_| !matches!(result, Err(SearchMiss::TooFew { .. })));
                (result, candidate, searched, searching.elapsed())
            },
            move |this, (result, candidate, searched, searched_for), cx| {
                log::info!(
                    target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                    "[x] ticks search: #{seq} answered after {} ms ({}), current #{}",
                    started.elapsed().as_millis(),
                    match &result {
                        Ok(found) => format!(
                            "found {:?}, train {:.4} vs fact {:.4}, holdout {:?} vs fact {:?}, holdout_loses {}",
                            found.values,
                            found.train.profit,
                            found.fact_train.profit,
                            found.holdout.as_ref().map(|h| h.profit),
                            found.fact_holdout.as_ref().map(|f| f.profit),
                            found.holdout_loses
                        ),
                        Err(miss) => format!("nothing: {miss:?}"),
                    },
                    this.ticks.sugg_seq
                );
                if this.ticks.sugg_seq != seq {
                    return;
                }
                this.ticks.sugg = SuggState::Idle;
                // В1 as the search found it: a candidate is laid over this, never over the
                // answer, whose completed values (a `TakeProfit` its `UseTakeProfit` brought) sit
                // outside the searched fields and would stay behind in the candidate's column.
                let before = this.ticks.variant.clone();
                match result {
                    Ok(result) => {
                        this.ticks_take_search_cost(
                            cost_key,
                            searched_for,
                            result.stats.evaluations,
                            result.stats.fills_reused,
                        );
                        // The answer's own В1: what its holdout verdict speaks for
                        // (`current_result`). A candidate laid over В1 below is another point, so
                        // the verdict stays off the column while it holds the candidate.
                        let mut answered = this.ticks.variant.clone();
                        land_answer(&mut answered, &result.searched, &result.values);
                        this.ticks.result_variant = super::state::changes_of(&answered);
                        this.ticks.variant = answered;
                        this.ticks.last_seed = Some(result.seed);
                        this.ticks.last_result = Some(result);
                    }
                    // Why nothing: the floor no point kept — the typed one, or the search's own
                    // half of the training slice (`default_min_n`) —, the corridor none kept, no
                    // point that closed every trade it bought, or nothing at all. The previous
                    // answer's verdict no longer speaks for the column either.
                    Err(miss) => {
                        this.ticks.last_result = None;
                        let floor = min_n.unwrap_or_else(|| default_min_n(train_n));
                        this.ticks.sugg_note = Some(miss_note(miss, floor));
                    }
                }
                // The point that earns more on the deals it closed but leaves some open goes
                // into В1 in place of the answer (LinKvo, 2026-10-07): its values in the cells,
                // and the column scores it like any variant — the trades it closed, "by N of M",
                // the deals it left open and what they make at the tape's end.
                if let Some(candidate) = &candidate {
                    let mut laid = before;
                    land_answer(&mut laid, &searched, &candidate.values);
                    this.ticks.variant = laid;
                }
                if this.ticks.last_result.is_some() || candidate.is_some() {
                    this.ticks_reset_variant_inputs();
                    this.arm_ticks_variants(cx);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Repaint the search row while the run it follows goes on, each time its restart count
    /// moves — nothing else repaints a quiet window, and the count would stand still for the
    /// whole run. Ends when the run finished or was replaced (`sugg_seq`); a window closed under
    /// it stops the run, which has nobody left to answer.
    fn poll_ticks_search(&self, handle: SearchHandle, seq: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut shown = None;
            loop {
                executor.timer(SEARCH_POLL).await;
                let mut running = false;
                let view_gone = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        // The answer sets the row idle without a new generation.
                        running = this.ticks.sugg_seq == seq
                            && matches!(this.ticks.sugg, SuggState::Running { .. });
                        // A search of both groups moves its entry points long before a restart.
                        let done = (handle.completed(), handle.points());
                        if running && shown != Some(done) {
                            shown = Some(done);
                            cx.notify();
                        }
                    })
                    .is_err()
                });
                if view_gone {
                    handle.cancel();
                    return;
                }
                if !running {
                    return;
                }
            }
        })
        .detach();
    }

    /// Stop a running search; what it found so far is dropped.
    pub(in crate::analytics::tuner) fn ticks_stop_suggest(&mut self, cx: &mut Context<Self>) {
        self.ticks.stop_search("Stop");
        cx.notify();
    }

    /// "Save": write В1's changes to the selected strategies through the shared dialog.
    pub(in crate::analytics::tuner) fn ticks_open_save_dialog(&mut self, cx: &mut Context<Self>) {
        let targets = self.selected_targets();
        if targets.is_empty() {
            return;
        }
        let changes = self.ticks.variant_changes();
        if changes.is_empty() {
            log::info!("analytics: 'Save' (ticks) - no variant to write");
            return;
        }
        let mut warns = self.ticks_change_warnings(&changes, cx);
        warns.extend(self.ticks_unguarded_warning(&targets, &changes, cx));
        warns.extend(self.ticks_unmodelled_warns(&targets, cx));
        warns.extend(self.ticks_gate_warnings(Self::changed_groups(&changes)));
        self.open_change_dialog(targets, changes, None, Vec::new(), warns, false, cx);
    }

    /// "Make a copy": a new strategy with В1's changes.
    pub(in crate::analytics::tuner) fn ticks_open_copy_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.selected_targets().into_iter().next() else {
            return;
        };
        let changes = self.ticks.variant_changes();
        let mut warns = self.ticks_change_warnings(&changes, cx);
        warns.extend(self.ticks_unguarded_warning(std::slice::from_ref(&target), &changes, cx));
        warns.extend(self.ticks_unmodelled_warns(std::slice::from_ref(&target), cx));
        warns.extend(self.ticks_gate_warnings(Self::changed_groups(&changes)));
        self.open_copy_with(target, changes, warns, window, cx);
    }

    /// The honesty lines of a write. A variant whose corridor fields are inverted
    /// (`MShotPriceMin ≥ MShotPrice`, which the search never proposes) is judged on a corridor
    /// the core's fields do not describe; one whose corridor comes nearer the price than a
    /// trade's own — the rule the search keeps under "keep the corridor"
    /// (`search::check_corridors`), asked of В1 as it will be written, typed or found — is
    /// judged on a sample without the spikes such an order would catch, so the dialog says on
    /// how many trades it does. Over the rows the columns and the search replay, each on its
    /// strategy's current values as they take them, so "M" is the column's "by N"; and only
    /// for a variant that moves an Entry field or `MaxModifier`, which caps the corridor too
    /// (`params::moves_entry`) — one that leaves the corridor alone moves nothing a warning
    /// could be about.
    fn ticks_change_warnings(
        &self,
        changes: &[(String, String)],
        cx: &Context<Self>,
    ) -> Vec<String> {
        let moves_entry = changes.iter().any(|(key, _)| params::moves_entry(key));
        let Some(data) = self.ticks.data.data().filter(|_| moves_entry) else {
            return Vec::new();
        };
        let deals: Vec<(
            &moon_core::db::tuner::ticks::Deal,
            Arc<HashMap<String, String>>,
        )> = data
            .replayable()
            .map(|r| {
                let own = data
                    .own
                    .get(&(r.deal.strategy_id, r.deal.core_uid))
                    .cloned()
                    .unwrap_or_default();
                (&r.deal, own)
            })
            .collect();
        let defaults = self.filter_defaults(cx);
        let mut warns = Vec::new();
        let check = check_corridors(
            deals.iter().map(|(deal, own)| (*deal, own.as_ref())),
            &defaults,
            changes,
            super::model_cfg::current(),
        );
        if check.inverted > 0 {
            warns.push(
                t!(
                    "analytics.ticks.inverted_warn",
                    n = check.inverted,
                    m = check.checked
                )
                .to_string(),
            );
        }
        if check.nearer > 0 {
            warns.push(
                t!(
                    "analytics.ticks.closer_warn",
                    n = check.nearer,
                    m = check.checked
                )
                .to_string(),
            );
        }
        warns
    }

    /// The warning of a write that leaves a strategy it lands on with nothing standing to close a
    /// trade — no stop, and no trailing without a take profit: the search refuses such a point
    /// (`search::unguarded_strategies`), a variant typed by hand is said to do so. Over the
    /// write's own targets the axis has read; one it has not is named by
    /// `ticks_unmodelled_warns`.
    fn ticks_unguarded_warning(
        &self,
        targets: &[super::super::shared::SaveTarget],
        changes: &[(String, String)],
        cx: &Context<Self>,
    ) -> Vec<String> {
        let Some(data) = self.ticks.data.data() else {
            return Vec::new();
        };
        let owns: Vec<&HashMap<String, String>> = targets
            .iter()
            .filter_map(|t| data.own.get(&(t.sid, t.core?)))
            .map(|o| o.as_ref())
            .collect();
        let n = moon_core::db::tuner::ticks::search::unguarded_strategies(
            owns.iter().copied(),
            &self.filter_defaults(cx),
            data.single_kind().unwrap_or_default(),
            changes,
            super::model_cfg::current(),
        );
        if n == 0 {
            return Vec::new();
        }
        vec![t!("analytics.ticks.unguarded_warn", n = n, m = owns.len()).to_string()]
    }
}

/// Lay a search's answer over В1 — a search of one field or of every one alike. A searched field
/// is searched anew from the strategies, whatever В1 held for it (LinKvo, 2026-09-25), so its cell
/// takes the answer, or is emptied when the answer leaves it at the strategies' own value. The
/// fields the search left alone keep their cells, and so does every value it completed for a
/// switch it turned on (`search::deps` — `UseTakeProfit` brings its `TakeProfit`) by taking it
/// from the answer, which the search scored and Save must write with it.
///
/// Args:
///     v1: В1's cells.
///     searched: The fields the search varied
///         ([`SearchResult::searched`](moon_core::db::tuner::ticks::search::SearchResult)).
///     values: The answer ([`SearchResult::values`](moon_core::db::tuner::ticks::search::SearchResult)).
fn land_answer(v1: &mut HashMap<String, String>, searched: &[String], values: &[(String, String)]) {
    for key in searched {
        v1.remove(key);
    }
    v1.extend(values.iter().cloned());
}

/// Why a search came back with nothing, as the status band says it; `floor` is the trade floor
/// the run held ([`SearchMiss::Floor`]).
pub(super) fn miss_note(miss: SearchMiss, floor: i64) -> String {
    match miss {
        SearchMiss::Floor => t!("analytics.ticks.sugg_floor", n = floor).to_string(),
        SearchMiss::TooFew { n } => t!(
            "analytics.ticks.sugg_too_few",
            n = n,
            min = MIN_SEARCH_DEALS
        )
        .to_string(),
        SearchMiss::Corridor => t!("analytics.ticks.sugg_corridor").to_string(),
        SearchMiss::Unclosed => t!("analytics.ticks.sugg_unclosed").to_string(),
        SearchMiss::Risk => t!("analytics.ticks.sugg_risk").to_string(),
        SearchMiss::Nothing => t!("analytics.ticks.sugg_none").to_string(),
    }
}

/// The status band's account of the last search: restarts, the winning one, its passes and
/// whether it converged, how many distinct end points, how many points were scored.
pub(super) fn search_stats_line(stats: &moon_core::db::tuner::ticks::SearchStats) -> String {
    let passes = if stats.converged {
        t!("analytics.ticks.stats_converged", n = stats.passes)
    } else {
        t!("analytics.ticks.stats_cut", n = stats.passes)
    };
    let line = t!(
        "analytics.ticks.stats_line",
        restarts = stats.restarts,
        best = stats.best_restart,
        passes = passes,
        distinct = stats.distinct,
        evals = stats.evaluations,
        refused = stats.refused
    )
    .to_string();
    // Both groups searched: how many entry points each ran a whole exit search.
    let line = match stats.entry_points {
        0 => line,
        n => format!(
            "{line} · {}",
            t!("analytics.ticks.stats_entry_points", n = n)
        ),
    };
    // The deals taken out before the search: the sample it answers for is smaller by them.
    match stats.left_open {
        0 => line,
        n => format!("{line} · {}", t!("analytics.ticks.stats_left_open", n = n)),
    }
}
