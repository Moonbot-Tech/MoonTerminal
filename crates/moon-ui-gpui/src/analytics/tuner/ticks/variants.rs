//! The variant columns and the search of the "Entry/Exit" axis: the edits behind В1/В2, their
//! debounced rescore over the replayable rows, the search that fills В1, and the write of В1
//! through the shared confirmation dialog.
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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::*;
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::N_VAR;
use super::state::SuggState;
use super::tape::{PendingDeal, prepare_sample};
use crate::analytics::bg::ReadLane;
use moon_core::db::tuner::threshold_search::SearchHandle;
use moon_core::db::tuner::ticks::TICK_PARAMS;
use moon_core::db::tuner::ticks::params::ParamGroup;
use moon_core::db::tuner::ticks::search::{
    DEFAULT_MAX_PASSES, SearchMiss, SearchParams, check_corridors, suggest, train_len,
    variant_tally_by_deal,
};
use moon_core::db::tuner::ticks::stats_of;

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
    fn prepared_deals(&self) -> Vec<PendingDeal> {
        self.ticks
            .data
            .data()
            .map(|d| d.replayable().filter_map(|row| d.prepared(row)).collect())
            .unwrap_or_default()
    }

    /// Arm a debounced rescore of the variant columns — every edit of a cell, every row that
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
        // Nothing to score: an untouched pair of columns costs no clone of the rows and no
        // replay — a fetch over hundreds of rows re-arms this once per row.
        if (0..N_VAR).all(|i| self.ticks.variant_changes(i).is_empty()) {
            self.ticks.var_stats = Default::default();
            self.set_ticks_plan(Default::default());
            // The trade pane's modelled trades go with the columns.
            self.ticks_refresh_model_trades(cx);
            return;
        }
        let req = self.ticks.var_seq;
        self.ticks.var_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(VARIANT_DEBOUNCE).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.ticks.var_seq == req {
                        this.run_ticks_variants(req, cx);
                    }
                });
            });
        }));
    }

    /// Score every touched variant over the replayable rows.
    fn run_ticks_variants(&mut self, req: u64, cx: &mut Context<Self>) {
        let pending = self.prepared_deals();
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let kind = data.single_kind().unwrap_or_default().to_string();
        let changes: Vec<Vec<(String, String)>> =
            (0..N_VAR).map(|i| self.ticks.variant_changes(i)).collect();
        let defaults = self.filter_defaults(cx);
        let model = super::model_cfg::current();
        let n = pending.len();
        self.spawn_latest_db(
            &[ReadLane::TicksVariants],
            false,
            cx,
            move || {
                let deals = prepare_sample(pending);
                changes
                    .iter()
                    .map(|values| {
                        if values.is_empty() || deals.is_empty() {
                            return None;
                        }
                        let (tally, spent, money) =
                            variant_tally_by_deal(&deals, &defaults, &kind, values, model);
                        let plan: HashMap<i64, (f64, f64)> = money
                            .into_iter()
                            .filter_map(|(uid, value)| Some((uid, value?)))
                            .collect();
                        Some((stats_of(tally, spent), plan))
                    })
                    .collect::<Vec<_>>()
            },
            move |this, stats, cx| {
                if this.ticks.var_seq != req {
                    return;
                }
                let mut plan: [HashMap<i64, (f64, f64)>; N_VAR] = Default::default();
                for ((slot, value), plan) in this
                    .ticks
                    .var_stats
                    .iter_mut()
                    .zip(stats)
                    .zip(plan.iter_mut())
                {
                    *slot = value.map(|(stats, deals)| {
                        *plan = deals;
                        stats
                    });
                }
                this.set_ticks_plan(plan);
                this.ticks.var_n = n;
                // The trade pane draws what the columns now count.
                this.ticks_refresh_model_trades(cx);
                cx.notify();
            },
        );
    }

    /// Take the variants' per-deal results; a table sorted by the plan column is re-sorted.
    fn set_ticks_plan(&mut self, plan: [HashMap<i64, (f64, f64)>; N_VAR]) {
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

    /// One cell of a variant changed: store it and rescore.
    pub(in crate::analytics::tuner) fn set_ticks_variant(
        &mut self,
        index: usize,
        key: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        self.ticks.set_variant(index, key, value);
        self.arm_ticks_variants(cx);
    }

    /// Copy one variant column over the other — В1 into В2 keeps a found point while another is
    /// tried, В2 into В1 brings a kept one back for Save.
    pub(in crate::analytics::tuner) fn ticks_copy_variant(
        &mut self,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        self.ticks.variants[to] = self.ticks.variants[from].clone();
        self.ticks_reset_inputs_of(to);
        self.arm_ticks_variants(cx);
        cx.notify();
    }

    /// Clear one variant column.
    pub(in crate::analytics::tuner) fn ticks_clear_variant(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.ticks.variants[index].clear();
        self.ticks.var_stats[index] = None;
        self.ticks.plan[index].clear();
        self.ticks_reset_inputs_of(index);
        self.arm_ticks_variants(cx);
        cx.notify();
    }

    /// Drop the input boxes of a variant column so they are recreated from the stored values.
    fn ticks_reset_inputs_of(&mut self, index: usize) {
        let prefix = format!("v{index}:");
        self.ticks.inputs.retain(|id, _| !id.starts_with(&prefix));
    }

    /// Whether a group may be searched: the kind's support and the share gate.
    pub(in crate::analytics::tuner) fn ticks_group_searchable(&self, group: ParamGroup) -> bool {
        let Some(data) = self.ticks.data.data() else {
            return false;
        };
        let supported = match group {
            ParamGroup::Entry => data.entry_modelled(),
            ParamGroup::Exit => true,
        };
        supported && data.group_passes(group, self.ticks.gate()) == Some(true)
    }

    /// The tooltips of "Search" and "Search all": what each varies and where its answer lands —
    /// the selected field by name, the number of ticked fields the gate lets through. A scope of
    /// several kinds, which neither searches, says so on both.
    pub(super) fn ticks_search_tips(&self) -> (String, String) {
        let data = self.ticks.data.data();
        if data.is_some_and(|d| !d.kinds.is_empty() && d.single_kind().is_none()) {
            let refused = t!("analytics.ticks.sugg_one_kind").to_string();
            return (refused.clone(), refused);
        }
        let one = match self.ticks.sel_field {
            Some(key) => t!("analytics.ticks.suggest_one_tip", field = key).to_string(),
            None => t!("analytics.ticks.suggest_one_none").to_string(),
        };
        let entry_on = data.is_some_and(|d| d.entry_modelled());
        let ticked = data.map_or(0, |d| {
            d.grid
                .iter()
                .flat_map(super::sections::GridSection::knobs)
                .filter(|k| super::grid::knob_ticks(k, entry_on))
                .filter(|k| !self.ticks.locked.contains(k.key))
                .filter(|k| self.ticks_group_searchable(k.group))
                .count()
        });
        (
            one,
            t!("analytics.ticks.suggest_all_tip", n = ticked).to_string(),
        )
    }

    /// "Search all": every ticked field of the groups the gate lets through, into В1.
    pub(in crate::analytics::tuner) fn ticks_suggest(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ticks_run_search(None, window, cx);
    }

    /// "Search": the selected field alone, ticked or not, the rest of В1 held as it stands; the
    /// answer goes into that cell of В1 and the cells of the values it completed ([`land_answer`]).
    pub(in crate::analytics::tuner) fn ticks_suggest_one(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self.ticks.sel_field {
            self.ticks_run_search(Some(key), window, cx);
        }
    }

    /// A search asked for: first the warning when a strategy it runs on switches on an exit
    /// field the model does not have (`unmodelled.rs`) — the search then starts on its Continue.
    fn ticks_run_search(
        &mut self,
        only: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.ticks.sugg, SuggState::Running { .. }) {
            return;
        }
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
    /// every other held — at the strategies' value, or at В1's where В1 changes it.
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
        // Laid over every deal's own strategy before the search's point: nothing for a search
        // of every field, В1's other fields for a search of one.
        let mut held: HashMap<String, String> = HashMap::new();
        let (vary_entry, vary_exit, locked) = match only {
            None => (
                self.ticks_group_searchable(ParamGroup::Entry),
                self.ticks_group_searchable(ParamGroup::Exit),
                self.ticks.locked.clone(),
            ),
            Some(key) => {
                let Some(field) = TICK_PARAMS.iter().find(|f| f.key == key) else {
                    return;
                };
                if !model.entry_method.reads(key) {
                    return self.ticks_search_refused("analytics.ticks.sugg_not_read", cx);
                }
                if !self.ticks_group_searchable(field.group) {
                    return self.ticks_search_refused("analytics.ticks.sugg_gated", cx);
                }
                // The other fields as В1 has them: the one field is searched in the variant it
                // will land in, not in the strategy as it stands.
                held.extend(self.ticks.variant_changes(0));
                let locked: HashSet<String> = TICK_PARAMS
                    .iter()
                    .map(|f| f.key)
                    .filter(|k| *k != key)
                    .map(str::to_string)
                    .collect();
                (
                    field.group == ParamGroup::Entry,
                    field.group == ParamGroup::Exit,
                    locked,
                )
            }
        };
        if !(vary_entry || vary_exit) {
            return self.ticks_search_refused("analytics.ticks.sugg_nothing", cx);
        }
        if pending.is_empty() {
            return self.ticks_search_refused("analytics.ticks.sugg_no_tape", cx);
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
        self.ticks.sugg_note = None;
        let seq = self.ticks.sugg_seq;
        log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks search: start #{seq}, {restarts} restart(s) x {max_passes} pass(es) over {} deal(s), entry {vary_entry}, exit {vary_exit}",
            pending.len()
        );
        probe::watch(handle.clone(), restarts, seq);
        self.poll_ticks_search(handle.clone(), seq, cx);
        let started = std::time::Instant::now();
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
                    restarts,
                    min_n,
                    seed,
                    train_frac,
                    max_passes,
                    model,
                    keep_corridor,
                };
                suggest(&deals, &params, &handle)
            },
            move |this, result, cx| {
                log::info!(
                    target: moon_core::diagnostics::TICKS_AXIS_TARGET,
                    "[x] ticks search: #{seq} answered after {} ms ({}), current #{}",
                    started.elapsed().as_millis(),
                    match &result {
                        Ok(found) => format!("found {:?}", found.values),
                        Err(miss) => format!("nothing: {miss:?}"),
                    },
                    this.ticks.sugg_seq
                );
                if this.ticks.sugg_seq != seq {
                    return;
                }
                this.ticks.sugg = SuggState::Idle;
                match result {
                    Ok(result) => {
                        land_answer(&mut this.ticks.variants[0], only, &result.values);
                        this.ticks_reset_inputs_of(0);
                        this.ticks.last_seed = Some(result.seed);
                        this.ticks.last_result = Some(result);
                        this.arm_ticks_variants(cx);
                    }
                    // Why nothing: the floor no point kept — the typed one, or the search's own
                    // tenth of the training slice —, the corridor none kept, no point that
                    // closed every trade it bought, or nothing at all.
                    Err(miss) => {
                        this.ticks.sugg_note = Some(match miss {
                            SearchMiss::Floor => t!(
                                "analytics.ticks.sugg_floor",
                                n = min_n.unwrap_or((train_n as i64 / 10).max(1))
                            )
                            .to_string(),
                            SearchMiss::Corridor => t!("analytics.ticks.sugg_corridor").to_string(),
                            SearchMiss::Unclosed => t!("analytics.ticks.sugg_unclosed").to_string(),
                            SearchMiss::Nothing => t!("analytics.ticks.sugg_none").to_string(),
                        });
                    }
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
                        let done = handle.completed();
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
        let changes = self.ticks.variant_changes(0);
        if changes.is_empty() {
            log::info!("analytics: 'Save' (ticks) - no variant to write");
            return;
        }
        let mut warns = self.ticks_change_warnings(&changes, cx);
        warns.extend(self.ticks_unguarded_warning(&targets, &changes, cx));
        warns.extend(self.ticks_unmodelled_warns(&targets, cx));
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
        let changes = self.ticks.variant_changes(0);
        let mut warns = self.ticks_change_warnings(&changes, cx);
        warns.extend(self.ticks_unguarded_warning(std::slice::from_ref(&target), &changes, cx));
        warns.extend(self.ticks_unmodelled_warns(std::slice::from_ref(&target), cx));
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
    /// for a variant that moves an Entry field — one that leaves the corridor alone moves
    /// nothing a warning could be about.
    fn ticks_change_warnings(
        &self,
        changes: &[(String, String)],
        cx: &Context<Self>,
    ) -> Vec<String> {
        let moves_entry = changes.iter().any(|(key, _)| {
            TICK_PARAMS
                .iter()
                .any(|f| f.key == key && f.group == ParamGroup::Entry)
        });
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

/// Lay a search's answer into В1. A search of every field replaces В1 with it. A search of one
/// field lays its cells over В1's: the searched field, and every value the answer completed for a
/// switch it turned on (`search::deps` — `UseTakeProfit` brings its `TakeProfit`), which the
/// search scored and Save must write with it; the search locked every other field, so nothing
/// else moves, and a field it left at its base keeps what В1 had.
///
/// Args:
///     v1: В1's cells.
///     only: The searched field, for a search of one.
///     values: The answer ([`SearchResult::values`](moon_core::db::tuner::ticks::search::SearchResult)).
fn land_answer(v1: &mut HashMap<String, String>, only: Option<&str>, values: &[(String, String)]) {
    match only {
        None => *v1 = values.iter().cloned().collect(),
        Some(_) => v1.extend(values.iter().cloned()),
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
    // The deals taken out before the search: the sample it answers for is smaller by them.
    match stats.left_open {
        0 => line,
        n => format!("{line} · {}", t!("analytics.ticks.stats_left_open", n = n)),
    }
}
