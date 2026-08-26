//! Grid of the "By time" mode: THREE field rows, each one a single "from-to".
//!   - "Weekly" (`WorkingWeekTime`): a continuous span over the MINUTE OF THE WEEK, step 1 min,
//!     input/output `day.hh:mm` (day 1..7); e.g. `1.23:44-6.22:22`.
//!   - "Day" (`WorkingTime`, time-of-day mode): window `hh:mm-hh:mm`.
//!   - "In hour" (`WorkingTime`, minute-of-hour mode): window `N-M` (0..59).
//! "Day" and "In hour" are TWO views of the SAME WorkingTime field → their VALUES are
//! mutually exclusive: filling in / auto-suggesting one clears the other.
//! The row CHECKBOXES are independent of that — each only says "consider this field in the
//! sweep". Ticking both WorkingTime rows offers the sweep both formats as competing
//! candidates for the one field, and it keeps whichever earns more; so a full sweep yields
//! at most TWO windows — a week span plus one WorkingTime window.
//!
//! The current value of the strategy fields is simply DISPLAYED as a raw string (parsing
//! is unreliable) — in amber as "in strategy". v1/v2 are fresh input / auto-suggestion; Save writes
//! the non-empty changed fields. The shell (toolbar + suggestion row) is SHARED with the filter.

use gpui::*;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonInput, MoonInputEvent, MoonInputState, MoonPalette,
    MoonTooltipView, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::{TunerKind, glyph_btn};
use super::state::{N_FIELD, fmt_min, fmt_week_ep};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::TimeWindow;

/// Width of the row-checkbox column, in `design::ui_px` units. The header spacer and the
/// slider lead-in reuse it so the field names stay in one line down the card; it is sized
/// above the compact checkbox's own box, which moonui draws at a fixed size.
pub(in crate::analytics::tuner) const CHECK_COL: f32 = 16.0;
/// Field-name column: fits "WorkingWeekTime" on one line next to the checkbox.
pub(in crate::analytics::tuner) const NAME_COL: f32 = 110.0;

impl AnalyticsView {
    /// Commit a field bound (Blur/Enter): state → WT mutual exclusion → KPIs.
    fn commit_time(
        &mut self,
        vi: usize,
        field: usize,
        is_to: bool,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let slot = &mut self.time_tuner.bounds[vi][field];
        let cur = if is_to { &mut slot.1 } else { &mut slot.0 };
        if *cur == value {
            return;
        }
        *cur = value;
        if vi == 0 {
            self.time_tuner.invalidate_suggest();
        }
        // "Day" and "In hour" are one WorkingTime field: filling one clears the other.
        if field == 1 {
            self.clear_field(vi, 2);
        } else if field == 2 {
            self.clear_field(vi, 1);
        }
        self.reload_time(cx);
        cx.notify();
    }

    /// Clear both bounds of field `field` in variant `vi` (no recompute — internal helper).
    pub(in crate::analytics::tuner) fn clear_field(&mut self, vi: usize, field: usize) {
        self.time_tuner.bounds[vi][field] = (String::new(), String::new());
        if vi == 0 {
            self.time_tuner.invalidate_suggest();
        }
        self.time_tuner.inputs.remove(&format!("tt{vi}f{field}a"));
        self.time_tuner.inputs.remove(&format!("tt{vi}f{field}b"));
    }

    /// Clear field `field` of variant `vi` (the cell's ✕) + recompute.
    fn time_clear_cell(&mut self, vi: usize, field: usize, cx: &mut Context<Self>) {
        self.clear_field(vi, field);
        self.reload_time(cx);
        cx.notify();
    }

    /// Copy the WHOLE v2 column → v1 (the ← in the header) — all fields. Mirror of `time_copy_all`.
    fn time_copy_all_back(&mut self, cx: &mut Context<Self>) {
        for field in 0..N_FIELD {
            let v = self.time_tuner.bounds[1][field].clone();
            self.set_v1_cell(field, v.0, v.1);
        }
        self.reload_time(cx);
        cx.notify();
    }

