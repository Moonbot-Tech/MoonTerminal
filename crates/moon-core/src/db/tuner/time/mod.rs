//! Time-window parsing, profiling, and automatic schedule suggestions.

use rusqlite::Connection;

use super::{
    best_range, improvement_margin, read_tuner_rows, scan_time_rows, tuner_source_on,
    visit_time_rows, HourStat, Query, ReadResult,
};

/// WorkingTime: one strategy time range. `Day` is a time-of-day window (minutes 0..1439,
/// serialized as "hh:mm-hh:mm"); `Hour` is a minute window within EVERY HOUR (0..59,
/// serialized as "N-M"). Both are single continuous ranges (`from > to` wraps across the end
/// of the day or hour).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimeWindow {
    Day(u16, u16),
    Hour(u8, u8),
}

/// Convert a week-minute span `(from, to)` (0..10079) into a `WorkingWeekTime` field string,
/// `day.hh:mm-day.hh:mm` (1-based day: 1=Mon..7=Sun). Shorten a bound on a day boundary to
/// just `day`: minute 0 for the start and minute 1439 for the end. Thus `1-5` means Mon 00:00
/// through Fri 23:59, while `1.23:44-6.22:22` includes explicit times. The caller does not
/// write a full week because it means no restriction.
pub fn format_week_span((f, t): (u16, u16)) -> String {
    let ends = |m: u16, at_end: bool| {
        let (day, tod) = ((m / 1440) % 7 + 1, m % 1440);
        // Omit the time for a start at minute 0 or an end at minute 1439.
        if (!at_end && tod == 0) || (at_end && tod == 1439) {
            day.to_string()
        } else {
            format!("{day}.{}", fmt_min(tod))
        }
    };
    format!("{}-{}", ends(f, false), ends(t, true))
}

/// Convert `TimeWindow` into a `WorkingTime` field string. `Day` becomes "hh:mm-hh:mm";
/// `Hour` becomes "N-M" (MoonBot identifies minutes within each hour by the absence of ":").
pub fn format_working_time(tw: TimeWindow) -> String {
    match tw {
        TimeWindow::Day(f, t) => format!("{}-{}", fmt_min(f), fmt_min(t)),
        TimeWindow::Hour(f, t) => format!("{f}-{t}"),
    }
}

/// Convert minutes of the day to "hh:mm".
fn fmt_min(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Automatic-suggestion result for the "By time" axis: two independent strategy fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimeSuggest {
    /// WorkingWeekTime: best continuous span over the week minute (0..10079).
    /// `None` means no improvement.
    pub week_span: Option<(u16, u16)>,
    /// WorkingTime: the best window in the requested format. `None` — no improvement.
    pub tod: Option<TimeWindow>,
}

/// What the "By time" sweep may search, straight from the row checkboxes: a CHECKED row is
/// searched and may be rewritten; an UNCHECKED one is never rewritten.
///
/// `day` and `hour` are two FORMATS of the single `WorkingTime` field, so they are two
/// competing candidates, not two fields: with both checked the sweep tries each and keeps
/// whichever earns more, which is why the result is at most TWO windows (a week span plus
/// one `WorkingTime` window) no matter how many boxes are ticked.
///
/// Hence pinning is per FIELD, not per row. `WorkingWeekTime` is its own field, so an
/// unchecked "Weekly" row holding a value pins the sweep inside it (`fixed_week`). For
/// `WorkingTime` a pin only exists when NEITHER format may be searched (`fixed_tod`): with
/// either box ticked the sweep owns that field and its answer REPLACES whatever is in the
/// pair — a value there is a candidate being replaced, not a constraint, since the two
/// formats cannot both be written to one field.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimeAxes {
    /// The "Weekly" row is checked → search `WorkingWeekTime`.
    pub week: bool,
    /// The "Day" row is checked → try `WorkingTime` as `hh:mm-hh:mm` (minute of the day).
    pub day: bool,
    /// The "In hour" row is checked → try `WorkingTime` as `N-M` (minute of the hour).
    pub hour: bool,
    /// Week span pinned by an UNCHECKED "Weekly" row (`None` — the row is empty).
    pub fixed_week: Option<(u16, u16)>,
    /// `WorkingTime` window pinned when NEITHER of its two rows is checked.
    pub fixed_tod: Option<TimeWindow>,
}

