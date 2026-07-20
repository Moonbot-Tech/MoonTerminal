//! Grid of the "By time" mode: THREE field rows, each one a single "from-to".
//!   - "Weekly" (`WorkingWeekTime`): a continuous span over the MINUTE OF THE WEEK, step 1 min,
//!     input/output `day.hh:mm` (day 1..7); e.g. `1.23:44-6.22:22`.
//!   - "Day" (`WorkingTime`, time-of-day mode): window `hh:mm-hh:mm`.
//!   - "In hour" (`WorkingTime`, minute-of-hour mode): window `N-M` (0..59).
//! "Day" and "In hour" are TWO views of the SAME WorkingTime field → MUTUALLY EXCLUSIVE:
//! filling in / auto-suggesting one clears the other (there is no mode switch).
//!
//! The current value of the strategy fields is simply DISPLAYED as a raw string (parsing
//! is unreliable) — in amber as "in strategy". v1/v2 are fresh input / auto-suggestion; Save writes
//! the non-empty changed fields. The shell (toolbar + suggestion row) is SHARED with the filter.

use std::collections::HashMap;

use gpui::*;
use moon_ui::{MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::state::{TunerKind, glyph_btn};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::{TimeWindow, Variant, format_week_span, format_working_time};

/// Variants besides the "Fact" one (v1, v2) — same as the filter tuner's N_VAR.
pub(super) const TIME_N_VAR: usize = 2;
/// Fields: 0 = Weekly (WorkingWeekTime), 1 = Day, 2 = In hour (both are WorkingTime).
const N_FIELD: usize = 3;
/// Minutes in a week (7 days × 1440) — the maximum minute of the week + 1.
pub(super) const WEEK_MIN: u16 = 7 * 1440;

/// State of the "By time" tuner inside `AnalyticsView`.
pub(in crate::analytics) struct TimeTunerState {
    /// `[variant 0..TIME_N_VAR][field 0..N_FIELD] = (from, to)` as strings.
    pub(super) bounds: Vec<[(String, String); N_FIELD]>,
    /// RAW current values of the strategy fields `[WorkingWeekTime, WorkingTime]`.
    pub(super) current_raw: [String; 2],
    /// The strategy's current `IgnoreTime` (the schedule is ignored when YES).
    pub(super) ignore_cur: bool,
    /// Current `IgnoreFilters` (the GLOBAL ignore — it gates time too; must be NO).
    pub(super) ign_filters_cur: bool,
    /// Manual `IgnoreTime` switch (None = follow the automatic logic).
    pub(super) ignore_staged: Option<bool>,
    /// Lazy input cache (created in render, as in the filter tuner).
    pub(super) inputs: HashMap<String, Entity<MoonInputState>>,
    // Shared-shell settings — parallel to `TunerState` (time hides some of them).
    pub(super) iters: String,
    pub(super) min_trades: String,
    pub(super) edges: usize,
    pub(super) round_results: bool,
    pub(super) sugg_busy: bool,
    /// Auto-suggestion generation — a stale result is discarded.
    pub(super) sugg_seq: u64,
}

impl TimeTunerState {
    /// A fresh tuner: all fields empty.
    pub(in crate::analytics) fn load() -> Self {
        Self {
            bounds: vec![std::array::from_fn(|_| (String::new(), String::new())); TIME_N_VAR],
            current_raw: Default::default(),
            ignore_cur: false,
            ign_filters_cur: false,
            ignore_staged: None,
            inputs: HashMap::new(),
            iters: "20".to_string(),
            min_trades: String::new(),
            edges: 64,
            round_results: true,
            sugg_busy: false,
            sugg_seq: 0,
        }
    }

    /// Span of variant `vi` over the minute of the week (field 0). Empty → None; a missing edge →
    /// start (0) / end (WEEK_MIN-1) of the week.
    pub(super) fn week_span(&self, vi: usize) -> Option<(u16, u16)> {
        let (from, to) = &self.bounds[vi][0];
        if from.trim().is_empty() && to.trim().is_empty() {
            return None;
        }
        Some((
            parse_week_ep(from, false).unwrap_or(0),
            parse_week_ep(to, true).unwrap_or(WEEK_MIN - 1),
        ))
    }

    /// WorkingTime window of variant `vi`: from the "Day" row (field 1 → `Day`) OR the
    /// "In hour" row (field 2 → `Hour`). Both filled (shouldn't happen) → "Day" wins.
    pub(super) fn tod(&self, vi: usize) -> Option<TimeWindow> {
        let (df, dt) = &self.bounds[vi][1];
        if !df.trim().is_empty() || !dt.trim().is_empty() {
            let w = (parse_time(df).unwrap_or(0), parse_time(dt).unwrap_or(1439));
            // A full day (00:00–23:59) = no restriction (symmetric with a full week).
            return (w != (0, 1439)).then_some(TimeWindow::Day(w.0, w.1));
        }
        let (hf, ht) = &self.bounds[vi][2];
        if !hf.trim().is_empty() || !ht.trim().is_empty() {
            let w = (parse_moh(hf).unwrap_or(0), parse_moh(ht).unwrap_or(59));
            // The whole hour (0–59) = no restriction.
            return (w != (0, 59)).then_some(TimeWindow::Hour(w.0, w.1));
        }
        None
    }

    /// Which WorkingTime field is set in v1 (used to dim the unused row /
    /// slider): `Some(1)`=Day, `Some(2)`=In hour, `None`=both empty.
    pub(super) fn active_wt(&self) -> Option<usize> {
        let d = &self.bounds[0][1];
        if !d.0.trim().is_empty() || !d.1.trim().is_empty() {
            return Some(1);
        }
        let h = &self.bounds[0][2];
        if !h.0.trim().is_empty() || !h.1.trim().is_empty() {
            return Some(2);
        }
        None
    }

    /// Strategy field string built from variant `vi`: `which` 0 = WorkingWeekTime (week
    /// span; a full week = "no restriction" → ""), 1 = WorkingTime. Empty → "".
    pub(super) fn field_value(&self, vi: usize, which: usize) -> String {
        if which == 0 {
            self.week_span(vi)
                .filter(|&s| s != (0, WEEK_MIN - 1))
                .map(format_week_span)
                .unwrap_or_default()
        } else {
            self.tod(vi).map(format_working_time).unwrap_or_default()
        }
    }

    /// Variants for the KPIs: `[Fact, v1, v2]`. Each carries both week_span AND tod.
    pub(super) fn variants(&self) -> Vec<Variant> {
        let mut out = vec![Variant::default()];
        for vi in 0..self.bounds.len() {
            out.push(Variant {
                // A full week == no restriction (== "Fact").
                week_span: self.week_span(vi).filter(|&s| s != (0, WEEK_MIN - 1)),
                tod: self.tod(vi),
                ..Default::default()
            });
        }
        out
    }

    /// Whether we are changing the schedule: at least one v1 field is non-empty AND differs
    /// from the current value.
    pub(super) fn has_schedule_change(&self) -> bool {
        [0usize, 1].iter().any(|&which| {
            let v = self.field_value(0, which);
            !v.is_empty() && v != self.current_raw[which].trim()
        })
    }

    /// Effective `IgnoreTime` value for display/writing: the manual switch, otherwise
    /// automatic — NO when a schedule is set (else Moonbot ignores the windows), current value
    /// when it is not.
    pub(super) fn ignore_effective(&self) -> bool {
        self.ignore_staged.unwrap_or(if self.has_schedule_change() {
            false
        } else {
            self.ignore_cur
        })
    }

    /// Whether there is anything to write (Save lights up): the schedule, `IgnoreTime` or the
    /// global ignore.
    pub(super) fn is_dirty(&self) -> bool {
        !self.changes().is_empty()
    }

    /// Edits to the strategy: the changed `WorkingWeekTime`/`WorkingTime` plus the ignore flags
    /// that must be cleared for the schedule to take effect (`IgnoreTime`, and also the
    /// GLOBAL `IgnoreFilters` — when writing a schedule, if it is currently YES). We write only
    /// what actually changes.
    pub(super) fn changes(&self) -> Vec<(String, String)> {
        const NAMES: [&str; 2] = ["WorkingWeekTime", "WorkingTime"];
        let yesno = |b: bool| if b { "YES" } else { "NO" }.to_string();
        let mut out: Vec<(String, String)> = (0..2)
            .filter_map(|which| {
                let v = self.field_value(0, which);
                (!v.is_empty() && v != self.current_raw[which].trim())
                    .then(|| (NAMES[which].to_string(), v))
            })
            .collect();
        let schedule = !out.is_empty();
        // IgnoreTime — switch/automatic; written only if it differs from the current value.
        let ig_time = self.ignore_effective();
        if ig_time != self.ignore_cur {
            out.push(("IgnoreTime".to_string(), yesno(ig_time)));
        }
        // The global IgnoreFilters gates time as well — when writing a schedule we clear it if YES.
        if schedule && self.ign_filters_cur {
            out.push(("IgnoreFilters".to_string(), "NO".to_string()));
        }
        out
    }

    /// Bulk (multi-select) change set: push the schedule fields whenever they are non-empty
    /// — NOT gated on the anchor's current values, since the other targets differ — and always
    /// disable IgnoreTime + IgnoreFilters so the schedule actually applies on every target.
    /// Writing NO to a target that already had NO is idempotent, so this is safe to fan out.
    pub(super) fn changes_forced(&self) -> Vec<(String, String)> {
        const NAMES: [&str; 2] = ["WorkingWeekTime", "WorkingTime"];
        let mut out: Vec<(String, String)> = (0..2)
            .filter_map(|which| {
                let v = self.field_value(0, which);
                (!v.is_empty()).then(|| (NAMES[which].to_string(), v))
            })
            .collect();
        if !out.is_empty() {
            out.push(("IgnoreTime".to_string(), "NO".to_string()));
            out.push(("IgnoreFilters".to_string(), "NO".to_string()));
        }
        out
    }

    /// Reset the grid when the strategy changes (v1/v2 + inputs + current values + the ignore
    /// switch), and discard an in-flight auto-suggestion.
    pub(super) fn reset_grid(&mut self) {
        self.bounds = vec![std::array::from_fn(|_| (String::new(), String::new())); TIME_N_VAR];
        self.current_raw = Default::default();
        self.ignore_staged = None;
        self.inputs.clear();
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
        self.sugg_busy = false;
    }

    /// Discard an IN-FLIGHT auto-suggestion when the QUERY changes (period/filter) — just like the
    /// filter's `TunerState::invalidate`: bump the generation and clear busy WITHOUT touching
    /// v1. Otherwise a stale result from the old query would land in v1 (and become Saveable).
    pub(in crate::analytics) fn invalidate_suggest(&mut self) {
        self.sugg_seq = self.sugg_seq.wrapping_add(1);
        self.sugg_busy = false;
    }
}

/// Minutes of the day → "hh:mm".
pub(super) fn fmt_min(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// One edge of a week span `(from, to)` as the input string "day.hh:mm" (or just "day" on a
/// day boundary): the start of the week/day for "from" and the end for "to" — short, no time.
pub(super) fn fmt_week_ep(wm: u16, is_to: bool) -> String {
    let (day, tod) = (wm / 1440 % 7 + 1, wm % 1440);
    if (!is_to && tod == 0) || (is_to && tod == 1439) {
        day.to_string()
    } else {
        format!("{day}.{}", fmt_min(tod))
    }
}

/// Parse one edge of a week span from the field: "day" or "day.hh:mm" (day 1..7) →
/// minute of the week 0..WEEK_MIN-1. Without a time: "from" → start of the day, "to" → end of
/// the day. Empty/garbage/outside 1..7 → None.
fn parse_week_ep(s: &str, is_to: bool) -> Option<u16> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (day_s, tod) = match s.split_once('.') {
        Some((d, t)) => (d, parse_time(t)?),
        None => (s, if is_to { 1439 } else { 0 }),
    };
    let day: u16 = day_s.trim().parse().ok()?;
    if !(1..=7).contains(&day) {
        return None;
    }
    Some((day - 1) * 1440 + tod)
}

/// Minute of the hour 0..59 from the field. Empty/garbage → None; above 59 is clamped to 59.
pub(super) fn parse_moh(s: &str) -> Option<u8> {
    s.trim().parse::<u8>().ok().map(|v| v.min(59))
}

/// Time of day from the field: "hh:mm" or minutes of the day; empty/garbage = None.
pub(super) fn parse_time(s: &str) -> Option<u16> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((h, m)) = s.split_once(':') {
        let h: u16 = h.trim().parse().ok()?;
        let m: u16 = m.trim().parse().ok()?;
        if h > 23 || m > 59 {
            return None;
        }
        Some(h * 60 + m)
    } else {
        s.parse::<u16>().ok().map(|v| v.min(1439))
    }
}

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
    pub(super) fn clear_field(&mut self, vi: usize, field: usize) {
        self.time_tuner.bounds[vi][field] = (String::new(), String::new());
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

    /// Auto-suggestion into v1 (background): the week window (WorkingWeekTime) + the
    /// WorkingTime window, where the mode (Day vs In hour) is chosen BY the auto-suggestion
    /// itself — it fills ONE of the "Day"/"In hour" rows and clears the other. Scope is
    /// `tuner_query`. No improvement
    /// → empty.
    pub(super) fn time_suggest(&mut self, cx: &mut Context<Self>) {
        if self.time_tuner.sugg_busy {
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
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move { moon_core::db::tuner::suggest_time(&q, min_n, edges, round) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.time_tuner.sugg_seq != req {
                        return; // stale auto-suggestion (the strategy/scope changed)
                    }
                    this.time_tuner.sugg_busy = false;
                    match sugg {
                        Ok(s) => {
                            // Field 0: the week window as "day.hh:mm".
                            let (w0, w1) = s
                                .week_span
                                .map(|(f, t)| (fmt_week_ep(f, false), fmt_week_ep(t, true)))
                                .unwrap_or_default();
                            this.set_v1_cell(0, w0, w1);
                            // WorkingTime: the auto-suggestion chose the mode itself → fill ITS row
                            // and clear the other one (mutual exclusion).
                            match s.tod {
                                Some(TimeWindow::Day(f, t)) => {
                                    this.set_v1_cell(1, fmt_min(f), fmt_min(t));
                                    this.clear_field(0, 2);
                                }
                                Some(TimeWindow::Hour(f, t)) => {
                                    this.set_v1_cell(2, f.to_string(), t.to_string());
                                    this.clear_field(0, 1);
                                }
                                None => {
                                    this.clear_field(0, 1);
                                    this.clear_field(0, 2);
                                }
                            }
                            this.reload_time(cx);
                        }
                        Err(e) => log::warn!("analytics: time auto-suggestion failed — {e}"),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Write `(from, to)` into v1 of field `field` and drop its cached inputs.
    pub(super) fn set_v1_cell(&mut self, field: usize, from: String, to: String) {
        self.time_tuner.bounds[0][field] = (from, to);
        self.time_tuner.inputs.remove(&format!("tt0f{field}a"));
        self.time_tuner.inputs.remove(&format!("tt0f{field}b"));
    }

    /// Clear the WHOLE variant column (the ✕ in the header) — every from/to field of that vi.
    fn time_clear_variant(&mut self, vi: usize, cx: &mut Context<Self>) {
        for field in 0..N_FIELD {
            self.clear_field(vi, field);
        }
        self.reload_time(cx);
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
    fn toggle_ignore_time(&mut self, cx: &mut Context<Self>) {
        let cur = self.time_tuner.ignore_cur;
        let next = !self.time_tuner.ignore_effective();
        self.time_tuner.ignore_staged = (next != cur).then_some(next);
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
    pub(super) fn time_grid(
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
            .child(
                // Matches the row's field-name column width (fits "WorkingWeekTime").
                div()
                    .w(design::font_w_px(cx, 118.0))
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

        let mut row = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 3.0))
            .items_center()
            // Must match the header gap, or the columns drift apart.
            .gap(design::ui_px(cx, 4.0))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.5))
            .opacity(if dim { 0.45 } else { 1.0 })
            .child(
                // Field name column — wide enough for the full "WorkingWeekTime" on one line;
                // truncates as a safety net.
                div()
                    .w(design::font_w_px(cx, 118.0))
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
        row.into_any_element()
    }
}