    /// Auto-suggestion into v1 (background): the CHECKED rows only. With both WorkingTime
    /// formats ticked the sweep compares them and answers in the better one; an unchecked
    /// row keeps its value and, for a field nothing may search, pins the sweep to it. Scope
    /// is `tuner_query`. No improvement → the searched rows are cleared.
    pub(in crate::analytics::tuner) fn time_suggest(&mut self, cx: &mut Context<Self>) {
        if self.time_tuner.sugg_busy {
            return;
        }
        let axes = self.time_tuner.axes();
        // Nothing checked: spinning the button to write nothing reads as "found nothing".
        if axes.is_empty() {
            log::info!("analytics: time sweep skipped — no row is checked");
            return;
        }
        self.time_tuner.sugg_seq = self.time_tuner.sugg_seq.wrapping_add(1);
        let req = self.time_tuner.sugg_seq;
        self.time_tuner.sugg_busy = true;
        let q = self.tuner_query();
        let min_n = self
            .time_tuner
            .min_trades
            .trim()
            .parse::<i64>()
            .unwrap_or(5)
            .max(1);
        let edges = 512usize;
        // Time: exact minutes. round_bound (3-significant-digit rounding) on minutes of the
        // day/week would cross day/hour boundaries (e.g. 23:59→24:00) — so we do NOT round.
        let round = false;
        self.spawn_db(
            true,
            cx,
            move || moon_core::db::tuner::suggest_time(&q, min_n, edges, round, axes),
            move |this, sugg, cx| {
                if this.time_tuner.sugg_seq != req {
                    return; // stale auto-suggestion (the strategy/scope changed)
                }
                this.time_tuner.sugg_busy = false;
                match sugg {
                    Ok(s) => {
                        // Write ONLY into the FIELDS that were searched. WorkingWeekTime
                        // is its own field: unchecked → untouched, its value pinned the
                        // sweep. WorkingTime is one field behind two rows, so if either
                        // format was searched its answer replaces the pair.
                        if axes.week {
                            // Field 0: the week window as "day.hh:mm".
                            let (w0, w1) = s
                                .week_span
                                .map(|(f, t)| (fmt_week_ep(f, false), fmt_week_ep(t, true)))
                                .unwrap_or_default();
                            this.set_v1_cell(0, w0, w1);
                        }
                        // WorkingTime: the sweep picked the format among the ticked ones,
                        // so its answer names the row — fill it and clear the other view
                        // of the same field. Both unticked → the field is left alone.
                        if axes.day || axes.hour {
                            match s.tod {
                                Some(TimeWindow::Day(f, t)) => {
                                    this.set_v1_cell(1, fmt_min(f), fmt_min(t));
                                    this.clear_field(0, 2);
                                }
                                Some(TimeWindow::Hour(f, t)) => {
                                    this.set_v1_cell(2, f.to_string(), t.to_string());
                                    this.clear_field(0, 1);
                                }
                                // Nothing beat the baseline: clear only the formats the
                                // sweep was allowed to search. An unchecked row keeps
                                // the value the user put there — the sweep never judged
                                // it, so it is not ours to delete.
                                None => {
                                    if axes.day {
                                        this.clear_field(0, 1);
                                    }
                                    if axes.hour {
                                        this.clear_field(0, 2);
                                    }
                                }
                            }
                        }
                        this.reload_time(cx);
                    }
                    Err(e) => log::warn!("analytics: time auto-suggestion failed — {e}"),
                }
                cx.notify();
            },
        );
    }

    /// Write `(from, to)` into v1 of field `field` and drop its cached inputs.
    pub(in crate::analytics::tuner) fn set_v1_cell(
        &mut self,
        field: usize,
        from: String,
        to: String,
    ) {
        self.time_tuner.bounds[0][field] = (from, to);
        self.time_tuner.invalidate_suggest();
        self.time_tuner.inputs.remove(&format!("tt0f{field}a"));
        self.time_tuner.inputs.remove(&format!("tt0f{field}b"));
    }

    /// Clear the WHOLE variant column (the ✕ in the header). In v1 — the column the sweep
    /// owns — an unchecked row holds a value the user pinned on purpose, so ✕ leaves it
    /// alone (mirrors the filter tuner's `clear_variant`). v2 is a what-if column nothing
    /// sweeps, so its ✕ clears everything; gating it on a sweep flag would leave rows that
    /// cannot be cleared at all.
    fn time_clear_variant(&mut self, vi: usize, cx: &mut Context<Self>) {
        for field in 0..N_FIELD {
            if vi == 0 && !self.time_tuner.enabled[field] {
                continue;
            }
            self.clear_field(vi, field);
        }
        self.reload_time(cx);
        cx.notify();
    }

