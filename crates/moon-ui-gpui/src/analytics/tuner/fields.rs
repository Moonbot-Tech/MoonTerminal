//! Threshold tuner over the report's market fields (carried over from 'Analytics V3') —
//! the 'Filters' mode of the 'Strategies' tab: a 'Fact vs variants' KPI matrix, a
//! from/to range builder per field and a profit histogram over the quantile buckets of
//! the selected field. The scope is the strategy selected in the list (or all). Bounds retain
//! the raw strings the user typed and start empty on every open because they describe one
//! strategy's search. Only restart count and depth persist.

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
pub(super) use super::state::{
    N_VAR, TunerKind, card, flag_of, fmt_bound, glyph_btn, parse_num, staged_dirty,
};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::{FIELDS, FieldClass, StratFilters};

/// Histogram buckets.
const HIST_BUCKETS: usize = 14;

impl AnalyticsView {
    /// Tuner query: the shared filters plus the scope of EVERY selected strategy.
    ///
    /// The whole selection, not just the clicked row — with Ctrl multi-select the KPI
    /// matrix, the histogram and the sweep have to describe the same set the user sees
    /// highlighted, and the same set Save writes to.
    pub(super) fn tuner_query(&self) -> moon_core::db::analytics::Query {
        let mut q = self.query();
        // Row keys are `strategyid@core_uid`, so the scope is per strategy AND per core.
        q.strategies = self
            .selected_targets()
            .into_iter()
            .map(|t| (t.sid, t.core))
            .collect();
        q
    }

    /// Background recompute of the per-variant KPI matrix (+ the selected strategy's thresholds).
    pub(in crate::analytics) fn reload_tuner(&mut self, cx: &mut Context<Self>) {
        self.tuner.seq = self.tuner.seq.wrapping_add(1);
        let req = self.tuner.seq;
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
        // We do NOT reset the auto-suggestion: editing the bounds does not change the fact
        // distribution it was computed from (the reset lives in invalidate()).
        // Carry current numbers as stale during recomputation; any completed
        // non-data result drops them.
        self.tuner.stats.begin();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let (stats, strat) = executor
                .spawn(async move {
                    let stats = moon_core::db::tuner::variant_stats(&q, &variants);
                    let sf = sid
                        .map(|sid| moon_core::db::tuner::strategy_filters(sid, core, &defaults))
                        .unwrap_or_default();
                    (stats, sf)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.seq != req {
                        return;
                    }
                    // A completed non-data result clears stale numbers because
                    // values under a changed period label must belong to it.
                    this.tuner.stats.apply(stats);
                    this.tuner.strat = Arc::new(strat);
                    this.tuner.dirty = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Background histogram of the selected field.
    pub(in crate::analytics) fn reload_hist(&mut self, cx: &mut Context<Self>) {
        self.tuner.hist_seq = self.tuner.hist_seq.wrapping_add(1);
        let req = self.tuner.hist_seq;
        let q = self.tuner_query();
        let field = FIELDS[self.tuner.sel_field].col.to_string();
        self.tuner.hist.begin();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let hist = executor
                .spawn(async move { moon_core::db::tuner::histogram(&q, &field, HIST_BUCKETS) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.hist_seq != req {
                        return;
                    }
                    this.tuner.hist.apply(hist);
                    cx.notify();
                });
            });
        })
        .detach();
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
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Programmatic set of BOTH bounds of a field (strategy chip / clear /
    /// auto-suggestion): state + a silent resync of the inputs + recompute.
    pub(super) fn apply_bounds(
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
    pub(super) fn fields_grid(
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
                        .checked(
                            FIELDS
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| s.mapped())
                                .all(|(fi, _)| self.tuner.enabled[fi]),
                        )
                        .size(MoonCheckboxSize::Compact)
                        .on_change({
                            let view = cx.entity();
                            move |ch: &bool, _w, app| {
                                let on = *ch;
                                view.update(app, |this, cx| {
                                    for (fi, spec) in FIELDS.iter().enumerate() {
                                        this.tuner.enabled[fi] = on && spec.mapped();
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                ),
            )
            .child(
                div()
                    .w(design::font_w_px(cx, 58.0))
                    .flex_none()
                    .child(t!("analytics.tuner.field").to_string()),
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
                                    view.update(app, |this, cx| {
                                        this.tuner.enabled[fi] = on;
                                        cx.notify();
                                    });
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
                            this.tuner.enabled[fi] = false;
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
            .into_any_element()
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
                        cx.notify();
                    })),
            );
        }
        hdr.into_any_element()
    }
}
