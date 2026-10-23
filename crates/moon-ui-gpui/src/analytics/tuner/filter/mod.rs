//! Threshold tuner over the report's market fields — the 'Filters' mode of the 'Strategies' tab:
//! a 'Fact vs variants' KPI matrix, a
//! from/to range builder per field and a profit histogram over the quantile buckets of
//! the selected field. The scope is the strategy selected in the list (or all). Bounds retain
//! the raw strings the user typed and start empty on every open because they describe one
//! strategy's search. Only the search SETTINGS persist — restart count, quantile depth, seed,
//! train share, which fields the checkboxes admit, and whether the search composes their set.
//! The Detect group is offered only when every strategy in the current scope is a Drops detect,
//! and its parameter is never written onto a strategy of another kind.

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
    MoonCheckbox, MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::refresh::CatchUpOutcome;
use super::super::{AnalyticsView, LoadState};
pub(in crate::analytics::tuner) use super::shared::{TunerKind, card, glyph_btn};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;
use moon_core::db::tuner::threshold_search::{ComposeDecision, ComposeSkip};
use moon_core::db::tuner::{FIELDS, FieldClass, StratFilters};
pub(in crate::analytics::tuner) use state::{
    flag_of, fmt_bound, ignore_badge, parse_num, staged_dirty,
};

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
    /// A Detect field the scope is not showing is left as it was when turning the visible set
    /// on, and is cleared with the rest when turning everything off.
    ///
    /// Args:
    ///     on: Whether every mapped field takes part in the search.
    ///     cx: GPUI context used to persist and repaint.
    fn set_all_fields_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let drops_only = self.scope_is_drops_only();
        for (fi, spec) in FIELDS.iter().enumerate() {
            let offered = offer_field(spec.class, drops_only);
            self.tuner.enabled[fi] = master_bit(self.tuner.enabled[fi], on, spec.mapped(), offered);
        }
        self.tuner.invalidate_suggest();
        self.persist_tuner_fields(cx);
        cx.notify();
    }

    /// Tuner query: the shared filters plus the scope of EVERY selected strategy.
    ///
    /// The whole read-visible selection, not just the anchor row: the KPI matrix, histogram, and
    /// sweep must describe the same Ctrl- or Shift-built set the user sees highlighted. Write
    /// targets are bounded separately by the Auto group's action authority.
    pub(in crate::analytics::tuner) fn tuner_query(&self) -> moon_core::db::analytics::Query {
        let mut q = self.query();
        // Row keys are `strategyid@core_uid`, so the scope is per strategy AND per core.
        q.strategies = self.visible_target_keys(self.read_core_ids());
        q
    }

    /// Whether the read scope the By-filter tab already uses is entirely Drops detects.
    ///
    /// The strategy list's kind dropdown filters the table only. This reads the loaded rows,
    /// the same set `tuner_query` scopes, so hiding MoonShot in the list cannot reveal
    /// xPriceDelta while those deals are still in the search.
    fn scope_is_drops_only(&self) -> bool {
        let Some(data) = self.strategy_data.data() else {
            return false;
        };
        let selected = self.visible_target_keys(self.read_core_ids());
        drops_only_scope(&data.strategies, &selected)
    }

    /// SignalType of one write target, from the strategy list the page already loaded.
    ///
    /// `None` when that list is missing or has no row for the target. Save then treats the
    /// target as not Drops and strips a Detect parameter rather than guessing a kind.
    fn strategy_kind(&self, sid: i64, core: Option<u64>) -> Option<String> {
        self.strategy_data
            .data()
            .and_then(|data| kind_of(&data.strategies, sid, core).map(str::to_string))
    }

    /// Read numeric strategy-field defaults used to hide unconfigured threshold chips — the
    /// same table the Entry/Exit model fills defaulted fields from
    /// (`ticks::fetch::strategy_field_defaults`).
    ///
    /// Args:
    ///     cx: GPUI context used to read the backend schema store.
    ///
    /// Returns:
    ///     Lowercase field names mapped to their first available core-schema default.
    pub(in crate::analytics::tuner) fn filter_defaults(
        &self,
        cx: &Context<Self>,
    ) -> HashMap<String, f64> {
        super::ticks::strategy_field_defaults(self.backend.read(cx))
    }

    /// Recompute only KPI and selected strategy thresholds after a local tuner edit.
    ///
    /// Args:
    ///     cx: GPUI context used to run and publish the background read.
    pub(in crate::analytics) fn reload_tuner(&mut self, cx: &mut Context<Self>) {
        self.reload_filter_kpi_inner(false, true, true, cx);
    }

    /// Recompute the complete Filters axis from one prepared report source.
    ///
    /// Args:
    ///     cx: GPUI context used to run and publish the background read.
    pub(in crate::analytics) fn reload_filter_axis(&mut self, cx: &mut Context<Self>) {
        self.reload_filter_axis_inner(false, true, cx);
    }

    /// Recompute report-derived filter data without invalidating a pending Save dialog.
    ///
    /// KPI and histogram share one report snapshot and one prepared tuner source, so neither a
    /// manual refresh nor an automatic refresh repeats quote preflight and source construction.
    ///
    /// Args:
    ///     show_overlay: Whether queued user work requires blocking progress feedback.
    ///     cx: GPUI context used to execute the serialized background reads.
    pub(in crate::analytics) fn reload_tuner_after_report(
        &mut self,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        self.reload_filter_axis_inner(true, show_overlay, cx);
    }

    /// Start the compound KPI and histogram query for a whole-axis refresh.
    ///
    /// Args:
    ///     after_report: Whether to preserve the Save dialog and a missing strategy baseline.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     cx: GPUI context used to execute and publish the reads.
    fn reload_filter_axis_inner(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        if !after_report {
            self.report_busy_retries.reset();
            self.tuner.mark_dialog_draft_changed();
        }
        self.tuner.read_after_report = after_report;
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
        let variants = self.tuner.variants(self.scope_is_drops_only());
        let field = FIELDS[self.tuner.sel_field].col.to_string();
        self.tuner.hist_seq = self.tuner.hist_seq.wrapping_add(1);
        self.tuner.hist_dirty = false;
        self.tuner.hist_loading = true;
        self.tuner.hist.begin();
        let hist_req = self.tuner.hist_seq;
        let defaults = self.filter_defaults(cx);
        // Starting a KPI recomputation does not by itself retire the auto-suggestion. Input and
        // v1-destination edits retire it at their mutation sites, while unrelated refreshes may
        // carry the last completed verdict until one of those inputs changes.
        // Carry current numbers as stale during recomputation; any completed
        // non-data result drops them.
        self.tuner.stats.begin();
        self.spawn_latest_db(
            &[
                super::super::bg::ReadLane::FilterKpi,
                super::super::bg::ReadLane::FilterHistogram,
            ],
            show_overlay,
            cx,
            move || {
                let result =
                    moon_core::db::tuner::filter_tuner_data(&q, &variants, &field, HIST_BUCKETS);
                let sf = sid
                    .map(|sid| moon_core::db::tuner::strategy_filters(sid, core, &defaults))
                    .unwrap_or_default();
                (result.stats, result.histogram, sf)
            },
            move |this, (stats, histogram, strat), cx| {
                let mut hist_error = None;
                let mut hist_outcome = CatchUpOutcome::Replacement;
                if this.tuner.hist_seq == hist_req {
                    this.tuner.hist_loading = false;
                    hist_outcome = CatchUpOutcome::of_read(&histogram);
                    hist_error = histogram.as_ref().err().cloned();
                    let keep_hist = this.keep_on_catch_up(after_report, hist_outcome, report_req);
                    this.tuner.hist.apply_or_keep(histogram, keep_hist);
                    this.tuner.hist_dirty = super::super::refresh::report_result_is_stale(
                        report_req,
                        this.current_report_generation(),
                        hist_error.is_some(),
                    );
                }
                if this.tuner.seq != req {
                    if hist_error.is_some() {
                        this.settle_report_refresh_retry(hist_outcome.is_transient(), cx);
                    }
                    cx.notify();
                    return;
                }
                let stats_outcome = CatchUpOutcome::of_read(&stats);
                let error = stats.as_ref().err().cloned();
                // A completed non-data result clears stale numbers, unless it is a transient
                // outcome with a scheduled correction left (`keep_on_catch_up`) — values
                // under a changed period label must belong to it, but a settled snapshot with a
                // scheduled correction must not blink through the intermediate result.
                let keep_stats = this.keep_on_catch_up(after_report, stats_outcome, report_req);
                this.tuner.sample_snapshot.observe(stats.as_ref().ok());
                this.tuner.stats.apply_or_keep(stats, keep_stats);
                // `strategy_filters` is intentionally lossy and reports an unreadable row as
                // `found=false`. An automatic report refresh must not turn that ambiguity into
                // an empty Save baseline; explicit scope changes may clear the old strategy.
                this.tuner.apply_strategy_read(strat, after_report);
                this.tuner.dirty = super::super::refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    error.is_some(),
                );
                let transient = stats_outcome.is_transient() || hist_outcome.is_transient();
                this.settle_report_refresh_retry(transient, cx);
                cx.notify();
            },
        );
    }

    /// Start a KPI-only query while preserving an unrelated completed histogram.
    ///
    /// Args:
    ///     after_report: Whether to preserve a missing strategy baseline and settle report retry.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     user_edit: Whether this local edit invalidates a pending Save-dialog preview.
    ///     cx: GPUI context used to execute and publish the read.
    fn reload_filter_kpi_inner(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        user_edit: bool,
        cx: &mut Context<Self>,
    ) {
        if user_edit {
            self.report_busy_retries.reset();
            self.tuner.mark_dialog_draft_changed();
        }
        let retired = self
            .latest_reads
            .cancel(&[super::super::bg::ReadLane::FilterKpi]);
        let restart_histogram = retired.contains(&super::super::bg::ReadLane::FilterHistogram);
        self.tuner.read_after_report = after_report;
        self.tuner.seq = self.tuner.seq.wrapping_add(1);
        let req = self.tuner.seq;
        let report_req = self.current_report_generation();
        let query = self.tuner_query();
        let variants = self.tuner.variants(self.scope_is_drops_only());
        let anchor = self
            .sel_strategy
            .as_ref()
            .and_then(|(key, _)| super::parse_strat_key(key));
        let sid = anchor.map(|(sid, _)| sid);
        let core = anchor.and_then(|(_, core)| core);
        let defaults = self.filter_defaults(cx);
        self.tuner.stats.begin();
        self.spawn_latest_db(
            &[super::super::bg::ReadLane::FilterKpi],
            show_overlay,
            cx,
            move || {
                let stats = moon_core::db::tuner::variant_stats(&query, &variants);
                let filters = sid
                    .map(|sid| moon_core::db::tuner::strategy_filters(sid, core, &defaults))
                    .unwrap_or_default();
                (stats, filters)
            },
            move |this, (stats, filters), cx| {
                if this.tuner.seq != req {
                    return;
                }
                let stats_outcome = CatchUpOutcome::of_read(&stats);
                let error = stats.as_ref().err().cloned();
                let keep_stats = this.keep_on_catch_up(after_report, stats_outcome, report_req);
                this.tuner.sample_snapshot.observe(stats.as_ref().ok());
                this.tuner.stats.apply_or_keep(stats, keep_stats);
                this.tuner.apply_strategy_read(filters, after_report);
                this.tuner.dirty = super::super::refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    error.is_some(),
                );
                if after_report {
                    this.settle_report_refresh_retry(stats_outcome.is_transient(), cx);
                }
                cx.notify();
            },
        );
        if restart_histogram {
            self.reload_hist_inner(false, cx);
        }
    }

    /// Recompute the selected field's histogram after a user-controlled change.
    ///
    /// Args:
    ///     cx: GPUI context used to run and publish the background read.
    pub(in crate::analytics) fn reload_hist(&mut self, cx: &mut Context<Self>) {
        self.reload_hist_inner(true, cx);
    }

    /// Start a user-requested histogram query.
    ///
    /// Args:
    ///     show_overlay: Whether the user-requested read needs blocking progress feedback.
    ///     cx: GPUI context used to execute and publish the histogram read.
    fn reload_hist_inner(&mut self, show_overlay: bool, cx: &mut Context<Self>) {
        let retired = self
            .latest_reads
            .cancel(&[super::super::bg::ReadLane::FilterHistogram]);
        let restart_kpi = retired.contains(&super::super::bg::ReadLane::FilterKpi);
        self.tuner.hist_seq = self.tuner.hist_seq.wrapping_add(1);
        let req = self.tuner.hist_seq;
        let report_req = self.current_report_generation();
        let q = self.tuner_query();
        let field = FIELDS[self.tuner.sel_field].col.to_string();
        self.tuner.hist_dirty = false;
        self.tuner.hist_loading = true;
        self.tuner.hist.begin();
        self.spawn_latest_db(
            &[super::super::bg::ReadLane::FilterHistogram],
            show_overlay,
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
        if restart_kpi {
            self.restart_filter_kpi_after_compound_cancel(cx);
        }
    }

    /// Restart the KPI half when a histogram-only change cancels their compound SQLite scan.
    ///
    /// Args:
    ///     cx: GPUI context used to execute and publish the replacement KPI read.
    fn restart_filter_kpi_after_compound_cancel(&mut self, cx: &mut Context<Self>) {
        let after_report = self.tuner.read_after_report;
        self.reload_filter_kpi_inner(after_report, false, false, cx);
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

    /// Keep search/results fixed above the scrollable field ranges and histogram selectors.
    pub(in crate::analytics::tuner) fn fields_grid(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = super::bounds::GridLayout::new(
            super::layout::side_width(self.tuner_host_width, cx),
            false,
            cx,
        );
        let mut grid = v_flex().w_full();
        for variants in layout.sections() {
            grid = grid.child(self.filter_table(layout, &variants, p, window, cx));
        }
        // The suggestion row and the toolbar come from the SHARED shell (tuner_shell), axis
        // 'By filter': one code path for every tuner, actions dispatched by axis.
        let cfg_row = self.shell_config_row(TunerKind::Filter, p, window, cx);
        let header = self.shell_toolbar(
            TunerKind::Filter,
            t!("analytics.tuner.fields_title").to_string(),
            cx,
        );
        let fixed = v_flex()
            .w_full()
            .child(header)
            .child(cfg_row)
            .children(self.split_summary(p, cx))
            .into_any_element();
        super::layout::parameters(fixed, grid.into_any_element(), p, window, cx)
    }

    /// Render one variant section with the same slots in its header and field rows.
    fn filter_table(
        &mut self,
        layout: super::bounds::GridLayout,
        variants: &[usize],
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::bounds::{VariantHead, header};
        let drops_only = self.scope_is_drops_only();
        let check = div()
            .flex_none()
            .child(
                MoonCheckbox::new("tun-en-all")
                    .checked(self.tuner.all_mapped_enabled(drops_only))
                    .size(design::CONTROL_TIER)
                    .on_change({
                        let view = cx.entity();
                        move |on: &bool, _, app| {
                            let on = *on;
                            view.update(app, |this, cx| this.set_all_fields_enabled(on, cx));
                        }
                    }),
            )
            .into_any_element();
        let name = layout
            .name_cell()
            .id("tun-en-all-lbl")
            .truncate()
            .cursor_pointer()
            .child(t!("analytics.tuner.field").to_string())
            .on_click(cx.listener(|this, _, _, cx| {
                let all_on = this.tuner.all_mapped_enabled(this.scope_is_drops_only());
                this.set_all_fields_enabled(!all_on, cx);
            }))
            .into_any_element();
        let heads = variants
            .iter()
            .map(|&vi| VariantHead {
                index: vi,
                copy: glyph_btn(
                    if vi == 0 { "tun-cp-col" } else { "tun-cpb-col" },
                    if vi == 0 { "→" } else { "←" },
                    t!(if vi == 0 {
                        "analytics.time.tip_to_v2"
                    } else {
                        "analytics.time.tip_to_v1"
                    })
                    .to_string(),
                    p.amber,
                    p,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if vi == 0 {
                        this.copy_v1_to_v2(None, cx);
                    } else {
                        this.copy_v2_to_v1(None, cx);
                    }
                }))
                .into_any_element(),
                clear: glyph_btn(
                    format!("tun-clr-col-{vi}"),
                    "✕",
                    t!("analytics.time.tip_clear_all").to_string(),
                    p.orange,
                    p,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.clear_variant(vi, cx)))
                .into_any_element(),
            })
            .collect();
        let head = header(
            layout,
            format!("filter-grid-head-{}", variants[0]),
            check,
            name,
            heads,
            p,
            cx,
        );
        // The per-field "in strategy" chips and the group ignore=YES/NO labels are the ANCHOR's
        // only; in multi-select each selected strategy differs, so `current_if_single` yields
        // None and we render from an empty card (found=false hides both). Display-only — the
        // write still reads `self.tuner.strat`.
        let strat = self
            .current_if_single(self.tuner.strat.clone())
            .unwrap_or_else(|| Arc::new(StratFilters::default()));
        let mut grid = v_flex()
            .id(("filter-grid-section", variants[0]))
            .w_full()
            .flex_none()
            .child(head);
        let mut last_class: Option<FieldClass> = None;
        for (fi, field) in FIELDS.iter().enumerate() {
            // Skipping the row also skips its header: the header is emitted from this loop,
            // and Detect is its own parent, so a hidden Detect field leaves no empty group.
            if !offer_field(field.class, drops_only) {
                continue;
            }
            let class = field.class;
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
            let mut row = layout
                .row(cx)
                .id(SharedString::from(format!("tun-field-{fi}")))
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .when(sub && !layout.compact, |el| el.pl(design::ui_px(cx, 22.0)))
                .py(design::ui_px(cx, 2.0))
                .items_center()
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.5))
                .child(
                    div().flex_none().child(
                        MoonCheckbox::new(SharedString::from(format!("tun-en-{fi}")))
                            .checked(self.tuner.enabled[fi])
                            .size(design::CONTROL_TIER)
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
                    layout
                        .name_cell()
                        .font_family(design::mono())
                        .id(SharedString::from(format!("tun-name-{fi}")))
                        .tooltip(crate::panels::common::text_tooltip(
                            FIELDS[fi].label.to_string(),
                        ))
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
            let show_current = chip.is_some() || unmapped;
            let current = match chip {
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
                        .text_size(design::t_body(cx))
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
                    .font_family(design::ui_font())
                    .text_size(design::t_body(cx))
                    .text_color(moon_alpha(p.text_muted, 0.7))
                    .child(t!("analytics.tuner.no_param").to_string())
                    .into_any_element(),
                None => div().flex_1().into_any_element(),
            };
            let compact_current = if layout.compact {
                show_current.then_some(current)
            } else {
                row = row.child(current);
                None
            };
            for &vi in variants {
                for is_to in [false, true] {
                    // Spacer under the header's copy arrow (it sits between "from" and "to"), so
                    // the row columns stay aligned with the header.
                    if is_to {
                        row = row.child(div().w(design::ui_px(cx, 12.0)).flex_none());
                    }
                    let input = self.bound_input(vi, fi, is_to, window, cx);
                    row = row.child(
                        div()
                            .w(design::font_w_px(cx, layout.input))
                            .flex_none()
                            .child(
                                MoonInput::new(SharedString::from(format!(
                                    "tun-in-{vi}-{fi}-{is_to}"
                                )))
                                .state(&input)
                                .size(design::INPUT_SIZE),
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
            if let Some(current) = compact_current {
                grid = grid.child(
                    h_flex()
                        .w_full()
                        .px(design::ui_px(cx, 12.0))
                        .pb(design::ui_px(cx, 3.0))
                        .gap(design::ui_px(cx, 6.0))
                        .text_size(design::t_caption(cx))
                        .child(
                            div()
                                .flex_none()
                                .font_family(design::ui_font())
                                .text_color(moon(p.text_muted))
                                .child(t!("analytics.time.cur").to_string()),
                        )
                        .child(current),
                );
            }
        }
        grid.into_any_element()
    }

    /// What the last suggestion's ranges achieved, split in two by whether the search was allowed
    /// to see the trades being measured.
    ///
    /// Shown directly below the result band and above the grid: fitted profit and independent
    /// holdout must be visible together. Nothing is drawn until a
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
        let stale = self.tuner.sample_snapshot.materially_changed(
            self.tuner.stats.data().map(|s| s.as_ref()),
            self.tuner_query().to,
        );
        if !split.should_show_summary(stale) {
            return None;
        }
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
                    // The path above is the best supported answer either way; this says WHETHER
                    // anything supported it. Without it an undecided gate — the ordinary outcome,
                    // since a pairwise win must repeat on both reserved folds — reaches the user
                    // wearing the same confident wording as a verdict that was actually won.
                    let decision = if set.gate_robust {
                        decision.to_string()
                    } else {
                        format!(
                            "{decision} — {}",
                            t!("analytics.tuner.compose_inconclusive")
                        )
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
                                    .font_family(design::ui_font())
                                    .text_color(moon(p.text_muted))
                                    .child(t!("analytics.tuner.compose_result").to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(design::ui_font())
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
                    let el = if fields.is_empty() && set.dropped_at_refit == 0 {
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
                                        .font_family(design::ui_font())
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
                    };
                    // "No additional filters" is the ordinary verdict and reads as the search
                    // having found nothing. It usually DID find a set — one that earned its keep
                    // on the folds it was fitted on and lost the money back on the two nobody
                    // fitted it to. Naming that set and both figures is the difference between a
                    // verdict the user can act on and one they can only distrust. Soft, not
                    // orange: the orange rows in this block mean something could not be CHECKED,
                    // and this one is an explanation of a check that ran and came out negative.
                    el.when(
                        matches!(set.decision, ComposeDecision::NoAdditionalFilters),
                        |el| {
                            el.when_some(set.rejected.as_ref(), |el, rejected| {
                                let names: Vec<&str> = rejected
                                    .fields
                                    .iter()
                                    .map(|col| {
                                        FIELDS
                                            .iter()
                                            .find(|s| s.col == *col)
                                            .map_or(*col, |s| s.label)
                                    })
                                    .collect();
                                el.child(
                                    h_flex()
                                        .w_full()
                                        .gap(design::ui_px(cx, 6.0))
                                        .child(
                                            div()
                                                .w(design::font_w_px(cx, 74.0))
                                                .flex_none()
                                                .truncate()
                                                .font_family(design::ui_font())
                                                .text_color(moon(p.text_muted))
                                                .child(
                                                    t!("analytics.tuner.compose_rejected")
                                                        .to_string(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(moon(p.text_soft))
                                                .child(
                                                    t!(
                                                        "analytics.tuner.compose_rejected_detail",
                                                        fields = names.join(" · "),
                                                        inner = super::super::summary::fmt_signed(
                                                            rejected.inner_lift
                                                        ),
                                                        gate = super::super::summary::fmt_signed(
                                                            rejected.gate_lift
                                                        )
                                                    )
                                                    .to_string(),
                                                ),
                                        ),
                                )
                            })
                        },
                    )
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
                                    .font_family(design::ui_font())
                                    .text_color(moon(p.text_muted))
                                    .child(t!("analytics.tuner.compose_result").to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(design::ui_font())
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
                            .font_family(design::ui_font())
                            .text_color(moon(p.orange))
                            .child(text.to_string()),
                    )
                })
                .child(super::split::line(
                    t!("analytics.tuner.split_train").to_string(),
                    &split.train,
                    p.text_soft,
                    p,
                    cx,
                ))
                // A losing holdout is the whole point of the exercise: the ranges fitted the past
                // and did not survive contact with the rest of it. It is coloured as the failure
                // it is rather than left to be read off a sign.
                .when_some(holdout, |el, holdout| {
                    el.child(super::split::line(
                        t!("analytics.tuner.split_holdout").to_string(),
                        holdout,
                        super::split::holdout_color(
                            holdout.profit,
                            split.fact_holdout.as_ref().map(|fact| fact.profit),
                            p,
                        ),
                        p,
                        cx,
                    ))
                })
                .when_some(split.fact_holdout.as_ref(), |el, fact| {
                    el.child(super::split::fact_line(fact, p, cx))
                })
                .when(
                    holdout
                        .zip(split.fact_holdout.as_ref())
                        .is_some_and(|(fit, fact)| fit.profit < fact.profit),
                    |el| {
                        el.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .font_family(design::ui_font())
                                .text_color(moon(p.orange))
                                .child(t!("analytics.tuner.split_below_fact").to_string()),
                        )
                    },
                )
                .when(stale, |el| {
                    el.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .font_family(design::ui_font())
                            .text_color(moon(p.orange))
                            .child(t!("analytics.coins.stale_note_report").to_string()),
                    )
                })
                // A composed set with nothing held back is a set chosen with no way to check it.
                // The warning belongs beside the set, where it is read, not only in the popover
                // where the share was chosen.
                .when(holdout.is_none() && split.composed.is_some(), |el| {
                    el.child(
                        div()
                            .w_full()
                            .truncate()
                            .font_family(design::ui_font())
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
    ///
    /// Green means MoonBot applies the class ([`ignore_badge`]): a parent switch that turns it
    /// off greys the badge and is named after it, while the click still toggles the class's own
    /// switch only.
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
            FieldClass::Detect => t!("analytics.tuner.grp_detect"),
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
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_color(moon(p.text_soft))
                    .child(label),
            );
        if strat.found
            && let Some(badge) = ignore_badge(class, strat, &self.tuner.staged_ignore)
            && !(sub && class != FieldClass::BvSv)
        {
            let flag = badge.flag;
            // The colour is what MoonBot will do (#1068); the text is the class's own switch,
            // which is what a click stages. A parent that turns the class off is named beside it
            // and is never cleared from here.
            let off = moon_alpha(p.text_muted, 0.7);
            hdr = hdr.child(
                div()
                    .id(SharedString::from(format!("tun-ign-{flag}")))
                    .cursor_pointer()
                    .text_color(if badge.live() { moon(p.green) } else { off })
                    .child(if badge.own_ignored {
                        "ignore=YES"
                    } else {
                        "ignore=NO"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let cur =
                            flag_of(class, &this.tuner.strat.clone()).is_some_and(|(_, cur)| cur);
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
            // Read-only, in the style of the time grid's observed `IgnoreFilters`: the flag name
            // is MoonBot's own identifier, so it is not translated.
            if let Some(parent) = badge.overridden_by {
                hdr = hdr.child(div().text_color(off).child(format!("{parent}=YES")));
            }
        }
        hdr.into_any_element()
    }
}

/// Whether a strategy kind is a Drops detect.
///
/// The schema column stores `Drops` and a MoonBot export stores `DropsDetection`. Both name
/// the same kind. Anything else, including an empty string, is not a Drops strategy.
///
/// Args:
///     kind: `SignalType` text from the loaded strategy row.
fn is_drops_kind(kind: &str) -> bool {
    let kind = kind.trim();
    kind.eq_ignore_ascii_case("DropsDetection") || kind.eq_ignore_ascii_case("Drops")
}

/// Whether every kind in the iterator is a Drops detect.
///
/// An empty iterator is not Drops-only. "All of nothing" would show xPriceDelta on a scope
/// that has no strategy to save it onto.
///
/// Args:
///     kinds: Strategy kinds in the scope being judged.
///
/// Returns:
///     `true` only when the iterator yields at least one kind and each one is Drops.
fn drops_only_kinds<I, S>(kinds: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut any = false;
    for kind in kinds {
        any = true;
        if !is_drops_kind(kind.as_ref()) {
            return false;
        }
    }
    any
}

/// Whether the loaded strategy rows, narrowed by the current selection, are all Drops detects.
///
/// An empty selection means the whole loaded list, which is what an empty `q.strategies` means
/// for the tuner query. A selected key the list does not contain fails closed: the page cannot
/// prove that target is Drops.
///
/// Args:
///     strategies: Loaded strategy rows, unfiltered by the list's kind dropdown.
///     selected: Read-scope keys as `(strategy id, core)`. Empty means every loaded row.
///
/// Returns:
///     `true` only when that scope is non-empty and every kind in it is Drops.
fn drops_only_scope(strategies: &[GroupStat], selected: &[(i64, Option<u64>)]) -> bool {
    if strategies.is_empty() {
        return false;
    }
    if selected.is_empty() {
        return drops_only_kinds(strategies.iter().map(|row| row.kind.as_str()));
    }
    let mut kinds = Vec::with_capacity(selected.len());
    for &(sid, core) in selected {
        let Some(kind) = kind_of(strategies, sid, core) else {
            return false;
        };
        kinds.push(kind);
    }
    drops_only_kinds(kinds)
}

/// Kind text of one `strategyid@core` row, or the bare id when the key has no core.
fn kind_of(strategies: &[GroupStat], sid: i64, core: Option<u64>) -> Option<&str> {
    let key = match core {
        Some(core) => format!("{sid}@{core}"),
        None => sid.to_string(),
    };
    strategies
        .iter()
        .find(|row| row.key == key)
        .map(|row| row.kind.as_str())
}

/// Whether the grid offers this field on the current scope.
///
/// Detect is the only class the scope can hide. Every other class stays, including on a
/// mixed or non-Drops selection.
///
/// Args:
///     class: Field class from `FIELDS`.
///     drops_only: Whether [`drops_only_scope`] accepted the current scope.
fn offer_field(class: FieldClass, drops_only: bool) -> bool {
    class != FieldClass::Detect || drops_only
}

/// Next checkbox bit when the master switch is clicked.
///
/// Turning the visible set on does not tick a field the grid is not showing. Turning
/// everything off clears that hidden field too, so returning to a Drops scope does not
/// revive a search the user cleared.
///
/// Args:
///     currently: The bit stored before the click.
///     on: Whether the master click is switching the set on.
///     mapped: Whether the field has a strategy parameter.
///     offered: Whether the grid is showing the field.
///
/// Returns:
///     The bit to store.
fn master_bit(currently: bool, on: bool, mapped: bool, offered: bool) -> bool {
    if on && !offered {
        currently
    } else {
        on && mapped && offered
    }
}

/// Whether `key` is a strategy parameter of a Detect field.
///
/// Matched through `FIELDS`, so a second Detect parameter is covered without a hard-coded name.
fn is_detect_param(key: &str) -> bool {
    FIELDS.iter().any(|spec| {
        spec.class == FieldClass::Detect && (spec.p_min == Some(key) || spec.p_max == Some(key))
    })
}

/// Change list to write onto a strategy of `kind`.
///
/// A Drops strategy keeps every parameter. Any other kind, including an unknown one, loses
/// Detect parameters. The caller already excludes retained Detect bounds on a mixed read scope;
/// this target check also protects stale or unknown write targets. The rest is unchanged.
///
/// Args:
///     changes: Shared parameter edits, including a Comment stamp when one was added.
///     kind: The target's `SignalType`, or `None` when the loaded list has no row for it.
///
/// Returns:
///     The edits that may be written onto that target.
fn changes_for_kind(changes: &[(String, String)], kind: Option<&str>) -> Vec<(String, String)> {
    if kind.is_some_and(is_drops_kind) {
        return changes.to_vec();
    }
    changes
        .iter()
        .filter(|(key, _)| !is_detect_param(key))
        .cloned()
        .collect()
}

/// Per-target change lists, with Detect parameters removed from every non-Drops target.
///
/// When every target receives the same list, that list is the shared payload and the second
/// value is `None`. A mixed selection has no shared list: the confirmation dialog draws the
/// shared list as written to every target, and draws one group per target whenever individual
/// payloads are present. Returning the anchor's list there would show `DropsPriceDelta` on a MoonShot
/// target the write does not touch.
///
/// Args:
///     changes: Shared edits before the kind filter.
///     kinds: One kind per target, aligned with the target list. `None` is not Drops.
///
/// Returns:
///     The shared payload (empty when the targets differ), and per-target lists when they differ.
fn changes_by_target(
    changes: &[(String, String)],
    kinds: &[Option<&str>],
) -> (Vec<(String, String)>, Option<Vec<Vec<(String, String)>>>) {
    let tailored: Vec<Vec<(String, String)>> = kinds
        .iter()
        .map(|kind| changes_for_kind(changes, *kind))
        .collect();
    let uniform = tailored.windows(2).all(|pair| pair[0] == pair[1]);
    if uniform {
        let preview = tailored.first().cloned().unwrap_or_default();
        (preview, None)
    } else {
        (Vec::new(), Some(tailored))
    }
}

/// Whether a tailored save still has a parameter to write.
///
/// A mixed selection can leave the anchor's preview empty while another target still has
/// edits. That is a real write. An empty shared list, or a per-target set whose every list
/// is empty, is not.
fn changes_remain(
    preview: &[(String, String)],
    per_target: Option<&[Vec<(String, String)>]>,
) -> bool {
    match per_target {
        Some(all) => all.iter().any(|row| !row.is_empty()),
        None => !preview.is_empty(),
    }
}

#[cfg(test)]
mod tests;