    /// Row checkbox: "consider this field in the sweep" — the filter tuner's rule, and the
    /// three rows are independent. Ticking both WorkingTime formats is meaningful: they
    /// become competing candidates for that one field. No value moves, so the KPIs need no
    /// recompute; only an in-flight sweep, started under the old set, becomes stale.
    fn toggle_time_field(&mut self, field: usize, on: bool, cx: &mut Context<Self>) {
        self.time_tuner.enabled[field] = on;
        self.time_tuner.invalidate_suggest();
        cx.notify();
    }

    /// Copy the WHOLE v1 column → v2 (the → in the header) — all fields.
    fn time_copy_all(&mut self, cx: &mut Context<Self>) {
        for field in 0..N_FIELD {
            self.time_tuner.bounds[1][field] = self.time_tuner.bounds[0][field].clone();
            self.time_tuner.inputs.remove(&format!("tt1f{field}a"));
            self.time_tuner.inputs.remove(&format!("tt1f{field}b"));
        }
        self.reload_time(cx);
        cx.notify();
    }

    /// `IgnoreTime` switch (YES/NO). A click toggles the manual stage; if we land back on the
    /// current value the stage is dropped (automatic logic). Affects Save, not the KPIs.
    ///
    /// Args:
    ///     cx: GPUI context used to retire a pending Save preview and repaint the switch.
    fn toggle_ignore_time(&mut self, cx: &mut Context<Self>) {
        let cur = self.time_tuner.ignore_cur;
        let next = !self.time_tuner.ignore_effective();
        self.time_tuner.ignore_staged = (next != cur).then_some(next);
        self.tuner.mark_dialog_draft_changed();
        cx.notify();
    }

