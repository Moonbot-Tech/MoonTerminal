//! State of the "By time" tuner: the three field rows' values, what the sweep may
//! search, and the parsers/formatters for the two schedule fields. Pure data — the grid
//! that renders it lives in `grid`.

use std::collections::HashMap;

use gpui::Entity;
use moon_ui::MoonInputState;

use moon_core::db::tuner::{TimeAxes, TimeWindow, Variant, format_week_span, format_working_time};
/// Variants besides the "Fact" one (v1, v2) — same as the filter tuner's N_VAR.
pub(super) const TIME_N_VAR: usize = 2;
/// Fields: 0 = Weekly (WorkingWeekTime), 1 = Day, 2 = In hour (both are WorkingTime).
pub(super) const N_FIELD: usize = 3;
/// Minutes in a week (7 days × 1440) — the maximum minute of the week + 1.
pub(super) const WEEK_MIN: u16 = 7 * 1440;

/// State of the "By time" tuner inside `AnalyticsView`.
pub(in crate::analytics) struct TimeTunerState {
    /// `[variant 0..TIME_N_VAR][field 0..N_FIELD] = (from, to)` as strings.
    pub(super) bounds: Vec<[(String, String); N_FIELD]>,
    /// Row checkboxes — "consider this field in the sweep", as in the filter tuner. All
    /// three are independent, and an unchecked row is never rewritten. Ticking both
    /// WorkingTime rows lets the sweep pick the better FORMAT for that one field; pinning
    /// is therefore per field, not per row — see `TimeAxes`.
    pub(super) enabled: [bool; N_FIELD],
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
    /// Minimum trade count as raw input text; empty selects the automatic threshold.
    pub(super) min_trades: String,
    /// Whether suggested bounds are rounded outward.
    pub(super) round_results: bool,
    /// Whether an auto-suggestion is in flight.
    pub(super) sugg_busy: bool,
    /// Auto-suggestion generation — a stale result is discarded.
    pub(super) sugg_seq: u64,
}

impl TimeTunerState {
    /// A fresh tuner: all fields empty.
    pub(in crate::analytics) fn load() -> Self {
        Self {
            bounds: vec![std::array::from_fn(|_| (String::new(), String::new())); TIME_N_VAR],
            // Everything searched by default: "sweep it all and see what comes out" is the
            // normal way in, and both WorkingTime formats compete for that one field.
            enabled: [true; N_FIELD],
            current_raw: Default::default(),
            ignore_cur: false,
            ign_filters_cur: false,
            ignore_staged: None,
            inputs: HashMap::new(),
            min_trades: String::new(),
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
    /// "In hour" row (field 2 → `Hour`). Only one of them ever holds a value — every write
    /// path clears the sibling — so this reads the live one. Both filled (shouldn't
    /// happen) → "Day" wins.
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

    /// Which of the Day/In-hour pair currently HOLDS the WorkingTime value (dims the unused
    /// row + its slider): `Some(1)`=Day, `Some(2)`=In hour, `None`=both empty. This follows
    /// the value, not the checkbox — the checkboxes only decide what the sweep may try, and
    /// both of them can be ticked at once.
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

    /// Axes for the sweep, straight from the row checkboxes: what to search, and what an
    /// unchecked-but-filled row pins it to.
    pub(super) fn axes(&self) -> TimeAxes {
        TimeAxes {
            week: self.enabled[0],
            day: self.enabled[1],
            hour: self.enabled[2],
            // A full week/day window is "no restriction" — same rule the KPI variants use,
            // so it must not reach the sweep as a constraint either.
            fixed_week: (!self.enabled[0])
                .then(|| self.week_span(0).filter(|&s| s != (0, WEEK_MIN - 1)))
                .flatten(),
            // Pinned only when NEITHER format may be searched: with either row ticked the
            // sweep owns the field and would overwrite the pin anyway (it is one field).
            fixed_tod: (!self.enabled[1] && !self.enabled[2])
                .then(|| self.tod(0))
                .flatten(),
        }
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
