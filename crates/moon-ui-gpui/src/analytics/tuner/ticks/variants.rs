//! The variant columns and the search of the "Entry/Exit" axis: the edits behind В1/В2, their
//! debounced rescore over the replayable rows, the search that fills В1, and the write of В1
//! through the shared confirmation dialog.
//!
//! Every score here is a replay — `variant_tally` over the rows whose tape is in memory — so
//! the columns describe the SAME subset the "Fact · with tape" column describes, never the
//! whole scope. The captions say "by N" for that reason.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::*;
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::N_VAR;
use super::state::{NowValue, SuggState};
use crate::analytics::bg::ReadLane;
use moon_core::db::tuner::threshold_search::SearchHandle;
use moon_core::db::tuner::ticks::mshot::DEFAULT_LATENCY_MS;
use moon_core::db::tuner::ticks::params::ParamGroup;
use moon_core::db::tuner::ticks::search::{
    PreparedDeal, SearchParams, clip_to_horizon, common_horizon_ms, suggest, variant_tally,
};
use moon_core::db::tuner::ticks::stats_of;

/// How long a burst of cell edits may keep coalescing before the columns are rescored.
const VARIANT_DEBOUNCE: Duration = Duration::from_millis(350);

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

/// The base every variant is laid over: the fields the selected strategies agree on.
fn base_of(now: &HashMap<String, NowValue>) -> HashMap<String, String> {
    now.iter()
        .filter_map(|(key, value)| match value {
            NowValue::Same(v) if !v.is_empty() => Some((key.clone(), v.clone())),
            _ => None,
        })
        .collect()
}

