//! Time-window parsing, profiling, and automatic schedule suggestions.

mod evaluation;
mod search;
pub use evaluation::{TimeEvaluation, suggest_time_checked};

use rusqlite::Connection;

use super::{
    HourStat, Query, ReadResult, improvement_margin, read_tuner_rows, scan_time_rows,
    tuner_source_on, visit_time_rows,
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
    format!(
        "{}-{}",
        crate::util::time::format_week_endpoint(f, false),
        crate::util::time::format_week_endpoint(t, true)
    )
}

/// Convert `TimeWindow` into a `WorkingTime` field string. `Day` becomes "hh:mm-hh:mm";
/// `Hour` becomes "N-M" (MoonBot identifies minutes within each hour by the absence of ":").
pub fn format_working_time(tw: TimeWindow) -> String {
    match tw {
        TimeWindow::Day(f, t) => format!(
            "{}-{}",
            crate::util::time::format_hhmm(f),
            crate::util::time::format_hhmm(t)
        ),
        TimeWindow::Hour(f, t) => format!("{f}-{t}"),
    }
}

/// Automatic-suggestion result for the "By time" axis: two jointly chosen strategy fields.
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
/// it. Circular windows are exact on each axis; bounded multi-start refinement chooses their
/// joint application. Weekday and minute are read from `OPEN_TS` on the core's own clock,
/// independent of the display zone, matching the saved schedule and KPI predicates.
///
/// Args:
///     q: Report scope and profit metric.
///     min_n: Minimum trades retained by a schedule candidate.
///     edges: Legacy caller resolution; time boundaries are now always exact.
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

/// Fit exact circular axes inside pinned bounds, then refine both fields jointly.
/// Each coordinate step is exact; bounded multi-start refinement also retains every
/// independent candidate, so joint fitting cannot make that earlier answer worse.
fn time_suggest_from_rows(
    rows: &[(i64, i64, f64)],
    min_n: i64,
    _edges: usize,
    round: bool,
    axes: TimeAxes,
) -> TimeSuggest {
    if axes.is_empty() {
        return TimeSuggest::default();
    }
    let rows: Vec<_> = rows
        .iter()
        .copied()
        .filter(|&(wd, mn, _)| {
            (0..7).contains(&wd)
                && (0..1440).contains(&mn)
                && accepts(wd, mn, axes.fixed_week, axes.fixed_tod)
        })
        .collect();
    let min_n = min_n.max(1) as usize;
    let base: f64 = rows.iter().map(|r| r.2).sum();
    let margin = improvement_margin(base);
    let mut best = TimeSuggest::default();
    let mut best_profit = base;
    let week = |td| {
        search::best_span(
            rows.iter()
                .filter(|&&(_, mn, _)| accepts(0, mn, None, td))
                .map(|&(wd, mn, p)| ((wd * 1440 + mn) as usize, p)),
            10080,
            min_n,
            round,
        )
    };
    let tod = |wk, hour| {
        let period = if hour { 60 } else { 1440 };
        search::best_span(
            rows.iter()
                .filter(|&&(wd, mn, _)| accepts(wd, mn, wk, None))
                .map(|&(_, mn, p)| (mn as usize % period, p)),
            period,
            min_n,
            round,
        )
        .map(|(f, t)| {
            if hour {
                TimeWindow::Hour(f as u8, t as u8)
            } else {
                TimeWindow::Day(f, t)
            }
        })
    };
    let wk = axes.week.then(|| week(None)).flatten();
    let day = axes.day.then(|| tod(None, false)).flatten();
    let hour = axes.hour.then(|| tod(None, true)).flatten();
    for start_week in [None, wk] {
        for start_tod in [None, day, hour] {
            let mut candidate = TimeSuggest {
                week_span: start_week,
                tod: start_tod,
            };
            for step in 0..4 {
                let kept: Vec<_> = rows
                    .iter()
                    .filter(|&&(wd, mn, _)| accepts(wd, mn, candidate.week_span, candidate.tod))
                    .collect();
                let profit: f64 = kept.iter().map(|r| r.2).sum();
                if kept.len() >= min_n && profit > best_profit + margin {
                    best_profit = profit;
                    best = candidate;
                }
                if step == 3 {
                    break;
                }
                if axes.week {
                    candidate.week_span = week(candidate.tod);
                }
                let mut choices = vec![candidate];
                if axes.day {
                    choices.push(TimeSuggest {
                        tod: tod(candidate.week_span, false),
                        ..candidate
                    });
                }
                if axes.hour {
                    choices.push(TimeSuggest {
                        tod: tod(candidate.week_span, true),
                        ..candidate
                    });
                }
                candidate = choices
                    .into_iter()
                    .max_by(|a, b| {
                        let score = |s: &TimeSuggest| {
                            rows.iter()
                                .filter(|&&(wd, mn, _)| accepts(wd, mn, s.week_span, s.tod))
                                .map(|r| r.2)
                                .sum::<f64>()
                        };
                        score(a).total_cmp(&score(b))
                    })
                    .unwrap_or_default();
            }
        }
    }
    best
}

/// Inclusive schedule predicate shared by fitting and chronological validation.
fn accepts(wd: i64, mn: i64, week: Option<(u16, u16)>, tod: Option<TimeWindow>) -> bool {
    let span = |v: i64, f: u16, t: u16| {
        if f <= t {
            f as i64 <= v && v <= t as i64
        } else {
            v >= f as i64 || v <= t as i64
        }
    };
    week.is_none_or(|(f, t)| span(wd * 1440 + mn, f, t))
        && tod.is_none_or(|tw| match tw {
            TimeWindow::Day(f, t) => span(mn, f, t),
            TimeWindow::Hour(f, t) => span(mn % 60, f as u16, t as u16),
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
