//! Threshold tuner over the report's market fields — the 'Filters' mode of the 'Strategies' tab:
//! a 'Fact vs variants' KPI matrix, a
//! from/to range builder per field and a profit histogram over the quantile buckets of
//! the selected field. The scope is the strategy selected in the list (or all). Bounds retain
//! the raw strings the user typed and start empty on every open because they describe one
//! strategy's search. Only the search SETTINGS persist — restart count, quantile depth, seed,
//! train share, which fields the checkboxes admit, and whether the search composes their set.

// Files of this axis.
/// Its auto-suggestion and the write it produces.
mod actions;
/// Its histogram of the selected field.
mod hist;
/// Its own state: the threshold grid, the staged ignore flags, the suggestion settings.
pub(in crate::analytics::tuner) mod state;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::super::{AnalyticsView, LoadState};
pub(in crate::analytics::tuner) use super::shared::{N_VAR, TunerKind, card, glyph_btn};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::threshold_search::{ComposeDecision, ComposeSkip};
use moon_core::db::tuner::{FIELDS, FieldClass, StratFilters};
pub(in crate::analytics::tuner) use state::{flag_of, fmt_bound, parse_num, staged_dirty};

/// Histogram buckets.
const HIST_BUCKETS: usize = 14;

impl AnalyticsView {
    /// Admit or drop ONE field from the automatic search — the single writer of the checkbox mask.
    ///
    /// A funnel rather than three inline handlers, mirroring `toggle_time_field` on the By-time
    /// axis: the mask is now persisted, so every writer also has to retire the suggestion and
    /// save. With one door that is structural instead of a rule each future handler must recall.
    ///
    /// Args:
    ///     field: Index into `FIELDS`.
    ///     on: Whether the field takes part in the search.
    ///     cx: GPUI context used to persist and repaint.
    fn set_field_enabled(&mut self, field: usize, on: bool, cx: &mut Context<Self>) {
        self.tuner.enabled[field] = on;
        self.tuner.invalidate_suggest();
        self.persist_tuner_fields(cx);
        cx.notify();
    }