impl TimeAxes {
    /// Is there anything at all to search (otherwise the whole read is pointless).
    ///
    /// Returns:
    ///     `true` when no time axis is enabled.
    pub fn is_empty(&self) -> bool {
        !self.week && !self.day && !self.hour
    }
}

/// Profit sweep over the schedule fields: a continuous span of days (`WorkingWeekTime`)
/// and one time window (`WorkingTime`). What to search is dictated by `axes` (the row
/// checkboxes); with both `WorkingTime` formats checked the sweep tries each and keeps the
/// better one, and a row that is unchecked but already holds a value pins the sweep inside
/// it. The two fields are independent; each is `None` when the best candidate does not
/// improve on the unrestricted sample. Using `OPEN_TS`, the UTC weekday is
/// `(((OPEN_TS/86400)+4)%7+6)%7` (0=Mon..6=Sun), and the minute is
/// `(OPEN_TS%86400)/60`.
///
/// Args:
///     q: Report scope and profit metric.
///     min_n: Minimum trades retained by a schedule candidate.
///     edges: Search resolution for continuous time windows.
///     round: Whether accepted boundaries are rounded outward.
///     axes: Enabled and pinned schedule axes.
///
/// Returns:
///     Best schedule changes or a classified read failure.
pub fn suggest_time(
    q: &Query,
    min_n: i64,
    edges: usize,
    round: bool,
    axes: TimeAxes,
) -> ReadResult<TimeSuggest> {
    // Nothing is checked → nothing to search; skip the scan instead of reading the whole
    // period only to throw it away.
    if axes.is_empty() {
        return Ok(TimeSuggest::default());
    }
    let trades = read_tuner_rows(q, |conn, q, src| {
        scan_time_rows(conn, q, src, "tuner: suggest_time")
    })?;
    Ok(time_suggest_from_rows(&trades, min_n, edges, round, axes))
}

