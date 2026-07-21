//! Tuner actions for threshold suggestions and applying v1 to the selected
//! strategy, including the required ignore-class changes.

use std::collections::HashMap;

use gpui::*;

use moon_ui::{MoonInputEvent, MoonInputState};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::SaveTarget;
use super::state::{edges_of, iters_of};
use super::{fmt_bound, parse_num, staged_dirty};
use moon_core::db::tuner::{FIELDS, FieldClass, slot_type_for};

impl AnalyticsView {
    /// Suggest all v1 ranges jointly with coordinate descent.
    ///
    /// `NotReady` and failed reads are published through the shared KPI load
    /// state; a valid result updates only fields enabled for search.
    pub(in crate::analytics::tuner) fn suggest_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        // Suggestion failures share `tuner.stats` with KPI reads, so both the
        // suggestion and KPI generations must still match before publishing.
        let stats_req = self.tuner.seq;
        self.tuner.sugg_busy = true;
        let q = self.tuner_query();
        let rounds = iters_of(&self.tuner.iters);
        let min_n = self.suggest_min_n();
        let edges = self.suggest_edges();
        let round = self.tuner.round_results;
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
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move {
                    moon_core::db::tuner_smart::smart_suggest(
                        &q, rounds, min_n, &locked, edges, round,
                    )
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.sugg_seq != req {
                        return;
                    }
                    this.tuner.sugg_busy = false;
                    // A non-successful read must not look like "found nothing": the
                    // button would just stop spinning and leave no trace.
                    let sugg = match sugg {
                        Ok(v) => v,
                        Err(e) => {
                            log::warn!("analytics: smart suggestion failed — {e}");
                            if this.tuner.seq == stats_req {
                                this.tuner.stats.apply(Err(e));
                            }
                            cx.notify();
                            return;
                        }
                    };
                    if let Some(res) = sugg {
                        log::info!(
                            "analytics: smart suggestion — profit {:+.2}, trades {}, rounds {}",
                            res.profit,
                            res.n,
                            res.rounds
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
                });
            });
        })
        .detach();
    }

    /// Suggest the best v1 range for the selected field.
    ///
    /// `NotReady` and failed reads are published through the shared KPI load
    /// state; a valid `None` means no threshold improves on the baseline.
    pub(in crate::analytics::tuner) fn suggest_one_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        // The shared KPI error channel requires the current KPI generation too.
        let stats_req = self.tuner.seq;
        self.tuner.sugg_busy = true;
        let fi = self.tuner.sel_field;
        let field = FIELDS[fi].col.to_string();
        let q = self.tuner_query();
        let min_n = self.suggest_min_n();
        let edges = self.suggest_edges();
        let round = self.tuner.round_results;
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move {
                    moon_core::db::tuner::suggest_field(&q, &field, min_n, edges, round)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.sugg_seq != req {
                        return;
                    }
                    this.tuner.sugg_busy = false;
                    match sugg {
                        Ok(Some(s)) => {
                            let from = s.from.map(fmt_bound).unwrap_or_default();
                            let to = s.to.map(fmt_bound).unwrap_or_default();
                            this.apply_bounds(0, fi, from, to, cx);
                        }
                        // A genuine "no threshold beats the baseline".
                        Ok(None) => {}
                        Err(e) => {
                            log::warn!("analytics: threshold suggestion failed — {e}");
                            if this.tuner.seq == stats_req {
                                this.tuner.stats.apply(Err(e));
                            }
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
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

    /// Minimum trades for auto-suggestion: from the config string; empty = auto (1/5
    /// of the scope's actual trades).
    fn suggest_min_n(&self) -> i64 {
        if let Ok(v) = self.tuner.min_trades.trim().parse::<i64>() {
            return v.max(1);
        }
        self.tuner
            .stats
            .data()
            .and_then(|s| s.first().map(|f| f.n / 5))
            .unwrap_or(0)
            .max(1)
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

    /// "To strategy" for the "By time" axis: writes v1 into the `WorkingWeekTime`
    /// (day span) and `WorkingTime` (time window) fields of the selected strategy through
    /// the SAME confirmation dialog (`open_change_dialog`) as the thresholds. Only
    /// NON-EMPTY and changed fields are written (an empty field is left alone, the
    /// schedule is never wiped). The MoonBot field formats are unconfirmed — the strings
    /// are visible in the confirmation dialog.
    pub(in crate::analytics::tuner) fn time_open_save_dialog(&mut self, cx: &mut Context<Self>) {
        let targets = self.selected_targets();
        if targets.is_empty() {
            return;
        }
        // Single: same fields that light "Save" amber (`is_dirty`) so the button state and
        // the write permission agree. Multi: the forced set pushes the schedule to every
        // target regardless of the anchor's current values, and disables IgnoreTime/
        // IgnoreFilters so it actually applies.
        let bulk = targets.len() > 1;
        let changes = if bulk {
            self.time_tuner.changes_forced()
        } else {
            self.time_tuner.changes()
        };
        if changes.is_empty() {
            log::info!("analytics: 'Save' (time) — nothing to write (fields empty or = current)");
            return;
        }
        self.open_change_dialog(targets, changes, None, Vec::new(), Vec::new(), false, cx);
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

    /// "Make a copy" for the "By time" axis: the same changes as Save (schedule +
    /// IgnoreTime/IgnoreFilters), but the target is a NEW strategy (a copy of the selected
    /// one). Shares the filter path through `open_copy_with`; empty changes are fine (a
    /// plain duplicate).
    pub(in crate::analytics::tuner) fn time_open_copy_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Copy is single-target only (button hidden in multi-select) — take the anchor.
        let Some(target) = self.selected_targets().into_iter().next() else {
            return;
        };
        let mut changes = self.time_tuner.changes();
        changes.push(self.analyzer_comment());
        self.open_copy_with(target, changes, Vec::new(), window, cx);
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
        let proposed = crate::strategies::tree_ops::unique_name(&taken, &target.name);
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
    fn analyzer_comment(&self) -> (String, String) {
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