    /// The grid header's master checkbox: admit or drop every field at once.
    ///
    /// Unmapped fields stay out even when switching everything on — "all enabled" means all
    /// MAPPED, because a field with no strategy parameter has nowhere to write its threshold.
    ///
    /// Args:
    ///     on: Whether every mapped field takes part in the search.
    ///     cx: GPUI context used to persist and repaint.
    fn set_all_fields_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        for (fi, spec) in FIELDS.iter().enumerate() {
            self.tuner.enabled[fi] = on && spec.mapped();
        }
        self.tuner.invalidate_suggest();
        self.persist_tuner_fields(cx);
        cx.notify();
    }

    /// Tuner query: the shared filters plus the scope of EVERY selected strategy.
    ///
    /// The whole selection, not just the anchor row: the KPI matrix, histogram, and sweep must
    /// describe the same Ctrl- or Shift-built set the user sees highlighted and Save writes to.
    pub(in crate::analytics::tuner) fn tuner_query(&self) -> moon_core::db::analytics::Query {
        let mut q = self.query();
        // Row keys are `strategyid@core_uid`, so the scope is per strategy AND per core.
        q.strategies = self
            .selected_targets()
            .into_iter()
            .map(|t| (t.sid, t.core))
            .collect();
        q
    }

    /// Recompute the per-variant KPI matrix and selected strategy thresholds.
    ///
    /// Args:
    ///     cx: GPUI context used to run and publish the background read.
    pub(in crate::analytics) fn reload_tuner(&mut self, cx: &mut Context<Self>) {
        self.reload_tuner_inner(false, true, cx);
    }

    /// Recompute report-derived filter data without invalidating a pending Save dialog.
    ///
    /// The histogram starts only after this query completes so an automatic refresh never runs
    /// two full-history filter scans concurrently.
    ///
    /// Args:
    ///     show_overlay: Whether queued user work requires blocking progress feedback.
    ///     cx: GPUI context used to execute the serialized background reads.
    pub(in crate::analytics) fn reload_tuner_after_report(
        &mut self,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        self.reload_tuner_inner(true, show_overlay, cx);
    }

    /// Start the KPI query with behavior selected by its reload cause.
    ///
    /// Args:
    ///     after_report: Whether to preserve the Save dialog and read KPI/histogram atomically.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     cx: GPUI context used to execute and publish the reads.
    fn reload_tuner_inner(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        if !after_report {
            self.report_busy_retries.reset();
            self.tuner.mark_dialog_draft_changed();
        }
        self.tuner.seq = self.tuner.seq.wrapping_add(1);
        let req = self.tuner.seq;
        let report_req = self.current_report_generation();
        let q = self.tuner_query();
        // The "in strategy" chips are the ANCHOR's own thresholds — a per-strategy value
        // that only means something for a lone selection (`current_if_single` blanks them
        // otherwise). So they are read from the anchor, NOT from the multi-strategy scope
        // of `q`: the diff Save computes must be against the core the write targets.
        let anchor = self
            .sel_strategy
            .as_ref()
            .and_then(|(k, _)| super::parse_strat_key(k));
        let sid = anchor.map(|(s, _)| s);
        let core = anchor.and_then(|(_, c)| c);
        let variants = self.tuner.variants();
        let field = FIELDS[self.tuner.sel_field].col.to_string();
        let hist_req = after_report.then(|| {
            self.tuner.hist_seq = self.tuner.hist_seq.wrapping_add(1);
            self.tuner.hist_dirty = false;
            self.tuner.hist_loading = true;
            self.tuner.hist.begin();
            self.tuner.hist_seq
        });
        // Core schema defaults (numeric fields): the chips hide values equal to the
        // default — 'filter not configured' is not a threshold.
        let defaults: HashMap<String, f64> = {
            let b = self.backend.read(cx);
            let store = b.session.store();
            let mut out = HashMap::new();
            for (_, cd) in store.cores() {
                let Some(sch) = cd.schema.as_ref() else {
                    continue;
                };
                for k in &sch.kinds {
                    for s in &k.sections {
                        for f in &s.fields {
                            let Some(d) = f.default.as_ref() else {
                                continue;
                            };
                            if let Ok(v) = d
                                .trim()
                                .trim_end_matches('%')
                                .replace(',', ".")
                                .parse::<f64>()
                            {
                                out.entry(f.name.to_ascii_lowercase()).or_insert(v);
                            }
                        }
                    }
                }
                break; // the cores' schemas are identical — one is enough
            }
            out
        };
        // Starting a KPI recomputation does not by itself retire the auto-suggestion. Input and
        // v1-destination edits retire it at their mutation sites, while unrelated refreshes may
        // carry the last completed verdict until one of those inputs changes.
        // Carry current numbers as stale during recomputation; any completed
        // non-data result drops them.
        self.tuner.stats.begin();
        self.spawn_db(
            show_overlay,
            cx,
            move || {
                let (stats, histogram) = if after_report {
                    let result = moon_core::db::tuner::filter_tuner_data(
                        &q,
                        &variants,
                        &field,
                        HIST_BUCKETS,
                    );
                    (result.stats, Some(result.histogram))
                } else {
                    (moon_core::db::tuner::variant_stats(&q, &variants), None)
                };
                let sf = sid
                    .map(|sid| moon_core::db::tuner::strategy_filters(sid, core, &defaults))
                    .unwrap_or_default();
                (stats, histogram, sf)
            },
            move |this, (stats, histogram, strat), cx| {
                let mut hist_error = None;
                if let (Some(hist_req), Some(histogram)) = (hist_req, histogram) {
                    if this.tuner.hist_seq == hist_req {
                        this.tuner.hist_loading = false;
                        hist_error = histogram.as_ref().err().cloned();
                        this.tuner.hist.apply(histogram);
                        this.tuner.hist_dirty = super::super::refresh::report_result_is_stale(
                            report_req,
                            this.current_report_generation(),
                            hist_error.is_some(),
                        );
                    }
                }
                if this.tuner.seq != req {
                    if let Some(error) = hist_error.as_ref() {
                        this.settle_report_refresh_retry(Some(error), cx);
                    }
                    cx.notify();
                    return;
                }
                let error = stats.as_ref().err().cloned();
                // A completed non-data result clears stale numbers because
                // values under a changed period label must belong to it.
                this.tuner.stats.apply(stats);
                // `strategy_filters` is intentionally lossy and reports an unreadable row as
                // `found=false`. An automatic report refresh must not turn that ambiguity into
                // an empty Save baseline; explicit scope changes may clear the old strategy.
                this.tuner.apply_strategy_read(strat, after_report);
                this.tuner.dirty = super::super::refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    error.is_some(),
                );
                let retry_error = error
                    .as_ref()
                    .filter(|error| error.kind() == Some(moon_core::db::FailKind::Busy))
                    .or_else(|| {
                        hist_error
                            .as_ref()
                            .filter(|error| error.kind() == Some(moon_core::db::FailKind::Busy))
                    });
                this.settle_report_refresh_retry(retry_error, cx);
                cx.notify();
            },
        );
    }

    /// Recompute the selected field's histogram after a user-controlled change.
    ///
    /// Args:
    ///     cx: GPUI context used to run and publish the background read.
    pub(in crate::analytics) fn reload_hist(&mut self, cx: &mut Context<Self>) {
        self.reload_hist_inner(cx);
    }

    /// Start a user-requested histogram query.
    ///
    /// Args:
    ///     cx: GPUI context used to execute and publish the histogram read.
    fn reload_hist_inner(&mut self, cx: &mut Context<Self>) {
        self.tuner.hist_seq = self.tuner.hist_seq.wrapping_add(1);
        let req = self.tuner.hist_seq;
        let report_req = self.current_report_generation();
        let q = self.tuner_query();
        let field = FIELDS[self.tuner.sel_field].col.to_string();
        self.tuner.hist_dirty = false;
        self.tuner.hist_loading = true;
        self.tuner.hist.begin();
        self.spawn_db(
            true,
            cx,
            move || moon_core::db::tuner::histogram(&q, &field, HIST_BUCKETS),
            move |this, hist, cx| {
                if this.tuner.hist_seq != req {
                    return;
                }
                this.tuner.hist_loading = false;
                let error = hist.as_ref().err().cloned();
                this.tuner.hist.apply(hist);
                this.tuner.hist_dirty = super::super::refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    error.is_some(),
                );
                cx.notify();
            },
        );
    }

    /// Commit a bound (on input Blur/Enter): store it in the tuner state and recompute.
    ///
    /// The bound lives only in memory — it describes the search for one strategy, so it is not
    /// persisted and starts empty on the next window open.
    fn commit_bound(
        &mut self,
        vi: usize,
        fi: usize,
        is_to: bool,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let slot = &mut self.tuner.bounds[vi][fi];
        let cur = if is_to { &mut slot.1 } else { &mut slot.0 };
        if *cur == value {
            return;
        }
        *cur = value;
        if vi == 0 {
            self.tuner.invalidate_suggest();
        }
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Programmatic set of BOTH bounds of a field (strategy chip / clear /
    /// auto-suggestion): state + a silent resync of the inputs + recompute.
    pub(in crate::analytics::tuner) fn apply_bounds(
        &mut self,
        vi: usize,
        fi: usize,
        from: String,
        to: String,
        cx: &mut Context<Self>,
    ) {
        if self.tuner.bounds[vi][fi] == (from.clone(), to.clone()) {
            return;
        }
        self.tuner.bounds[vi][fi] = (from, to);
        if vi == 0 {
            self.tuner.invalidate_suggest();
        }
        // Recreate the inputs (drop the cache): a fresh default_value is drawn from
        // the START of the string; sync_value left the caret at the end — a long value
        // 'scrolled off' to the right and only its tail was visible.
        self.tuner.inputs.remove(&format!("tv{vi}f{fi}a"));
        self.tuner.inputs.remove(&format!("tv{vi}f{fi}b"));
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Bound input with a lazy cache (the field_input_state pattern from Strategies).
    fn bound_input(
        &mut self,
        vi: usize,
        fi: usize,
        is_to: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!("tv{vi}f{fi}{}", if is_to { "b" } else { "a" });
        if let Some(state) = self.tuner.inputs.get(&id) {
            return state.clone();
        }
        let slot = &self.tuner.bounds[vi][fi];
        let value = if is_to {
            slot.1.clone()
        } else {
            slot.0.clone()
        };
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            // Typing alone retires a running v1 suggestion, without committing the half-typed
            // value or recomputing. The joint search runs WITHOUT the blocking overlay, so the
            // user can type into this box while it works — and its completion overwrites every
            // enabled v1 bound and drops the input entity. Waiting for Blur to notice would
            // silently throw that edit away.
            if vi == 0 && matches!(ev, MoonInputEvent::Change) {
                this.tuner.invalidate_suggest();
            }
            if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                let value = state.read(cx).value().to_string();
                this.commit_bound(vi, fi, is_to, value, cx);
            }
        })
        .detach();
        self.tuner.inputs.insert(id, state.clone());
        state
    }

    /// Range builder: one row per field, clicking a name shows that field's histogram.
    pub(in crate::analytics::tuner) fn fields_grid(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_w = 60.0;
        let mut head = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(
                div().flex_none().child(
                    MoonCheckbox::new("tun-en-all")
                        // The master checkbox ignores unmapped fields (no matching
                        // strategy parameter): 'all enabled' = all MAPPED ones.
                        .checked(self.tuner.all_mapped_enabled())
                        .size(MoonCheckboxSize::Compact)
                        .on_change({
                            let view = cx.entity();
                            move |ch: &bool, _w, app| {
                                let on = *ch;
                                view.update(app, |this, cx| this.set_all_fields_enabled(on, cx));
                            }
                        }),
                ),
            )
            // The column caption doubles as the master checkbox's label, by being a click target
            // rather than by becoming one. `MoonCheckbox::label` renders at CONTENT width, so
            // moving the text onto the checkbox would widen it by however long the word is in the
            // active language and shift every heading after it out of line with its own column —
            // worst in ru/es, where this header is already the widest. Clicking here routes
            // through `set_all_fields_enabled` like the checkbox itself, so the mask keeps its two
            // writers.
            .child(
                div()
                    .id("tun-en-all-lbl")
                    .w(design::font_w_px(cx, 58.0))
                    .flex_none()
                    .truncate()
                    .cursor_pointer()
                    .child(t!("analytics.tuner.field").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let all_on = this.tuner.all_mapped_enabled();
                        this.set_all_fields_enabled(!all_on, cx);
                    })),
            )
            // Chip column: the selected strategy's thresholds (click sends them to v1).
            .child(
                div()
                    .flex_1()
                    .child(t!("analytics.tuner.strat_chip").to_string()),
            );
        for vi in 0..N_VAR {
            head = head
                .child(
                    div()
                        .w(design::font_w_px(cx, in_w))
                        .flex_none()
                        .text_center()
                        .child(format!("v{} {}", vi + 1, t!("analytics.tuner.from"))),
                )
                // The only two copy buttons, both "copy the WHOLE column", each between its
                // variant's "from" and "to": → carries v1→v2, ← carries v2→v1. Rows have none —
                // they keep a matching spacer so the columns stay aligned.
                .child(if vi == 0 {
                    glyph_btn(
                        "tun-cp-col",
                        "→",
                        t!("analytics.time.tip_to_v2").to_string(),
                        p.amber,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.copy_v1_to_v2(None, cx)))
                } else {
                    glyph_btn(
                        "tun-cpb-col",
                        "←",
                        t!("analytics.time.tip_to_v1").to_string(),
                        p.amber,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.copy_v2_to_v1(None, cx)))
                })
                .child(
                    div()
                        .w(design::font_w_px(cx, in_w))
                        .flex_none()
                        .text_center()
                        .child(t!("analytics.tuner.to").to_string()),
                )
                // Clear the WHOLE variant column — above the per-row crosses.
                .child(
                    glyph_btn(
                        SharedString::from(format!("tun-clr-col-{vi}")),
                        "✕",
                        t!("analytics.time.tip_clear_all").to_string(),
                        p.orange,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.clear_variant(vi, cx))),
                );
        }

        // The per-field "in strategy" chips and the group ignore=YES/NO labels are the ANCHOR's
        // only; in multi-select each selected strategy differs, so `current_if_single` yields
        // None and we render from an empty card (found=false hides both). Display-only — the
        // write still reads `self.tuner.strat`.
        let strat = self
            .current_if_single(self.tuner.strat.clone())
            .unwrap_or_else(|| Arc::new(StratFilters::default()));
        let mut grid = v_flex().w_full().child(head);
        let mut last_class: Option<FieldClass> = None;
        for fi in 0..FIELDS.len() {
            let class = FIELDS[fi].class;
            // Headers: the MoonBot section (the parent), then the indented
            // subgroup (BV/SV inside Volumes, the Δ2/Δ3 slots inside Deltas).
            let sub = class.parent() != class;
            if last_class != Some(class) {
                if sub && last_class.map(|c| c.parent()) != Some(class.parent()) {
                    grid = grid.child(self.group_header(class.parent(), &strat, p, cx));
                }
                last_class = Some(class);
                grid = grid.child(self.group_header(class, &strat, p, cx));
            }
            let selected = self.tuner.sel_field == fi;
            let unmapped = !FIELDS[fi].mapped();
            let mut row = h_flex()
                .id(SharedString::from(format!("tun-field-{fi}")))
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .when(sub, |el| el.pl(design::ui_px(cx, 22.0)))
                .py(design::ui_px(cx, 2.0))
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.5))
                .child(
                    div().flex_none().child(
                        MoonCheckbox::new(SharedString::from(format!("tun-en-{fi}")))
                            .checked(self.tuner.enabled[fi])
                            .size(MoonCheckboxSize::Compact)
                            .on_change({
                                let view = cx.entity();
                                move |ch: &bool, _w, app| {
                                    let on = *ch;
                                    view.update(app, |this, cx| this.set_field_enabled(fi, on, cx));
                                }
                            }),
                    ),
                )
                .child(
                    div()
                        .w(design::font_w_px(cx, 58.0))
                        .flex_none()
                        .truncate()
                        .cursor_pointer()
                        .text_color(if selected {
                            moon(p.amber)
                        } else if unmapped {
                            // Field with no strategy parameter — dim it.
                            moon(p.text_muted)
                        } else {
                            moon(p.text)
                        })
                        .child(FIELDS[fi].label.to_string()),
                )
                // How many walk-forward folds also gave this field a range, when the last search
                // composed the set. It is the only per-field figure here that was not chosen for,
                // so it says something the thresholds beside it cannot: "3/3" is a field that
                // earned its place in every stretch of the period, "1/3" one that earned it in a
                // single one and may well be an artefact of that stretch.
                .when_some(self.tuner.compose_support(fi), |el, (support, folds)| {
                    el.child(
                        div()
                            .flex_none()
                            .truncate()
                            .text_color(if support == folds {
                                moon(design::positive_color(p))
                            } else {
                                moon(p.text_muted)
                            })
                            .child(format!("{support}/{folds}")),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.tuner.sel_field != fi {
                        this.tuner.sel_field = fi;
                        // Another field's histogram is not this field's stale
                        // data — drop it outright rather than carrying it.
                        this.tuner.hist = LoadState::default();
                        this.reload_hist(cx);
                        cx.notify();
                    }
                }));
            if selected {
                row = row.bg(moon_alpha(p.amber, 0.08));
            }
            // Chip: the selected strategy's NON-default thresholds (informational).
            // Slot fields show the 'Δ2/Δ3' assignment plus the slot's thresholds. If
            // the class is ignored by the flags we show NO values (the 'ignore' label
            // sits on the group's subheader).
            let chip: Option<(Option<u8>, Option<f64>, Option<f64>)> =
                if strat.found && !strat.class_ignored(class) {
                    if class == FieldClass::DeltaSlot {
                        strat
                            .slot_of(FIELDS[fi].col)
                            .map(|(n, lo, hi)| (Some(n), lo, hi))
                    } else {
                        strat
                            .bounds
                            .get(FIELDS[fi].col)
                            .copied()
                            .map(|(lo, hi)| (None, lo, hi))
                    }
                } else {
                    None
                };
            row = row.child(match chip {
                Some((slot, lo, hi)) => {
                    // Compact range: '(from…to)'; an open side stays empty.
                    let range = (lo.is_some() || hi.is_some()).then(|| {
                        format!(
                            "({}…{})",
                            lo.map(fmt_bound).unwrap_or_default(),
                            hi.map(fmt_bound).unwrap_or_default()
                        )
                    });
                    let text = [slot.map(|n| format!("Δ{n}")), range]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ");
                    // Clicking the chip: the strategy's values → v1 (replacing them) +
                    // clear the participation checkbox — the field becomes a FIXED
                    // filter that the sweep leaves alone.
                    let (from_s, to_s) = (
                        lo.map(fmt_bound).unwrap_or_default(),
                        hi.map(fmt_bound).unwrap_or_default(),
                    );
                    div()
                        .id(SharedString::from(format!("tun-chip-{fi}")))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .cursor_pointer()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.amber))
                        .hover(move |st| st.text_color(moon(p.text)))
                        .child(text)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_field_enabled(fi, false, cx);
                            this.apply_bounds(0, fi, from_s.clone(), to_s.clone(), cx);
                            cx.notify();
                        }))
                        .into_any_element()
                }
                // Unmapped field: instead of a chip, a note that there is nothing to
                // write the fitted threshold into on the strategy (no such parameter).
                None if unmapped => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_caption(cx))
                    .text_color(moon_alpha(p.text_muted, 0.7))
                    .child(t!("analytics.tuner.no_param").to_string())
                    .into_any_element(),
                None => div().flex_1().into_any_element(),
            });
            for vi in 0..N_VAR {
                for is_to in [false, true] {
                    // Spacer under the header's copy arrow (it sits between "from" and "to"), so
                    // the row columns stay aligned with the header.
                    if is_to {
                        row = row.child(div().w(design::ui_px(cx, 12.0)).flex_none());
                    }
                    let input = self.bound_input(vi, fi, is_to, window, cx);
                    row = row.child(
                        div().w(design::font_w_px(cx, in_w)).flex_none().child(
                            MoonInput::new(SharedString::from(format!("tun-in-{vi}-{fi}-{is_to}")))
                                .state(&input)
                                .small(),
                        ),
                    );
                }
                // Clear both bounds of this variant in this row.
                row = row.child(
                    glyph_btn(
                        SharedString::from(format!("tun-clr-{vi}-{fi}")),
                        "✕",
                        t!("analytics.time.tip_clear").to_string(),
                        p.orange,
                        p,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.apply_bounds(vi, fi, String::new(), String::new(), cx)
                    })),
                );
            }
            grid = grid.child(row);
        }
        // The suggestion row and the toolbar come from the SHARED shell (tuner_shell), axis
        // 'By filter': one code path for every tuner, actions dispatched by axis.
        let cfg_row = self.shell_config_row(TunerKind::Filter, p, window, cx);
        let header = self.shell_toolbar(
            TunerKind::Filter,
            t!("analytics.tuner.fields_title").to_string(),
            p,
            cx,
        );
        v_flex()
            .w_full()
            // Fill the column and scroll the field rows internally (toolbar/config pinned), so
            // the parameters never spill off the bottom of a short panel.
            .flex_1()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(header)
            .child(cfg_row)
            .child(
                div()
                    .id("an-fields-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(grid),
            )
            .children(self.split_summary(p, cx))
            .into_any_element()
    }

    /// What the last suggestion's ranges achieved, split in two by whether the search was allowed
    /// to see the trades being measured.
    ///
    /// Pinned BELOW the scrolling rows rather than placed inside them: the fitted profit is the
    /// number the search maximized and always looks good, so the held-back one has to be visible
    /// at the same moment, not somewhere the user has to scroll to. Nothing is drawn until a
    /// search has produced an answer — an untouched grid has nothing to report — and editing any
    /// bound retires the suggestion, taking this with it.
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Fit/holdout summary when a completed suggestion has something to disclose.
    fn split_summary(&self, p: MoonPalette, cx: &Context<Self>) -> Option<AnyElement> {
        let state::SuggestState::Done {
            split: Some(split), ..
        } = &self.tuner.sugg
        else {
            return None;
        };
        // Nothing was held back, so there is no second opinion to show and the KPI matrix above
        // already carries the fitted figures. A COMPOSED set still has to be reported in that
        // case — it is a claim about which fields matter, and silence about it would be the one
        // reading it must not get: that no set was chosen.
        let holdout = split.holdout.as_ref();
        if holdout.is_none() && split.composed.is_none() && split.compose_skipped.is_none() {
            return None;
        }
        let line = |caption: String, tally: &moon_core::db::metrics::Tally, color: u32| {
            h_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .w(design::font_w_px(cx, 74.0))
                        .flex_none()
                        .truncate()
                        .text_color(moon(p.text_muted))
                        .child(caption),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(moon(color))
                        // The same formatters the KPI matrix above uses for the same figures:
                        // winrate to one decimal as the strategy table shows it, profit factor
                        // and drawdown through the shared dimensionless one. Only the headline
                        // profit carries the unit — the line has to survive truncation on a
                        // narrow dock, and repeating the quote ticker inside it spends that width twice.
                        .child(
                            t!(
                                "analytics.tuner.split_stats",
                                profit = super::super::summary::fmt_signed(tally.profit),
                                n = tally.n,
                                wr = format!("{:.1}", tally.winrate()),
                                pf = super::super::summary::fmt_signed_plain(tally.profit_factor()),
                                dd = super::super::summary::fmt_signed_plain(tally.max_dd)
                            )
                            .to_string(),
                        ),
                )
        };
        Some(
            v_flex()
                .w_full()
                .flex_none()
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 6.0))
                .gap(design::ui_px(cx, 2.0))
                .border_t_1()
                .border_color(moon(p.border))
                .text_size(design::t_caption(cx))
                // The composed set sits in the SAME pinned block as the held-back figure, and
                // above it, deliberately. "Four fields chosen" reads as an achievement on its
                // own; the number that says whether those four survived the period nobody fitted
                // has to be in the same glance, not somewhere the user could scroll away from.
                .when_some(split.composed.as_ref(), |el, set| {
                    let decision = match set.decision {
                        ComposeDecision::ReducedSet => {
                            t!("analytics.tuner.compose_decision_reduced")
                        }
                        ComposeDecision::AllAllowedFields => {
                            t!("analytics.tuner.compose_decision_all")
                        }
                        ComposeDecision::NoAdditionalFilters => {
                            t!("analytics.tuner.compose_decision_none")
                        }
                    };
                    let fields: Vec<String> = set
                        .fields
                        .iter()
                        .map(|(col, support)| {
                            let label = FIELDS
                                .iter()
                                .find(|s| s.col == *col)
                                .map_or(*col, |s| s.label);
                            format!("{label} {support}/{}", set.folds)
                        })
                        .collect();
                    let el = el.child(
                        h_flex()
                            .w_full()
                            .gap(design::ui_px(cx, 6.0))
                            .child(
                                div()
                                    .w(design::font_w_px(cx, 74.0))
                                    .flex_none()
                                    .truncate()
                                    .text_color(moon(p.text_muted))
                                    .child(t!("analytics.tuner.compose_result").to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(moon(p.text_soft))
                                    .child(decision.to_string()),
                            ),
                    );
                    // The result line names the selected path. This second line is detail only:
                    // applied fields and their support, or a disagreement between the selected
                    // mask and the later full-budget refit.
                    let text = if !fields.is_empty() {
                        fields.join(" · ")
                    } else {
                        t!(
                            "analytics.tuner.compose_refit_dropped",
                            n = set.dropped_at_refit
                        )
                        .to_string()
                    };
                    if fields.is_empty() && set.dropped_at_refit == 0 {
                        el
                    } else {
                        el.child(
                            h_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 6.0))
                                .child(
                                    div()
                                        .w(design::font_w_px(cx, 74.0))
                                        .flex_none()
                                        .truncate()
                                        .text_color(moon(p.text_muted))
                                        .child(t!("analytics.tuner.compose_set").to_string()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(moon(p.text_soft))
                                        .child(text),
                                ),
                        )
                    }
                })
                // Composition was asked for and could not compare its three paths. State both
                // what DID run and why the comparison was unavailable.
                .when_some(split.compose_skipped, |el, why| {
                    // Each reason asks for a different action — widen the period, or pick a
                    // strategy — so the executed path and the reason remain separate lines.
                    let text = match why {
                        ComposeSkip::TooFewFolds => t!("analytics.tuner.compose_small"),
                        ComposeSkip::NoStrategyScope => t!("analytics.tuner.compose_no_scope"),
                    };
                    el.child(
                        h_flex()
                            .w_full()
                            .gap(design::ui_px(cx, 6.0))
                            .child(
                                div()
                                    .w(design::font_w_px(cx, 74.0))
                                    .flex_none()
                                    .truncate()
                                    .text_color(moon(p.text_muted))
                                    .child(t!("analytics.tuner.compose_result").to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(moon(p.text_soft))
                                    .child(
                                        t!("analytics.tuner.compose_decision_all_direct")
                                            .to_string(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_color(moon(p.orange))
                            .child(text.to_string()),
                    )
                })
                .child(line(
                    t!("analytics.tuner.split_train").to_string(),
                    &split.train,
                    p.text_soft,
                ))
                // A losing holdout is the whole point of the exercise: the ranges fitted the past
                // and did not survive contact with the rest of it. It is coloured as the failure
                // it is rather than left to be read off a sign.
                .when_some(holdout, |el, holdout| {
                    el.child(line(
                        t!("analytics.tuner.split_holdout").to_string(),
                        holdout,
                        if holdout.profit > 0.0 {
                            design::positive_color(p)
                        } else {
                            design::danger_color(p)
                        },
                    ))
                })
                // A composed set with nothing held back is a set chosen with no way to check it.
                // The warning belongs beside the set, where it is read, not only in the popover
                // where the share was chosen.
                .when(holdout.is_none() && split.composed.is_some(), |el| {
                    el.child(
                        div()
                            .w_full()
                            .truncate()
                            .text_color(moon(p.orange))
                            .child(t!("analytics.tuner.compose_no_split").to_string()),
                    )
                })
                .into_any_element(),
        )
    }

    /// Group subheader: the label plus a clickable 'ignore' (which is staged) and an
    /// 'apply' when the staged value differs from the strategy's current flag — it
    /// writes ONLY that flag TO THE STRATEGY (putting the ignore back is as easy as
    /// clearing it by saving thresholds).
    fn group_header(
        &self,
        class: FieldClass,
        strat: &StratFilters,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let label = match class {
            FieldClass::Filter => t!("analytics.tuner.grp_filter"),
            FieldClass::BvSv => t!("analytics.tuner.grp_bvsv"),
            FieldClass::Ping => t!("analytics.tuner.grp_ping"),
            FieldClass::Base => t!("analytics.tuner.grp_base"),
            FieldClass::DeltaSlot => t!("analytics.tuner.grp_slot"),
            FieldClass::Delta => t!("analytics.tuner.grp_delta"),
            FieldClass::Volume => t!("analytics.tuner.grp_volume"),
        }
        .to_string();
        // Subgroup of a MoonBot section (BV/SV in Volumes, the Δ2/Δ3 slots in Deltas):
        // indented + a paler background. Slots have NO flag of their own (the gate is
        // the parent's IgnoreDelta), so no clickable ignore; BV/SV has its own toggle.
        let sub = class.parent() != class;
        let mut hdr = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .when(sub, |el| el.pl(design::ui_px(cx, 22.0)))
            .py(design::ui_px(cx, 2.0))
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .bg(moon_alpha(p.table_head, if sub { 0.35 } else { 0.6 }))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.7))
            .text_size(design::t_caption(cx))
            .child(div().text_color(moon(p.text_soft)).child(label));
        if strat.found && !(sub && class != FieldClass::BvSv) {
            let (flag, cur_ignore) = flag_of(class, strat);
            let staged = self.tuner.staged_ignore.get(flag).copied();
            let shown = staged.unwrap_or(cur_ignore);
            hdr = hdr.child(
                div()
                    .id(SharedString::from(format!("tun-ign-{flag}")))
                    .cursor_pointer()
                    .text_color(if shown {
                        // The class's filters are IGNORED — dark grey.
                        moon_alpha(p.text_muted, 0.7)
                    } else {
                        // The filters are live — green.
                        moon(p.green)
                    })
                    .child(if shown { "ignore=YES" } else { "ignore=NO" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let (_, cur) = flag_of(class, &this.tuner.strat.clone());
                        let now = this.tuner.staged_ignore.get(flag).copied().unwrap_or(cur);
                        if !now == cur {
                            this.tuner.staged_ignore.remove(flag);
                        } else {
                            this.tuner.staged_ignore.insert(flag, !now);
                        }
                        this.tuner.mark_dialog_draft_changed();
                        cx.notify();
                    })),
            );
        }
        hdr.into_any_element()
    }
}
