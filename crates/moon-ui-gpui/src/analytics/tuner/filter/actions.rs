//! Tuner actions for threshold suggestions and applying v1 to the selected
//! strategy, including the required ignore-class changes.

use std::collections::HashMap;

use gpui::*;

use moon_ui::{MoonInputEvent, MoonInputState};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::SaveTarget;
use super::state::{
    SearchSplit, SuggestJob, SuggestState, edges_of, iters_of, seed_of, train_frac,
};
use super::{fmt_bound, parse_num, staged_dirty};
use moon_core::db::tuner::threshold_search::{SearchHandle, SearchParams};
use moon_core::db::tuner::{FIELDS, FieldClass, slot_type_for};

/// How often the suggestion row repaints while a search runs.
///
/// The search publishes its progress into an atomic instead of sending an event per restart, so
/// this is what turns that counter into a moving number. Fast enough to read as live, slow enough
/// that a 20 000-restart run costs a few dozen repaints rather than 20 000.
const SUGGEST_POLL: std::time::Duration = std::time::Duration::from_millis(200);

impl AnalyticsView {
    /// Suggest all v1 ranges jointly with coordinate descent.
    ///
    /// Deliberately WITHOUT the window's blocking overlay: this search can run for tens of
    /// seconds, and the overlay would cover the Stop button it depends on. Its outcome and its
    /// failures live in `tuner.sugg`, so a failed SEARCH no longer erases the KPI matrix, which
    /// is a different read entirely. A valid result updates only fields enabled for search.
    ///
    /// Args:
    ///     cx: GPUI context used to execute and publish the suggestion.
    pub(in crate::analytics::tuner) fn suggest_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        // A search that is being replaced must be told, not merely ignored: its result is already
        // unpublishable, and letting it run to the end holds every worker for nothing.
        self.tuner.stop_suggest();
        let q = self.tuner_query();
        let restarts = iters_of(&self.tuner.iters);
        let min_n = self.suggest_min_n();
        let edges = self.suggest_edges();
        let round = self.tuner.round_results;
        let seed = seed_of(&self.tuner.seed);
        let train_frac = train_frac(self.tuner.train_pct);
        // Unchecked boxes: the field is not searched; with bounds filled in it
        // still participates as a fixed filter.
        let locked: Vec<Option<(Option<f64>, Option<f64>)>> = (0..FIELDS.len())
            .map(|fi| {
                if self.tuner.enabled[fi] {
                    None
                } else {
                    let (from, to) = &self.tuner.bounds[0][fi];
                    Some((parse_num(from), parse_num(to)))
                }
            })
            .collect();
        let handle = SearchHandle::new();
        self.tuner.sugg = SuggestState::Running(SuggestJob::AllFields {
            handle: handle.clone(),
            total: restarts,
        });
        self.poll_suggest_progress(handle.clone(), cx);
        let worker = handle.clone();
        self.spawn_db(
            false,
            cx,
            move || {
                moon_core::db::tuner::threshold_search::suggest(
                    &q,
                    SearchParams {
                        restarts,
                        min_n,
                        locked: &locked,
                        edges,
                        round,
                        seed,
                        train_frac,
                    },
                    &worker,
                )
            },
            move |this, sugg, cx| {
                if this.tuner.sugg_seq != req {
                    return;
                }
                // Read from the handle, not from the request: after a stop this is what the
                // search actually got through.
                let rounds = handle.completed();
                // A stop that arrived after the last restart finished — while the result was on
                // its way to this closure — abandoned nothing, so it is not a stop. Reporting it
                // as one would tell the user their search was cut short when it was complete.
                let stopped = handle.is_cancelled() && rounds < restarts;
                let found = match sugg {
                    Ok(found) => found,
                    // A failed read must not look like "found nothing": the button would just
                    // stop spinning and leave no trace.
                    Err(e) => {
                        log::warn!("analytics: smart suggestion failed — {e}");
                        this.tuner.sugg = SuggestState::Failed(e);
                        cx.notify();
                        return;
                    }
                };
                this.tuner.sugg = SuggestState::Done {
                    rounds,
                    stopped,
                    split: found.as_ref().map(|res| SearchSplit {
                        train: res.train.clone(),
                        holdout: res.holdout.clone(),
                    }),
                };
                // Offer the seed for pinning only after a COMPLETE run. A stopped search finishes
                // an arbitrary subset of restart indices, not the first N, so rerunning its seed
                // for its reported count would explore a different set and answer differently —
                // the one thing a "reproduce this" button must not do.
                if let (Some(res), false) = (found.as_ref(), stopped) {
                    this.tuner.last_seed = Some(res.seed);
                }
                if let Some(res) = found {
                    // The held-back figure is logged beside the fitted one on purpose: the two
                    // together are the whole verdict, and a log carrying only the fitted profit
                    // reports the flattering half of it.
                    let holdout = res.holdout.as_ref().map_or_else(
                        || "not split".to_string(),
                        |h| format!("{:+.2} over {}", h.profit, h.n),
                    );
                    log::info!(
                        "analytics: smart suggestion — in sample {:+.2} over {}, \
                         out of sample {holdout}, restarts {rounds}, seed {}{}",
                        res.train.profit,
                        res.train.n,
                        res.seed,
                        if stopped { " (stopped)" } else { "" }
                    );
                    let by_field: HashMap<&str, _> =
                        res.fields.into_iter().map(|f| (f.field, f)).collect();
                    for fi in 0..FIELDS.len() {
                        // Leave fields that were not searched alone (fixed/disabled).
                        if !this.tuner.enabled[fi] {
                            continue;
                        }
                        let (from, to) = by_field
                            .get(FIELDS[fi].col)
                            .map(|f| (fmt_bound(f.from), fmt_bound(f.to)))
                            .unwrap_or_default();
                        this.tuner.bounds[0][fi] = (from, to);
                        this.tuner.inputs.remove(&format!("tv0f{fi}a"));
                        this.tuner.inputs.remove(&format!("tv0f{fi}b"));
                    }
                    this.reload_tuner(cx);
                }
                cx.notify();
            },
        );
    }

    /// Stop the running joint search from the row's Stop button.
    ///
    /// The state stays `Running` until the search returns, which is the truth: it is still
    /// unwinding, and the restarts it already finished are what it will answer with.
    ///
    /// Args:
    ///     cx: GPUI context used to repaint the suggestion row.
    pub(in crate::analytics::tuner) fn stop_suggest_into_v1(&mut self, cx: &mut Context<Self>) {
        if self.tuner.stop_suggest() {
            cx.notify();
        }
    }

    /// Repaint the suggestion row while `handle`'s search runs, so its progress advances.
    ///
    /// Also the search's lifeline to its window: once the view is gone there is nobody left to
    /// publish to, so the search is stopped rather than left running against a closed window.
    ///
    /// Args:
    ///     handle: The run this poll follows; a search that replaced it ends the loop.
    ///     cx: GPUI context used to spawn the timer loop.
    fn poll_suggest_progress(&self, handle: SearchHandle, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let mut shown = usize::MAX;
            loop {
                executor.timer(SUGGEST_POLL).await;
                let mut mine = false;
                let view_gone = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        mine = this
                            .tuner
                            .sugg
                            .joint_run()
                            .is_some_and(|(running, _)| running.same_run(&handle));
                        // A repaint here redraws the WHOLE analytics panel — the 24-row grid, the
                        // KPI matrix, the strategy list — to move one integer. Only ask for one
                        // when that integer actually moved, so a search finishing inside a single
                        // tick costs no repaint at all.
                        let done = handle.completed();
                        if mine && done != shown {
                            shown = done;
                            cx.notify();
                        }
                    })
                    .is_err()
                });
                if view_gone {
                    handle.cancel();
                    return;
                }
                if !mine {
                    return;
                }
            }
        })
        .detach();
    }

    /// Suggest the best v1 range for the selected field.
    ///
    /// One field over one scan, so it keeps the blocking overlay rather than a Stop button: it
    /// returns before a stop would be reachable. Failures land in `tuner.sugg` alongside the
    /// joint search's; a valid `None` means no threshold improves on the baseline.
    ///
    /// Args:
    ///     cx: GPUI context used to execute and publish the suggestion.
    pub(in crate::analytics::tuner) fn suggest_one_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        self.tuner.stop_suggest();
        let fi = self.tuner.sel_field;
        let field = FIELDS[fi].col.to_string();
        let q = self.tuner_query();
        // This search holds nothing back, so its automatic minimum is one tenth of the whole
        // scope and can be settled here — unlike the joint search, whose train window is not
        // known until the split has been snapped.
        let min_n = self.suggest_min_n().unwrap_or_else(|| {
            self.tuner
                .stats
                .data()
                .and_then(|s| s.first().map(|f| f.n / 10))
                .unwrap_or(0)
                .max(1)
        });
        let edges = self.suggest_edges();
        let round = self.tuner.round_results;
        self.tuner.sugg = SuggestState::Running(SuggestJob::SingleField);
        self.spawn_db(
            true,
            cx,
            move || moon_core::db::tuner::suggest_field(&q, &field, min_n, edges, round),
            move |this, sugg, cx| {
                if this.tuner.sugg_seq != req {
                    return;
                }
                match sugg {
                    Ok(Some(s)) => {
                        this.tuner.sugg = SuggestState::Idle;
                        let from = s.from.map(fmt_bound).unwrap_or_default();
                        let to = s.to.map(fmt_bound).unwrap_or_default();
                        this.apply_bounds(0, fi, from, to, cx);
                    }
                    // A genuine "no threshold beats the baseline".
                    Ok(None) => this.tuner.sugg = SuggestState::Idle,
                    Err(e) => {
                        log::warn!("analytics: threshold suggestion failed — {e}");
                        this.tuner.sugg = SuggestState::Failed(e);
                    }
                }
                cx.notify();
            },
        );
    }

    /// Copy bounds v1 → v2: row `fi`, or the whole column with (None).
    pub(in crate::analytics::tuner) fn copy_v1_to_v2(
        &mut self,
        fi: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let range: Vec<usize> = match fi {
            Some(fi) => vec![fi],
            None => (0..FIELDS.len()).collect(),
        };
        for fi in range {
            let v = self.tuner.bounds[0][fi].clone();
            if self.tuner.bounds[1][fi] == v {
                continue;
            }
            self.tuner.bounds[1][fi] = v;
            self.tuner.inputs.remove(&format!("tv1f{fi}a"));
            self.tuner.inputs.remove(&format!("tv1f{fi}b"));
        }
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Copy bounds v2 → v1: row `fi`, or the whole column with `None`. Mirror of
    /// [`Self::copy_v1_to_v2`] — drives the ← button in the fields grid header.
    pub(in crate::analytics::tuner) fn copy_v2_to_v1(
        &mut self,
        fi: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.tuner.invalidate_suggest();
        let range: Vec<usize> = match fi {
            Some(fi) => vec![fi],
            None => (0..FIELDS.len()).collect(),
        };
        for fi in range {
            let v = self.tuner.bounds[1][fi].clone();
            if self.tuner.bounds[0][fi] == v {
                continue;
            }
            self.tuner.bounds[0][fi] = v;
            self.tuner.inputs.remove(&format!("tv0f{fi}a"));
            self.tuner.inputs.remove(&format!("tv0f{fi}b"));
        }
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Clear a variant column (the cross in the grid header) — only rows whose
    /// checkbox is ENABLED: unchecked ones are fixed filters, we leave them alone.
    pub(in crate::analytics::tuner) fn clear_variant(&mut self, vi: usize, cx: &mut Context<Self>) {
        if vi == 0 {
            self.tuner.invalidate_suggest();
        }
        for fi in 0..FIELDS.len() {
            if !self.tuner.enabled[fi] {
                continue;
            }
            self.tuner.bounds[vi][fi] = (String::new(), String::new());
            self.tuner.inputs.remove(&format!("tv{vi}f{fi}a"));
            self.tuner.inputs.remove(&format!("tv{vi}f{fi}b"));
        }
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Number of quantile edges for the suggestion (the 4/8/…/128 dropdown).
    fn suggest_edges(&self) -> usize {
        edges_of(self.tuner.edges)
    }

    /// Is there anything to write: staged "ignore" toggles OR v1 thresholds that
    /// differ from the strategy's current parameters (the "Save" button lights amber).
    pub(in crate::analytics::tuner) fn save_dirty(&self) -> bool {
        if staged_dirty(&self.tuner.strat, &self.tuner.staged_ignore) {
            return true;
        }
        let near = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => (a - b).abs() <= a.abs().max(b.abs()).max(1.0) * 1e-9,
            _ => false,
        };
        let f = &self.tuner.strat;
        for (fi, spec) in FIELDS.iter().enumerate() {
            let (from, to) = &self.tuner.bounds[0][fi];
            let (lo, hi) = (parse_num(from), parse_num(to));
            if lo.is_none() && hi.is_none() {
                continue;
            }
            // Unmapped fields are never written to the strategy — they don't count.
            if !spec.mapped() {
                continue;
            }
            let cur = if spec.class == FieldClass::DeltaSlot {
                f.slot_of(spec.col).map(|(_, l, h)| (l, h))
            } else {
                f.bounds.get(spec.col).copied()
            };
            match cur {
                Some((cl, ch)) if near(lo, cl) && near(hi, ch) => {}
                _ => return true,
            }
        }
        false
    }

    /// Minimum trades a suggestion must retain: the number typed into the box, or `None` for the
    /// search's own automatic value.
    ///
    /// A typed number is taken literally — it is the user's, not a share of anything. The
    /// automatic one is deliberately NOT worked out here: it means one tenth of what the descent
    /// fits on, and after the train share is snapped to a timestamp boundary only the search
    /// knows how many trades that is.
    fn suggest_min_n(&self) -> Option<i64> {
        self.tuner.min_trades.trim().parse::<i64>().ok()
    }

    /// "To strategy": v1 thresholds → the selected strategy's parameters on all of
    /// its cores (sync sends the full set — the edits go in one command per core). If
    /// the classes of the touched fields were being ignored (IgnoreFilters/IgnoreDelta/
    /// IgnoreVolume) — the corresponding flags are turned off, otherwise the thresholds
    /// would have no effect. Fields mapped onto parameters are written; slot fields
    /// (d1h/d15m/d5m/d1m/Pump1H/Dump1H) go through Delta2/Delta3: first
    /// `DeltaN_Type`, then `DeltaN_Min/Max`; there are two slots — the rest go to the log.
    pub(in crate::analytics::tuner) fn open_save_dialog(&mut self, cx: &mut Context<Self>) {
        let targets = self.selected_targets();
        if targets.is_empty() {
            return;
        }
        // Multi-select is a blind fan-out (no per-target preview), so force the enabling
        // Ignore flags on so the thresholds actually take effect on every target.
        let bulk = targets.len() > 1;
        let (mut changes, warns) = self.build_strategy_changes(bulk);
        if changes.is_empty() {
            log::info!("analytics: 'Save' — neither mapped thresholds nor changed ignore flags");
            return;
        }
        // The analyzer Comment stamp is built from the ANCHOR's Comment text; writing it to a
        // bulk fan-out would overwrite every other strategy's own description. Single-target only.
        if !bulk {
            changes.push(self.analyzer_comment());
        }
        // No notes: a threshold reads perfectly well as "now → next".
        self.open_change_dialog(targets, changes, None, Vec::new(), warns, false, cx);
    }

    /// "Make a copy": the same change set, but the target is a NEW strategy
    /// (a copy of the current one with the thresholds applied) on all of the original's
    /// cores. The name is auto-uniqued and editable in the confirmation dialog. An
    /// empty change list is fine — that is just a copy.
    pub(in crate::analytics::tuner) fn open_copy_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Copy is single-target only (the button is hidden in multi-select) — take the anchor.
        let Some(target) = self.selected_targets().into_iter().next() else {
            return;
        };
        let (mut changes, warns) = self.build_strategy_changes(false);
        changes.push(self.analyzer_comment());
        self.open_copy_with(target, changes, warns, window, cx);
    }

    /// The SHARED tail of "Make a copy" (all axes): an auto-uniqued name for the new
    /// strategy + its input in the confirmation dialog, then the SHARED dialog
    /// (`open_change_dialog`, is_copy=true). The user can edit the name before the write.
    pub(in crate::analytics::tuner) fn open_copy_with(
        &mut self,
        target: SaveTarget,
        changes: Vec<(String, String)>,
        warns: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Auto name: unique among strategy names on ALL cores (Moonbot names are global
        // within a core; the union is the safe superset).
        let mut taken = std::collections::HashSet::new();
        {
            let b = self.backend.read(cx);
            for (_, cd) in b.session.store().cores() {
                for r in &cd.strategies {
                    taken.insert(r.name.clone());
                }
            }
        }
        let proposed = crate::strategies::tree::ops::unique_name(&taken, &target.name);
        self.tuner.copy_name = proposed.clone();
        // A fresh name input on every open (so the default is rendered from its start).
        self.tuner.inputs.remove("copy-name");
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(proposed));
        cx.subscribe(&state, |this, st, ev: &MoonInputEvent, cx| {
            if matches!(
                ev,
                MoonInputEvent::Change | MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                this.tuner.copy_name = st.read(cx).value().to_string();
            }
        })
        .detach();
        self.tuner.inputs.insert("copy-name".to_string(), state);
        self.open_change_dialog(vec![target], changes, None, Vec::new(), warns, true, cx);
    }

    /// Build the changes from v1 + the staged "ignore" toggles (without the Comment stamp):
    /// mapped thresholds, Delta2/3 slots (+warnings), and the Ignore flags.
    ///
    /// `force_enable` (bulk / multi-select): emit the enabling Ignore flags for every
    /// touched class unconditionally, not only when the anchor currently ignores it — so
    /// the thresholds take effect on targets whose current flags differ from the anchor's.
    fn build_strategy_changes(&self, force_enable: bool) -> (Vec<(String, String)>, Vec<String>) {
        let mut changes: Vec<(String, String)> = Vec::new();
        let mut warns: Vec<String> = Vec::new();
        let (mut delta_touched, mut volume_touched, mut bvsv_touched, mut ping_touched) =
            (false, false, false, false);
        let mut base_touched = false;
        // Slot fields with v1 thresholds — candidates for Delta2/Delta3.
        let mut slot_wanted: Vec<(&'static str, Option<f64>, Option<f64>)> = Vec::new();
        for (fi, spec) in FIELDS.iter().enumerate() {
            let (from, to) = &self.tuner.bounds[0][fi];
            let class = &spec.class;
            if *class == FieldClass::DeltaSlot {
                let (lo, hi) = (parse_num(from), parse_num(to));
                if lo.is_some() || hi.is_some() {
                    slot_wanted.push((spec.col, lo, hi));
                }
                continue;
            }
            for (txt, param) in [(from, spec.p_min), (to, spec.p_max)] {
                let Some(param) = param else { continue };
                let Some(v) = parse_num(txt) else { continue };
                changes.push((param.to_string(), fmt_plain(v)));
                match class {
                    FieldClass::Delta => delta_touched = true,
                    FieldClass::Volume => volume_touched = true,
                    FieldClass::BvSv => bvsv_touched = true,
                    FieldClass::Ping => ping_touched = true,
                    FieldClass::Base => base_touched = true,
                    FieldClass::Filter | FieldClass::DeltaSlot => {}
                }
            }
        }
        // Slot assignment: its own previous place (if the type is already set) —
        // otherwise the first free one in order. Slots held by a type with no report
        // column (2h/30m/Pump5m with thresholds — a 'foreign' live filter) are
        // overwritten LAST and with a warn. Only two fit — the rest go to the log.
        if !slot_wanted.is_empty() {
            let cur2 = self
                .tuner
                .strat
                .slots
                .iter()
                .find(|(n, ..)| *n == 2)
                .map(|(_, f, ..)| *f);
            let cur3 = self
                .tuner
                .strat
                .slots
                .iter()
                .find(|(n, ..)| *n == 3)
                .map(|(_, f, ..)| *f);
            let foreign = self.tuner.strat.foreign_slots.clone();
            let mut used = [false, false]; // [Delta2, Delta3]
            for (n, _) in &foreign {
                used[(*n - 2) as usize] = true;
            }
            let mut assigned: Vec<(u8, &'static str, Option<f64>, Option<f64>)> = Vec::new();
            let mut dropped: Vec<&'static str> = Vec::new();
            // First — the fields already sitting in their own slots.
            for (col, lo, hi) in &slot_wanted {
                if cur2 == Some(*col) && !used[0] {
                    used[0] = true;
                    assigned.push((2, col, *lo, *hi));
                } else if cur3 == Some(*col) && !used[1] {
                    used[1] = true;
                    assigned.push((3, col, *lo, *hi));
                }
            }
            // Then — the free slots; last — overwriting the 'foreign' ones.
            for overwrite_foreign in [false, true] {
                for (col, lo, hi) in &slot_wanted {
                    if assigned.iter().any(|(_, f, ..)| f == col) {
                        continue;
                    }
                    let Some(i) = (0..2).find(|i| {
                        !used[*i]
                            || (overwrite_foreign
                                && foreign.iter().any(|(n, _)| *n == *i as u8 + 2)
                                && !assigned.iter().any(|(n, ..)| *n == *i as u8 + 2))
                    }) else {
                        if overwrite_foreign {
                            dropped.push(col);
                        }
                        continue;
                    };
                    used[i] = true;
                    if let Some((_, ty)) = foreign.iter().find(|(n, _)| *n == i as u8 + 2) {
                        let slot = format!("Delta{}", i + 2);
                        log::warn!(
                            "analytics: 'Save' — {slot} was held by type '{ty}' (no such \
                             column in the report), overwriting with '{col}'"
                        );
                        warns.push(
                            t!("analytics.tuner.warn_slot_replace", slot = slot, old = ty)
                                .to_string(),
                        );
                    }
                    assigned.push((i as u8 + 2, col, *lo, *hi));
                }
            }
            if !dropped.is_empty() {
                log::warn!(
                    "analytics: 'To strategy' — only two Delta2/Delta3 slots, did not fit: {}",
                    dropped.join(", ")
                );
                warns.push(
                    t!(
                        "analytics.tuner.warn_slot_drop",
                        fields = dropped.join(", ")
                    )
                    .to_string(),
                );
            }
            for (n, col, lo, hi) in assigned {
                let Some(ty) = slot_type_for(col) else {
                    continue;
                };
                // Order matters: the slot type first, then its thresholds.
                changes.push((format!("Delta{n}_Type"), ty.to_string()));
                if let Some(v) = lo {
                    changes.push((format!("Delta{n}_Min"), fmt_plain(v)));
                }
                if let Some(v) = hi {
                    changes.push((format!("Delta{n}_Max"), fmt_plain(v)));
                }
                delta_touched = true;
            }
        }
        // Ignore flags: auto-enable the classes whose thresholds we write, PLUS explicit
        // 'ignore' clicks on the subheaders (the staged value wins over the auto logic).
        // `force` only fires when we ACTUALLY wrote at least one threshold — otherwise a bulk
        // Save with a clean anchor would fan IgnoreFilters=NO to every target (flipping filters
        // on where they were ignored) with nothing to justify it, and the empty-guard at the
        // caller would never trip.
        let has_params = !changes.is_empty();
        let force = force_enable && has_params;
        let f = self.tuner.strat.clone();
        let mut flags: Vec<(&'static str, bool)> = Vec::new(); // (flag, ignore)
        if force || f.ignore_filters {
            flags.push(("IgnoreFilters", false));
        }
        if delta_touched && (force || f.ignore_delta) {
            flags.push(("IgnoreDelta", false));
        }
        // BV/SV is a Filters/Volume subgroup: its thresholds need IgnoreVolume cleared AND
        // the filter itself enabled.
        if (volume_touched || bvsv_touched) && (force || f.ignore_volume) {
            flags.push(("IgnoreVolume", false));
        }
        if bvsv_touched && (force || !f.use_bvsv) {
            flags.push(("UseBV_SV_Filter", false));
        }
        if ping_touched && (force || f.ignore_ping) {
            flags.push(("IgnorePing", false));
        }
        if base_touched && (force || f.ignore_base) {
            flags.push(("IgnoreBase", false));
        }
        for (flag, want) in self.tuner.staged_ignore.clone() {
            flags.retain(|(fl, _)| *fl != flag);
            let cur = match flag {
                "IgnoreFilters" => f.ignore_filters,
                "IgnorePing" => f.ignore_ping,
                "IgnoreDelta" => f.ignore_delta,
                "IgnoreVolume" => f.ignore_volume,
                "IgnoreBase" => f.ignore_base,
                "UseBV_SV_Filter" => !f.use_bvsv,
                _ => continue,
            };
            if want != cur {
                flags.push((flag, want));
            }
        }
        for (flag, ignore) in flags {
            // UseBV_SV_Filter is an enabler (inverted ignore semantics).
            let value = if flag == "UseBV_SV_Filter" {
                if ignore { "NO" } else { "YES" }
            } else if ignore {
                "YES"
            } else {
                "NO"
            };
            changes.push((flag.to_string(), value.to_string()));
        }
        (changes, warns)
    }

    /// The analyzer stamp for Comment: "dd.mm.yyyy hh:mm:ss (Save from
    /// analyzer)" UTC. The user's own description is preserved — only the previous
    /// stamp is replaced (segments are separated by "; ").
    ///
    /// `tuner`-visible: the "By time" copy dialog (`time/save.rs`) stamps its copies
    /// through the same helper.
    pub(in crate::analytics::tuner) fn analyzer_comment(&self) -> (String, String) {
        const MARK: &str = "(Save from analyzer)";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let ts = moon_core::db::fmt_unix_secs(now); // YYYY-MM-DD HH:MM:SS
        let (date, time) = ts.split_once(' ').unwrap_or((ts.as_str(), ""));
        let mut dmy = date.splitn(3, '-');
        let (y, m, d) = (
            dmy.next().unwrap_or(""),
            dmy.next().unwrap_or(""),
            dmy.next().unwrap_or(""),
        );
        let stamp = format!("{d}.{m}.{y} {time} {MARK}");
        let base: Vec<&str> = self
            .tuner
            .strat
            .comment
            .split("; ")
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.contains(MARK))
            .collect();
        let comment = if base.is_empty() {
            stamp
        } else {
            format!("{}; {stamp}", base.join("; "))
        };
        ("Comment".to_string(), comment)
    }
}

/// A number for a strategy parameter: plain decimal format, no suffixes.
fn fmt_plain(v: f64) -> String {
    let mut s = format!("{v:.4}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}