/// Core of `suggest_time` over ready rows `(weekday, minute, profit)` — the entry point
/// for unit tests (no DB). Only the axes opened by `axes` are searched; rows cut off by
/// the pinned windows (`fixed_*`) are dropped BEFORE the sweep, so the free axis is
/// optimized inside them. The week and time axes are searched INDEPENDENTLY (each the
/// best window of its own projection) but applied with AND. Their intersection can yield
/// LESS profit than the baseline (each drops different winning trades), so we score every
/// combination on the real rows and take the maximum — the baseline is a candidate too,
/// which is why the result is NEVER worse than it.
///
/// NB: with a `fixed_*` window the baseline is the PINNED subset, not the whole sample —
/// the pin is a constraint the sweep may not lift, so "never worse" is measured against
/// what the user fixed. Without pins the baseline is the full sample, i.e. the "Fact"
/// column the UI shows.
fn time_suggest_from_rows(
    rows: &[(i64, i64, f64)],
    min_n: i64,
    edges: usize,
    round: bool,
    axes: TimeAxes,
) -> TimeSuggest {
    if axes.is_empty() {
        return TimeSuggest::default();
    }
    let min_n = min_n.max(1) as usize;
    // Is a trade inside the mask (bounds inclusive; `from > to` = wraps past the edge).
    let span_ok = |v: i64, f: i64, t: i64| {
        if f <= t {
            f <= v && v <= t
        } else {
            v <= t || v >= f
        }
    };
    let in_tod = |mn: i64, tw: TimeWindow| match tw {
        TimeWindow::Day(f, t) => span_ok(mn, f as i64, t as i64),
        TimeWindow::Hour(f, t) => span_ok(mn % 60, f as i64, t as i64),
    };
    // An unchecked row with a value pins the sweep: drop everything outside it, so both the
    // candidates and the baseline below are measured on the subset the user fixed. Nothing
    // pinned → work on the original slice (no copy).
    let pinned: Option<Vec<(i64, i64, f64)>> =
        (axes.fixed_week.is_some() || axes.fixed_tod.is_some()).then(|| {
            rows.iter()
                .copied()
                .filter(|&(wd, mn, _)| {
                    axes.fixed_week
                        .is_none_or(|(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                        && axes.fixed_tod.is_none_or(|tw| in_tod(mn, tw))
                })
                .collect()
        });
    let rows: &[(i64, i64, f64)] = pinned.as_deref().unwrap_or(rows);
    // Candidates for the opened axes (best window of the projection; None = unrestricted).
    let week = axes
        .week
        .then(|| best_week_span(rows, min_n, edges, round))
        .flatten();
    // The two WorkingTime formats are separate CANDIDATES for one field: each checked row
    // contributes one, and the comparison below keeps whichever earns more. Unchecked → no
    // candidate, so that format can never come out of the sweep.
    let day_w = axes
        .day
        .then(|| {
            let mut vals: Vec<(f64, f64)> =
                rows.iter().map(|&(_w, mn, p)| (mn as f64, p)).collect();
            best_range(&mut vals, min_n, edges, round).map(|s| {
                TimeWindow::Day(
                    s.from.unwrap_or(0.0).clamp(0.0, 1439.0) as u16,
                    s.to.unwrap_or(1439.0).clamp(0.0, 1439.0) as u16,
                )
            })
        })
        .flatten();
    let hour_w = axes
        .hour
        .then(|| {
            let mut vals: Vec<(f64, f64)> = rows
                .iter()
                .map(|&(_w, mn, p)| ((mn % 60) as f64, p))
                .collect();
            best_range(&mut vals, min_n, edges, round).map(|s| {
                TimeWindow::Hour(
                    s.from.unwrap_or(0.0).clamp(0.0, 59.0) as u8,
                    s.to.unwrap_or(59.0).clamp(0.0, 59.0) as u8,
                )
            })
        })
        .flatten();
    let prof = |wk: Option<(u16, u16)>, td: Option<TimeWindow>| -> f64 {
        rows.iter()
            .filter(|&&(wd, mn, _)| {
                wk.is_none_or(|(f, t)| span_ok(wd * 1440 + mn, f as i64, t as i64))
                    && td.is_none_or(|tw| in_tod(mn, tw))
            })
            .map(|r| r.2)
            .sum()
    };
    // The baseline (no window of our own, but already inside any pin) is the starting
    // candidate; every other one has to BEAT it.
    let base: f64 = rows.iter().map(|r| r.2).sum();
    let margin = improvement_margin(base);
    let mut best: (Option<(u16, u16)>, Option<TimeWindow>, f64) = (None, None, base);
    for wk in [None, week] {
        for td in [None, day_w, hour_w] {
            if wk.is_none() && td.is_none() {
                continue;
            }
            let pr = prof(wk, td);
            if pr > best.2 + margin {
                best = (wk, td, pr);
            }
        }
    }
    TimeSuggest {
        week_span: best.0,
        tod: best.1,
    }
}

/// Best CONTINUOUS profit window over the week minute
/// (0..10079 = day*1440 + minute_of_day), found through quantile-based `best_range`.
/// Returns `None` when the best window does not improve on the full week. The linear search
/// (`from <= to`) does not suggest a window wrapping PAST Sunday, though the manual slider can.
fn best_week_span(
    rows: &[(i64, i64, f64)],
    min_n: usize,
    edges: usize,
    round: bool,
) -> Option<(u16, u16)> {
    let mut vals: Vec<(f64, f64)> = rows
        .iter()
        .filter(|(wd, ..)| (0..7).contains(wd))
        .map(|&(wd, mn, p)| ((wd * 1440 + mn) as f64, p))
        .collect();
    best_range(&mut vals, min_n, edges, round).map(|s| {
        (
            s.from.unwrap_or(0.0).clamp(0.0, 10079.0) as u16,
            s.to.unwrap_or(10079.0).clamp(0.0, 10079.0) as u16,
        )
    })
}

/// Average-profit profiles for coloring the three "By time" sliders:
/// `week[168]` (day x hour, 0=Mon 00:00), `day[1440]` (minute of day), and `hour[60]`
/// (minute within the hour). Each cell contains its average trade profit, or 0.0 if empty.
#[derive(Clone, Copy, Debug)]
pub struct SliderProfiles {
    pub week: [f32; 168],
    pub day: [f32; 1440],
    pub hour: [f32; 60],
    /// Entry-hour KPI profile derived by the same streaming row pass.
    pub entry_hours: [HourStat; 24],
}

/// Calculate `SliderProfiles` for the same scope as automatic suggestions (`tuner_query`).
pub fn slider_profiles(q: &Query) -> ReadResult<SliderProfiles> {
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| slider_profiles_on(snapshot, q))
}

/// Calculate slider profiles on an existing connection or compound-read snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     q: Report scope and period.
///
/// Returns:
///     Three color profiles or a classified read failure.
pub(super) fn slider_profiles_on(conn: &Connection, q: &Query) -> ReadResult<SliderProfiles> {
    let (q, src) = tuner_source_on(conn, q)?;
    slider_profiles_from_source(conn, &q, &src)
}

/// Calculate slider profiles from a source already validated in the same snapshot.
///
/// Args:
///     conn: Pinned report snapshot.
///     q: Floored tuner query used by `src`.
///     src: Unified source whose quote scope was already validated.
///
/// Returns:
///     Three color profiles or a classified row-scan failure.
pub(super) fn slider_profiles_from_source(
    conn: &Connection,
    q: &Query,
    src: &str,
) -> ReadResult<SliderProfiles> {
    let mut accumulator = SliderProfileAccumulator::new();
    visit_time_rows(
        conn,
        q,
        src,
        "tuner: slider_profiles",
        |weekday, minute, profit| accumulator.push(weekday, minute, profit),
    )?;
    Ok(accumulator.finish())
}

/// Core of `slider_profiles` over `(day, minute_of_day, profit)` rows, for tests.
#[cfg(test)]
fn slider_profiles_from_rows(rows: &[(i64, i64, f64)]) -> SliderProfiles {
    let mut accumulator = SliderProfileAccumulator::new();
    for &(wd, mn, p) in rows {
        accumulator.push(wd, mn, p);
    }
    accumulator.finish()
}

/// Fixed-size accumulator for the three time heatmaps.
struct SliderProfileAccumulator {
    week_sum: [f64; 168],
    week_count: [u32; 168],
    day_sum: Vec<f64>,
    day_count: Vec<u32>,
    hour_sum: [f64; 60],
    hour_count: [u32; 60],
    entry_hours: [HourStat; 24],
}

impl SliderProfileAccumulator {
    /// Allocate the fixed 168/1440/60 buckets.
    fn new() -> Self {
        Self {
            week_sum: [0.0; 168],
            week_count: [0; 168],
            day_sum: vec![0.0; 1440],
            day_count: vec![0; 1440],
            hour_sum: [0.0; 60],
            hour_count: [0; 60],
            entry_hours: [HourStat::default(); 24],
        }
    }

    /// Add one decoded report row, ignoring invalid time coordinates.
    fn push(&mut self, wd: i64, mn: i64, profit: f64) {
        if !(0..7).contains(&wd) || !(0..1440).contains(&mn) {
            return;
        }
        let (mi, h, moh) = (mn as usize, (mn / 60) as usize, (mn % 60) as usize);
        let wk = wd as usize * 24 + h;
        self.week_sum[wk] += profit;
        self.week_count[wk] += 1;
        self.day_sum[mi] += profit;
        self.day_count[mi] += 1;
        self.hour_sum[moh] += profit;
        self.hour_count[moh] += 1;
        self.entry_hours[h].profit += profit;
        self.entry_hours[h].trades += 1;
        self.entry_hours[h].wins += i64::from(profit > 0.0);
    }

    /// Finish the bounded accumulator as average-profit profiles.
    fn finish(self) -> SliderProfiles {
        let avg = |sum: f64, count: u32| {
            if count > 0 {
                (sum / count as f64) as f32
            } else {
                0.0
            }
        };
        SliderProfiles {
            week: std::array::from_fn(|i| avg(self.week_sum[i], self.week_count[i])),
            day: std::array::from_fn(|i| avg(self.day_sum[i], self.day_count[i])),
            hour: std::array::from_fn(|i| avg(self.hour_sum[i], self.hour_count[i])),
            entry_hours: self.entry_hours,
        }
    }
}

#[cfg(test)]
mod tests;