impl AnalyticsView {
    /// The replayable rows as the search and the columns take them — every tape cut at the
    /// sample's one exit horizon (`clip_to_horizon`), so no variant is judged on more tape
    /// than another.
    fn prepared_deals(&self) -> Vec<PreparedDeal> {
        let mut deals: Vec<PreparedDeal> = self
            .ticks
            .data
            .data()
            .map(|d| {
                d.replayable()
                    .filter_map(|row| {
                        Some(PreparedDeal {
                            deal: row.deal.clone(),
                            ticks: row.ticks.clone()?,
                            entry_start: row.entry_start,
                            trail_ms: row.held.map(|(_, trail)| trail).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(horizon_ms) = common_horizon_ms(&deals) {
            clip_to_horizon(&mut deals, horizon_ms);
        }
        deals
    }

    /// Arm a debounced rescore of the variant columns — every edit of a cell, every row that
    /// joins the replayable set, goes through here.
    pub(in crate::analytics::tuner) fn arm_ticks_variants(&mut self, cx: &mut Context<Self>) {
        self.latest_reads.cancel(&[ReadLane::TicksVariants]);
        self.ticks.var_seq = self.ticks.var_seq.wrapping_add(1);
        // Nothing to score: an untouched pair of columns costs no clone of the rows and no
        // replay — a fetch over hundreds of rows re-arms this once per row.
        if (0..N_VAR).all(|i| self.ticks.variant_changes(i).is_empty()) {
            self.ticks.var_stats = Default::default();
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
        let deals = self.prepared_deals();
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let base = base_of(&data.now);
        let kind = data.single_kind().unwrap_or_default().to_string();
        let changes: Vec<Vec<(String, String)>> =
            (0..N_VAR).map(|i| self.ticks.variant_changes(i)).collect();
        let defaults = self.filter_defaults(cx);
        let n = deals.len();
        self.spawn_latest_db(
            &[ReadLane::TicksVariants],
            false,
            cx,
            move || {
                changes
                    .iter()
                    .map(|values| {
                        if values.is_empty() || deals.is_empty() {
                            return None;
                        }
                        let (tally, spent) = variant_tally(
                            &deals,
                            &base,
                            &defaults,
                            &kind,
                            values,
                            DEFAULT_LATENCY_MS,
                        );
                        Some(stats_of(tally, spent))
                    })
                    .collect::<Vec<_>>()
            },
            move |this, stats, cx| {
                if this.ticks.var_seq != req {
                    return;
                }
                for (slot, value) in this.ticks.var_stats.iter_mut().zip(stats) {
                    *slot = value;
                }
                this.ticks.var_n = n;
                cx.notify();
            },
        );
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

    /// Copy В1 into В2, so a found point can be kept while another is tried.
    pub(in crate::analytics::tuner) fn ticks_copy_v1_to_v2(&mut self, cx: &mut Context<Self>) {
        self.ticks.variants[1] = self.ticks.variants[0].clone();
        self.ticks_reset_inputs_of(1);
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
        self.ticks_reset_inputs_of(index);
        self.arm_ticks_variants(cx);
        cx.notify();
    }

    /// Drop the input boxes of a variant column so they are recreated from the stored values.
    fn ticks_reset_inputs_of(&mut self, index: usize) {
        let prefix = format!("v{index}:");
        self.ticks.inputs.retain(|id, _| !id.starts_with(&prefix));
    }

    /// Whether a group may be searched: the user's switch, the kind's support, and the share
    /// gate.
    pub(in crate::analytics::tuner) fn ticks_group_searchable(&self, group: ParamGroup) -> bool {
        let Some(data) = self.ticks.data.data() else {
            return false;
        };
        let supported = match group {
            ParamGroup::Entry => data.entry_modelled(),
            ParamGroup::Exit => true,
        };
        supported && data.group_passes(group) == Some(true)
    }

    /// Run the search into В1.
    pub(in crate::analytics::tuner) fn ticks_suggest(&mut self, cx: &mut Context<Self>) {
        if matches!(self.ticks.sugg, SuggState::Running { .. }) {
            return;
        }
        let deals = self.prepared_deals();
        let Some(data) = self.ticks.data.data() else {
            return;
        };
        let Some(kind) = data.single_kind().map(String::from) else {
            self.ticks.sugg_note = Some(t!("analytics.ticks.sugg_one_kind").to_string());
            cx.notify();
            return;
        };
        let vary_entry = self.ticks.vary_entry && self.ticks_group_searchable(ParamGroup::Entry);
        let vary_exit = self.ticks.vary_exit && self.ticks_group_searchable(ParamGroup::Exit);
        if !(vary_entry || vary_exit) {
            self.ticks.sugg_note = Some(t!("analytics.ticks.sugg_nothing").to_string());
            cx.notify();
            return;
        }
        if deals.is_empty() {
            self.ticks.sugg_note = Some(t!("analytics.ticks.sugg_no_tape").to_string());
            cx.notify();
            return;
        }
        let base = base_of(&data.now);
        let defaults = self.filter_defaults(cx);
        let locked: HashSet<String> = self.ticks.locked.clone();
        let restarts = restarts_of(&self.ticks.iters);
        let min_n = self
            .ticks
            .min_trades
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0);
        let train_frac = super::super::filter::state::train_frac(self.ticks.train_pct);
        let handle = SearchHandle::new();
        self.ticks.sugg = SuggState::Running {
            handle: handle.clone(),
            total: restarts,
        };
        self.ticks.sugg_seq = self.ticks.sugg_seq.wrapping_add(1);
        self.ticks.sugg_note = None;
        let seq = self.ticks.sugg_seq;
        self.spawn_latest_db(
            &[ReadLane::TicksSearch],
            false,
            cx,
            move || {
                let params = SearchParams {
                    base: &base,
                    defaults: &defaults,
                    kind: &kind,
                    vary_entry,
                    vary_exit,
                    locked: &locked,
                    restarts,
                    min_n,
                    seed: None,
                    train_frac,
                    latency_ms: DEFAULT_LATENCY_MS,
                };
                suggest(&deals, &params, &handle)
            },
            move |this, result, cx| {
                if this.ticks.sugg_seq != seq {
                    return;
                }
                this.ticks.sugg = SuggState::Idle;
                match result {
                    Some(result) => {
                        this.ticks.variants[0] =
                            result.values.iter().cloned().collect::<HashMap<_, _>>();
                        this.ticks_reset_inputs_of(0);
                        this.ticks.last_result = Some(result);
                        this.arm_ticks_variants(cx);
                    }
                    None => {
                        this.ticks.sugg_note = Some(t!("analytics.ticks.sugg_none").to_string());
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Stop a running search; what it found so far is dropped.
    pub(in crate::analytics::tuner) fn ticks_stop_suggest(&mut self, cx: &mut Context<Self>) {
        self.ticks.stop_search();
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
        let warns = self.ticks_change_warnings(&changes);
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
        let warns = self.ticks_change_warnings(&changes);
        self.open_copy_with(target, changes, warns, window, cx);
    }

    /// The honesty line of a write: a closer `MShotPrice` is UNDERESTIMATED by the sample
    /// (spikes the real order never reached are not in the report), so the dialog says so.
    fn ticks_change_warnings(&self, changes: &[(String, String)]) -> Vec<String> {
        let mut warns = Vec::new();
        let base_price = self
            .ticks
            .data
            .data()
            .and_then(|d| match d.now.get("MShotPrice") {
                Some(NowValue::Same(v)) => v.replace(',', ".").parse::<f64>().ok(),
                _ => None,
            });
        if let (Some(base), Some((_, value))) =
            (base_price, changes.iter().find(|(k, _)| k == "MShotPrice"))
        {
            if value
                .replace(',', ".")
                .parse::<f64>()
                .is_ok_and(|v| v < base)
            {
                warns.push(t!("analytics.ticks.closer_warn").to_string());
            }
        }
        warns
    }
}