    /// Ignore-flags row: the `IgnoreTime` switch + the observed GLOBAL `IgnoreFilters`
    /// (with a schedule set, YES → it will be cleared to NO). For the schedule to work,
    /// both must be NO.
    fn time_ignore_row(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let ig_time = self.time_tuner.ignore_effective();
        let ig_filters = self.time_tuner.ign_filters_cur;
        // Styled like the "By filter" subheadings: table_head background, green = active
        // (ignore=NO), grey = ignored (ignore=YES). Clicking IgnoreTime toggles it.
        let col = |ignored: bool| {
            if ignored {
                moon_alpha(p.text_muted, 0.7)
            } else {
                moon(p.green)
            }
        };
        let mut row = h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 3.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .bg(moon_alpha(p.table_head, 0.6))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.7))
            .text_size(design::t_caption(cx))
            .child(div().text_color(moon(p.text_soft)).child("IgnoreTime"))
            .child(
                div()
                    .id("tun-ign-IgnoreTime")
                    .cursor_pointer()
                    .text_color(col(ig_time))
                    .child(if ig_time { "ignore=YES" } else { "ignore=NO" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_ignore_time(cx))),
            );
        // The global IgnoreFilters gates time as well; shown read-only (with a schedule YES → NO).
        if ig_filters {
            row = row
                .child(div().text_color(moon(p.text_soft)).child("IgnoreFilters"))
                .child(div().text_color(col(true)).child("ignore=YES"));
        }
        row.into_any_element()
    }

    /// One-line caption above the schedule grid: the hour windows below are computed and applied
    /// on the CORE's own clock, which the user decided must never be silently ambiguous — the
    /// hours it shows may not match the clock on the user's screen. Reuses the compact-caption
    /// chrome `time_ignore_row` already established rather than inventing a second one.
    ///
    /// Args:
    ///     p: Active Moon palette used for the caption colour.
    ///     cx: GPUI context used to resolve caption spacing and typography.
    ///
    /// Returns:
    ///     The schedule-grid caption element.
    fn core_clock_note(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        div()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 3.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .child(t!("analytics.tuner.time.core_clock_note").to_string())
            .into_any_element()
    }

    /// Field-bound input with a lazy cache (the tuner's `bound_input` pattern).
    fn time_input(
        &mut self,
        vi: usize,
        field: usize,
        is_to: bool,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!("tt{vi}f{field}{}", if is_to { "b" } else { "a" });
        if let Some(state) = self.time_tuner.inputs.get(&id) {
            return state.clone();
        }
        let slot = &self.time_tuner.bounds[vi][field];
        let value = if is_to {
            slot.1.clone()
        } else {
            slot.0.clone()
        };
        let ph = placeholder.to_string();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(value)
                .placeholder(ph)
        });
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                let value = state.read(cx).value().to_string();
                this.commit_time(vi, field, is_to, value, cx);
            }
        })
        .detach();
        self.time_tuner.inputs.insert(id, state.clone());
        state
    }

    /// The bottom-right "By time" card: the SHARED shell (toolbar + suggestion row)
    /// + 3 field rows. Header: Field | in strategy | v1 from|to|✕ | v2 from|→|to|✕.
    pub(in crate::analytics::tuner) fn time_grid(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_w = 60.0; // fits the weekly field's "d.hh:mm" without leaving slack
        let header = self.shell_toolbar(
            TunerKind::Time,
            t!("analytics.time.autopick_title").to_string(),
            p,
            cx,
        );
        let cfg_row = self.shell_config_row(TunerKind::Time, p, window, cx);
        let ignore_row = self.time_ignore_row(p, cx);
        let clock_note = self.core_clock_note(p, cx);

        let head_cell = |label: String| {
            div()
                .w(design::font_w_px(cx, in_w))
                .flex_none()
                .text_center()
                .child(label)
        };
        let head = h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .items_center()
            // Must match the row gap below, or the columns drift apart.
            .gap(design::ui_px(cx, 4.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            // Master checkbox over the rows' column — the filter tuner's "all on/off".
            .child(
                div().w(design::ui_px(cx, CHECK_COL)).flex_none().child(
                    MoonCheckbox::new("tt-en-all")
                        .checked(self.time_tuner.enabled.iter().all(|&e| e))
                        .size(MoonCheckboxSize::Compact)
                        .on_change({
                            let view = cx.entity();
                            move |ch: &bool, _w, app| {
                                let on = *ch;
                                view.update(app, |this, cx| {
                                    this.time_tuner.enabled = [on; N_FIELD];
                                    // Values do not move → no KPI recompute; only an
                                    // in-flight sweep is now stale.
                                    this.time_tuner.invalidate_suggest();
                                    cx.notify();
                                });
                            }
                        }),
                ),
            )
            .child(
                // Matches the row's field-name column width (fits "WorkingWeekTime").
                div()
                    .w(design::font_w_px(cx, NAME_COL))
                    .flex_none()
                    .child(t!("analytics.time.col_field").to_string()),
            )
            .child(div().flex_1().child(t!("analytics.time.cur").to_string()))
            .child(head_cell(format!("v1 {}", t!("analytics.tuner.from"))))
            // The ONLY two copy buttons, both "copy the whole column": → inside the v1 pair
            // carries v1→v2, ← inside the v2 pair carries v2→v1. Rows have no copy buttons —
            // they keep a matching 12px spacer so the columns stay aligned.
            .child(
                glyph_btn(
                    "tt-cp-col",
                    "→",
                    t!("analytics.time.tip_to_v2").to_string(),
                    p.amber,
                    p,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.time_copy_all(cx))),
            )
            .child(head_cell(t!("analytics.tuner.to").to_string()))
            .child(
                glyph_btn(
                    "tt-clr-col-0",
                    "✕",
                    t!("analytics.time.tip_clear_all").to_string(),
                    p.orange,
                    p,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.time_clear_variant(0, cx))),
            )
            .child(head_cell(format!("v2 {}", t!("analytics.tuner.from"))))
            .child(
                glyph_btn(
                    "tt-cpb-col",
                    "←",
                    t!("analytics.time.tip_to_v1").to_string(),
                    p.amber,
                    p,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.time_copy_all_back(cx))),
            )
            .child(head_cell(t!("analytics.tuner.to").to_string()))
            .child(
                glyph_btn(
                    "tt-clr-col-1",
                    "✕",
                    t!("analytics.time.tip_clear_all").to_string(),
                    p.orange,
                    p,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.time_clear_variant(1, cx))),
            );

        let mut grid = v_flex().w_full().child(head);
        for field in 0..N_FIELD {
            grid = grid.child(self.time_row(field, in_w, p, window, cx));
        }
        // Three heatmap sliders under the rows (Weekly / Day / In hour).
        let sliders = self.time_sliders(p, cx);

        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            // Toolbar/config/ignore stay pinned; the parameter rows + heatmap sliders scroll so a
            // short panel can't clip them off the bottom (they used to just disappear past the edge).
            .child(header)
            .child(cfg_row)
            .child(ignore_row)
            .child(clock_note)
            .child(
                div()
                    .id("tt-params-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(v_flex().w_full().child(grid).child(sliders)),
            )
            .into_any_element()
    }

    /// A single field row (0 Weekly · 1 Day · 2 In hour). Placeholder depends on the field.
    fn time_row(
        &mut self,
        field: usize,
        in_w: f32,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (label_key, ph, cur_idx) = match field {
            0 => ("analytics.time.field_week", "d.hh:mm", 0usize),
            1 => ("analytics.time.field_day", "hh:mm", 1usize),
            _ => ("analytics.time.field_hour", "0..59", 1usize),
        };
        // `current_if_single` centralizes the rule: the "in strategy" value is the anchor's, so
        // show it only for a lone selection; several selected → None → "varies" (each differs).
        let cur = self.current_if_single(self.time_tuner.current_raw[cur_idx].trim().to_string());
        // Dim whichever of the Day/Hour pair is unused (they are one WorkingTime field).
        let dim = matches!(
            (field, self.time_tuner.active_wt()),
            (1, Some(2)) | (2, Some(1))
        );

        let enabled = self.time_tuner.enabled[field];
        // The checkbox lives OUTSIDE the dimmable content: a row is dimmed when the other
        // half of the WorkingTime pair holds the value, but its box still governs whether
        // that format is swept — so it must stay fully legible and clickable.
        let tip = t!("analytics.time.tip_pick").to_string();
        let check = div()
            .id(SharedString::from(format!("tt-en-w-{field}")))
            .flex_none()
            .w(design::ui_px(cx, CHECK_COL))
            .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tip.clone())).into())
            .child(
                MoonCheckbox::new(SharedString::from(format!("tt-en-{field}")))
                    .checked(enabled)
                    .size(MoonCheckboxSize::Compact)
                    .on_change({
                        let view = cx.entity();
                        move |ch: &bool, _w, app| {
                            let on = *ch;
                            view.update(app, |this, cx| this.toggle_time_field(field, on, cx));
                        }
                    }),
            );
        let mut row = h_flex()
            .flex_1()
            .min_w_0()
            .items_center()
            // Must match the header gap, or the columns drift apart.
            .gap(design::ui_px(cx, 4.0))
            .opacity(if dim { 0.45 } else { 1.0 })
            .child(
                // Field name column — wide enough for the full "WorkingWeekTime" on one line;
                // truncates as a safety net.
                div()
                    .w(design::font_w_px(cx, NAME_COL))
                    .flex_none()
                    .truncate()
                    .text_color(moon(p.text))
                    .child(t!(label_key).to_string()),
            )
            // "in strategy" — the field's raw current value (in amber); on multi-select — "varies".
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(match &cur {
                        Some(c) if !c.is_empty() => p.amber,
                        _ => p.text_muted,
                    }))
                    .child(match cur {
                        Some(c) if !c.is_empty() => c,
                        Some(_) => "—".to_string(),
                        None => t!("analytics.time.cur_varies").to_string(),
                    }),
            );
        for vi in [0usize, 1usize] {
            for is_to in [false, true] {
                // Spacer under the header's copy arrow (it sits between "from" and "to"), so the
                // row columns stay aligned with the header.
                if is_to {
                    row = row.child(div().w(design::ui_px(cx, 12.0)).flex_none());
                }
                let input = self.time_input(vi, field, is_to, ph, window, cx);
                // A filled field gets an accent border to stand out from an empty one (which shows
                // only the placeholder). Empty → transparent border (no layout shift).
                let filled = {
                    let slot = &self.time_tuner.bounds[vi][field];
                    let v = if is_to { &slot.1 } else { &slot.0 };
                    !v.trim().is_empty()
                };
                row = row.child(
                    div()
                        .w(design::font_w_px(cx, in_w))
                        .flex_none()
                        .rounded(design::ui_px(cx, 4.0))
                        .border_1()
                        .border_color(if filled {
                            moon(p.accent)
                        } else {
                            moon_alpha(p.border, 0.0)
                        })
                        .child(
                            MoonInput::new(SharedString::from(format!(
                                "tt-in-{vi}-{field}-{is_to}"
                            )))
                            .state(&input)
                            .small(),
                        ),
                );
            }
            row = row.child(
                glyph_btn(
                    SharedString::from(format!("tt-clr-{vi}-{field}")),
                    "✕",
                    t!("analytics.time.tip_clear").to_string(),
                    p.orange,
                    p,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.time_clear_cell(vi, field, cx))),
            );
        }
        // Outer shell carries the row chrome; the checkbox sits before the dimmable content.
        h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 3.0))
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.5))
            .child(check)
            .child(row)
            .into_any_element()
    }
}
