//! "Profit calendar" tab of the "Analytics" window.
//! Three modes (one segmented control, like the Summary period presets):
//! - "Year" — every year at once, each one a grid of 12 month squares (current
//!   year on top, future months greyed out). Clicking a month switches to
//!   "Month" mode. See `year`;
//! - "Month" — large day cards (date on the left; on the right the PnL, the win
//!   rate with its counts, turnover and execution cost, and the average holding
//!   time), grey background + red/green overlay whose alpha tracks |PnL|; a KPI
//!   row on top (with the delta to the previous month), a plus/minus-day bar
//!   below. Clicking a day switches to "Day" mode;
//! - "Day" — hour-by-hour detail of a single day. See `day`.
//! Each mode builds its own query (`cal_query` in mod.rs); the window's period
//! bar is hidden here.

mod day;
mod month;
mod year;

use chrono::{Datelike, NaiveDate};
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use super::{AnalyticsView, ProfitLoadState};
use crate::design;
use crate::design::moon;
use crate::load_state::{LoadState, note_el};
use moon_core::db::analytics::{CalendarPeriod, CellTotals, DayCell, PreviousPeriodBasis, Query};
use moon_core::db::{ProfitScope, ProfitUnit, ReadResult};

/// Store one consistent Calendar period result without collapsing classified read failures.
///
/// Args:
///     days: Current-period UI load state.
///     previous: Previous-month aggregate UI load state.
///     period_result: Current cells and optional comparison from one SQLite snapshot.
///     has_previous: Whether the active mode requested a comparison period.
///     preserve_transient: Whether a same-scope catch-up should keep the visible days and
///         previous-period cells instead of publishing the classified result — decided by the
///         caller via `refresh::preserve_on_catch_up` (a transient outcome with a scheduled
///         correction left; every other outcome is still published).
///
/// Returns:
///     `true` when the shared period read did not settle into a real replacement (a failure or a
///     split totals result, published or preserved alike), so `cal_dirty` stays set.
fn apply_calendar_results(
    days: &mut ProfitLoadState<Vec<DayCell>>,
    previous: &mut LoadState<Option<CellTotals>>,
    period_result: ReadResult<ProfitScope<CalendarPeriod>>,
    has_previous: bool,
    preserve_transient: bool,
) -> bool {
    match period_result {
        Ok(ProfitScope::Comparable { unit, data: period }) => {
            days.apply(Ok(ProfitScope::Comparable {
                unit,
                data: period.current,
            }));
            previous.apply(Ok(period.previous));
            false
        }
        Ok(ProfitScope::Empty(period)) => {
            days.apply(Ok(ProfitScope::Empty(period.current)));
            previous.apply(Ok(period.previous));
            false
        }
        Ok(ProfitScope::Split(totals)) => {
            // Mirrors the `Err` arm: a Split result is never a real replacement, so `cal_dirty`
            // stays set regardless of whether this pass published it or kept the prior snapshot.
            if !preserve_transient {
                days.apply(Ok(ProfitScope::Split(totals)));
                previous.apply(Ok(None));
            }
            true
        }
        Err(error) => {
            if !preserve_transient {
                days.apply(Err(error.clone()));
                previous.apply(if has_previous { Err(error) } else { Ok(None) });
            }
            true
        }
    }
}

/// Base floor for a calendar mode cell's fitted width.
const CAL_MODE_CELL_MIN_W: f32 = 44.0;

/// Base ceiling for a calendar mode cell's fitted width.
///
/// A long localized label cannot stretch the strip across the window; MoonUI truncates the visible
/// label, and the item's tooltip still carries the whole of it.
const CAL_MODE_CELL_MAX_W: f32 = 104.0;

/// Calendar mode (the tab's zoom level).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CalMode {
    Day,
    Month,
    Year,
}

impl CalMode {
    pub(super) const ALL: [CalMode; 3] = [CalMode::Year, CalMode::Month, CalMode::Day];
    pub(super) fn id(self) -> &'static str {
        match self {
            CalMode::Day => "day",
            CalMode::Month => "month",
            CalMode::Year => "year",
        }
    }
    pub(super) fn from_id(id: &str) -> Option<CalMode> {
        CalMode::ALL.into_iter().find(|m| m.id() == id)
    }
    fn title(self) -> String {
        match self {
            CalMode::Day => t!("analytics.cal.day"),
            CalMode::Month => t!("analytics.heat.month"),
            CalMode::Year => t!("analytics.heat.year"),
        }
        .to_string()
    }
}

// ── Calendar date helpers ───────────────────────────────────────────────────

/// Return the selected-zone civil date at one UTC instant.
///
/// Args:
///     secs: Absolute Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil date, or chrono's default date outside its representable range.
pub(super) fn date_of(secs: i64, zone: chrono_tz::Tz) -> NaiveDate {
    super::day_of_secs(secs, zone).unwrap_or_default()
}

/// Resolve a Calendar date while stepping past a fully skipped historical date.
///
/// Args:
///     date: First civil date to try.
///     zone: Selected IANA display zone.
///     direction: Negative to search earlier dates, non-negative to search later dates.
///
/// Returns:
///     First existing civil-day start in the requested direction, or `None` at chrono limits.
pub(super) fn resolve_calendar_date(
    date: NaiveDate,
    zone: chrono_tz::Tz,
    direction: i64,
) -> Option<i64> {
    moon_core::util::display_time::resolve_day_start(date, zone, direction)
}

/// Return the current selected-zone year and month that Calendar cannot page beyond.
///
/// Args:
///     zone: Selected IANA display zone.
///
/// Returns:
///     Current civil `(year, month)`.
pub(super) fn now_ym(zone: chrono_tz::Tz) -> (i32, u32) {
    let d = date_of(moon_core::util::now_unix_ms_i64() / 1000, zone);
    (d.year(), d.month())
}

/// Resolve the current selected-zone day's first instant for the future-grid boundary.
///
/// Args:
///     zone: Selected IANA display zone.
///
/// Returns:
///     UTC Unix seconds, or zero outside chrono's range.
pub(super) fn today_start(zone: chrono_tz::Tz) -> i64 {
    moon_core::util::display_time::bucket_start(
        moon_core::util::now_unix_ms_i64() / 1000,
        86_400,
        zone,
    )
    .unwrap_or(0)
}

/// Resolve the first instant of a selected-zone calendar month.
///
/// Args:
///     y: Civil year.
///     m: Civil month from 1 through 12.
///     zone: Selected IANA display zone.
///
/// Returns:
///     UTC Unix seconds, or zero for an invalid month or out-of-range instant.
pub(super) fn month_start(y: i32, m: u32, zone: chrono_tz::Tz) -> i64 {
    super::secs_of_day(NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_default(), zone)
}

pub(super) fn next_month(y: i32, m: u32) -> (i32, u32) {
    if m == 12 { (y + 1, 1) } else { (y, m + 1) }
}

fn prev_month(y: i32, m: u32) -> (i32, u32) {
    if m == 1 { (y - 1, 12) } else { (y, m - 1) }
}

/// Return the number of civil dates in a month from adjacent `NaiveDate` values.
fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = next_month(y, m);
    let current = NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_default();
    let next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap_or(current);
    (next - current).num_days().max(28) as u32
}

/// Return one selected-zone month's absolute `[from, to)` range.
///
/// Args:
///     `(y, m)`: Civil year and month.
///     zone: Selected IANA display zone.
///
/// Returns:
///     UTC Unix-second bounds at consecutive civil month starts.
pub(super) fn month_range((y, m): (i32, u32), zone: chrono_tz::Tz) -> (i64, i64) {
    let (ny, nm) = next_month(y, m);
    (month_start(y, m, zone), month_start(ny, nm, zone))
}

/// Return the previous selected-zone month's absolute `[from, to)` range.
///
/// Args:
///     `(y, m)`: Current civil year and month.
///     zone: Selected IANA display zone.
///
/// Returns:
///     UTC Unix-second bounds for the preceding calendar month.
pub(super) fn prev_month_range((y, m): (i32, u32), zone: chrono_tz::Tz) -> (i64, i64) {
    month_range(prev_month(y, m), zone)
}

/// Return the whole-history `[-1, tomorrow)` range for Calendar Year mode.
///
/// Args:
///     zone: Selected IANA display zone.
///
/// Returns:
///     All-history sentinel and the next selected-zone midnight.
pub(super) fn all_history_range(zone: chrono_tz::Tz) -> (i64, i64) {
    (-1, super::Period::Today.range(zone).1)
}

/// How many day rows "Day" mode shows.
pub(super) const DAY_ROWS: i64 = 30;

/// Day window for "Day" mode: the selected day sits in the middle, but NEVER
/// higher — if the rows below it would run into the future, the window is
/// pinned so that its bottom is "today" (the selected day drops toward the
/// bottom row). Returns `(top, bottom)` day-starts whose forward traversal contains exactly
/// `DAY_ROWS` existing civil dates even when the zone skipped a complete calendar date.
///
/// Args:
///     sel: Selected day's absolute start instant.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Inclusive top and bottom civil-day start instants.
pub(super) fn day_window(sel: i64, zone: chrono_tz::Tz) -> (i64, i64) {
    let candidate =
        moon_core::util::display_time::shift_day_start(sel, DAY_ROWS / 2, zone).unwrap_or(sel);
    let bottom = candidate.min(today_start(zone));
    let top = moon_core::util::display_time::shift_day_start(bottom, -(DAY_ROWS - 1), zone)
        .unwrap_or(bottom);
    (top, bottom)
}

/// Return the previous existing civil-day start for Calendar comparisons.
///
/// Args:
///     day: Current existing civil-day start.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Previous existing day, or `day` at chrono limits.
pub(super) fn previous_day(day: i64, zone: chrono_tz::Tz) -> i64 {
    moon_core::util::display_time::shift_day_start(day, -1, zone).unwrap_or(day)
}

/// Resolve one displayed civil hour inside a calendar day.
///
/// Args:
///     day: UTC instant at the selected civil day's start.
///     hour: Displayed wall-clock hour from 0 through 23.
///     zone: IANA display zone selected by the header clock.
///
/// Returns:
///     The hour's first UTC instant, or `None` when DST skips that civil hour.
pub(super) fn hour_start(day: i64, hour: u32, zone: chrono_tz::Tz) -> Option<i64> {
    use chrono::{LocalResult, TimeZone as _};

    let local = date_of(day, zone).and_hms_opt(hour, 0, 0)?;
    match zone.from_local_datetime(&local) {
        LocalResult::Single(value) => Some(value.timestamp()),
        LocalResult::Ambiguous(first, second) => Some(first.min(second).timestamp()),
        LocalResult::None => None,
    }
}

/// Step from one displayed calendar day to the next without assuming 24 elapsed hours.
///
/// Args:
///     day: Current civil-day start as UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Next civil-day start, or the input when it cannot be resolved.
pub(super) fn next_day(day: i64, zone: chrono_tz::Tz) -> i64 {
    moon_core::util::display_time::next_bucket(day, 86_400, zone).unwrap_or(day)
}

/// Localized list parsed out of an "a,b,c" string.
pub(super) fn split_i18n(s: String) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).collect()
}

// ── Money and duration formatting ───────────────────────────────────────────

/// Format traded turnover: SI-compact, unsigned, unitless.
///
/// Turnover is large and always positive, so it reads as "1.5M" rather than as an exact figure.
/// The quote ticker is NOT appended: the navigation bar already carries the scope's quote badge,
/// and repeating it on every card would spend the width the numbers need.
pub(super) fn fmt_volume(v: f64) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    moon_core::util::fmt::compact_si(v)
}

/// Format an unsigned money amount the way profit is formatted, marking it approximate when it
/// does not cover every trade.
///
/// Deliberately NOT the SI-compact form the turnover uses: this figure stands in the same KPI row
/// as the profit, and a commission rendered "1.4K" beside a profit of "+560.28" reads as another
/// kind of number entirely. Same two decimals as profit; only the sign is dropped, because a cost
/// has no direction.
///
/// The tilde separates "this is the commission" from "this is the commission of the trades that
/// could be priced" — a liquidation has no recoverable one, and unmarked the shortfall would read
/// as exact.
///
/// Args:
///     v: Amount in the projection's currency.
///     complete: Whether every counted trade contributed.
///
/// Returns:
///     Two-decimal amount, prefixed with a tilde when incomplete.
pub(super) fn fmt_amount(v: f64, complete: bool) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    let s = moon_core::util::fmt::compact(v, 2);
    if complete { s } else { format!("~{s}") }
}

/// Format a short duration as at most two units: "45s", "1m 29s", "2h 15m", "3d 4h".
///
/// The card has one line for it, so the second unit is dropped when it is zero and never a third.
///
/// Args:
///     secs: Duration in seconds; negative and non-finite inputs yield an em dash.
///
/// Returns:
///     Localized compact duration.
pub(super) fn fmt_duration_short(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "—".to_string();
    }
    let total = secs.round() as i64;
    let (s, m, h, d) = (
        t!("analytics.cal.dur_s"),
        t!("analytics.cal.dur_m"),
        t!("analytics.cal.dur_h"),
        t!("analytics.cal.dur_d"),
    );
    // Each arm picks the largest unit that fits and one below it, so precision falls away with
    // scale instead of printing "3d 4h 12m 6s" into a cell that has room for eight characters.
    let pair = |big: i64, big_unit: &str, small: i64, small_unit: &str| {
        if small > 0 {
            format!("{big}{big_unit} {small}{small_unit}")
        } else {
            format!("{big}{big_unit}")
        }
    };
    match total {
        ..=59 => format!("{total}{s}"),
        60..=3_599 => pair(total / 60, &m, total % 60, &s),
        3_600..=86_399 => pair(total / 3_600, &h, total % 3_600 / 60, &m),
        _ => pair(total / 86_400, &d, total % 86_400 / 3_600, &h),
    }
}

impl AnalyticsView {
    /// Mode switch (+ persist) — the query range changes, so we refetch.
    ///
    /// Args:
    ///     m: Calendar mode to activate.
    ///     cx: GPUI context used to persist the mode and reload Calendar.
    fn set_cal_mode(&mut self, m: CalMode, cx: &mut Context<Self>) {
        if self.cal_mode == m {
            return;
        }
        self.cal_mode = m;
        self.backend.update(cx, |b, _| {
            let id = Some(m.id().to_string());
            if b.layout.analytics_heat_mode != id {
                b.layout.analytics_heat_mode = id;
                b.layout_dirty = true;
            }
        });
        self.reload_calendar(cx);
        cx.notify();
    }

    /// Prev/Next navigation: "Month" steps a month, "Day" steps a day, "Year"
    /// does not page at all (every year is already on screen).
    ///
    /// Args:
    ///     forward: Whether to move toward later dates.
    ///     cx: GPUI context used to reload Calendar after navigation.
    fn cal_shift(&mut self, forward: bool, cx: &mut Context<Self>) {
        match self.cal_mode {
            CalMode::Year => return,
            CalMode::Month => {
                let (cy, cm) = now_ym(self.bound_zone());
                let (y, m) = self.cal_ym;
                if forward && (y, m) >= (cy, cm) {
                    return; // no paging into the future — no trades there
                }
                self.cal_ym = if forward {
                    next_month(y, m)
                } else {
                    prev_month(y, m)
                };
            }
            CalMode::Day => {
                if forward && self.cal_day >= today_start(self.bound_zone()) {
                    return;
                }
                let date = date_of(self.cal_day, self.bound_zone());
                let shifted =
                    moon_core::util::display_time::shift_date(date, if forward { 1 } else { -1 });
                self.cal_day =
                    resolve_calendar_date(shifted, self.bound_zone(), if forward { 1 } else { -1 })
                        .unwrap_or(self.cal_day);
            }
        }
        self.reload_calendar(cx);
        cx.notify();
    }

    /// Jump to a specific month (click on a year cell).
    ///
    /// Args:
    ///     y: Target calendar year.
    ///     m: Target month in `1..=12`.
    ///     cx: GPUI context used to persist Month mode and reload Calendar.
    pub(super) fn cal_goto_month(&mut self, y: i32, m: u32, cx: &mut Context<Self>) {
        self.cal_ym = (y, m);
        self.cal_mode = CalMode::Month;
        self.persist_cal_mode(cx);
        self.reload_calendar(cx);
        cx.notify();
    }

    /// Jump to a specific day (click on a month cell).
    ///
    /// Args:
    ///     dsec: Target day start, in unix seconds.
    ///     cx: GPUI context used to persist Day mode and reload Calendar.
    fn cal_goto_day(&mut self, dsec: i64, cx: &mut Context<Self>) {
        self.cal_day = dsec;
        self.cal_mode = CalMode::Day;
        self.persist_cal_mode(cx);
        self.reload_calendar(cx);
        cx.notify();
    }

    fn persist_cal_mode(&mut self, cx: &mut Context<Self>) {
        let id = Some(self.cal_mode.id().to_string());
        self.backend.update(cx, |b, _| {
            if b.layout.analytics_heat_mode != id {
                b.layout.analytics_heat_mode = id;
                b.layout_dirty = true;
            }
        });
    }

    /// Render Calendar data or its classified current/previous-period read failure.
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///     cx: GPUI view context used for sizing and event listeners.
    ///
    /// Returns:
    ///     The complete Calendar tab element.
    pub(super) fn calendar_tab(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let days = match self.cal_days.view(|_| false) {
            Ok(days) => days.clone(),
            Err(note) => {
                return v_flex()
                    .size_full()
                    .child(self.cal_nav(p, cx))
                    .child(note_el("analytics-calendar-read", note, 18.0, p, cx))
                    .into_any_element();
            }
        };
        if self.cal_mode == CalMode::Month {
            if let Err(note) = self.cal_prev.view(|_| false) {
                return v_flex()
                    .size_full()
                    .child(self.cal_nav(p, cx))
                    .child(note_el("analytics-calendar-prev-read", note, 18.0, p, cx))
                    .into_any_element();
            }
        };
        // Frame: the nav bar (like Summary's period bar) + the mode's content.
        let content = match self.cal_mode {
            CalMode::Month => self.calendar_month(&days, p, cx),
            CalMode::Year => self.calendar_year(&days, p, cx),
            CalMode::Day => self.calendar_day(&days, p, cx),
        };
        v_flex()
            .size_full()
            .child(self.cal_nav(p, cx))
            .child(content)
            .into_any_element()
    }

    // ── Nav bar (like Summary's period bar: px10 py8) ────────────────────────

    /// Render Calendar mode, period navigation, exact quote badge, and title.
    ///
    /// The segmented control expresses one zoom choice, navigation keeps its disabled actions, and
    /// the badge/title pair remains shrinkable; a narrow host wraps only between those groups.
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Calendar navigation bar element.
    fn cal_nav(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let (y, m) = self.cal_ym;
        let months = split_i18n(t!("analytics.heat.months").to_string());
        let month_name = |mm: u32| {
            months
                .get((mm as usize).saturating_sub(1))
                .cloned()
                .unwrap_or_default()
        };
        let label = match self.cal_mode {
            CalMode::Month => format!("{} {}", month_name(m), y),
            CalMode::Year => t!("analytics.cal.all_years").to_string(),
            CalMode::Day => super::fmt_day(self.cal_day, self.bound_zone()),
        };
        // "Next" greys out: both in "Year", on the current/future one otherwise.
        let (cy, cm) = now_ym(self.bound_zone());
        let (prev_off, next_off) = match self.cal_mode {
            CalMode::Year => (true, true),
            CalMode::Month => (false, (y, m) >= (cy, cm)),
            CalMode::Day => (false, self.cal_day >= today_start(self.bound_zone())),
        };
        let nav_btn = |id: &'static str, label: String, forward: bool, off: bool| {
            MoonButton::new(id)
                .variant(MoonButtonVariant::Soft)
                .size(MoonButtonSize::Micro)
                .disabled(off)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.cal_shift(forward, cx)))
                .render()
        };
        // The control takes a plain indexed `Fn`, so capture the entity and update it through the
        // app context. `CalMode::ALL` supplies both display order and click lookup to keep them equal.
        let view = cx.entity();
        let modes = MoonSegmentedControl::new("cal-mode-presets")
            .items(CalMode::ALL.map(|mode| {
                // The tooltip carries the untruncated title, since `fit_width` elides the label.
                let title = mode.title();
                MoonSegmentItem::new("", title.clone())
                    .fit_width(cx, CAL_MODE_CELL_MIN_W, CAL_MODE_CELL_MAX_W)
                    .tooltip(title)
                    .selected(self.cal_mode == mode)
            }))
            .on_click(move |ix, _, _window, app| {
                let Some(mode) = CalMode::ALL.get(ix).copied() else {
                    return;
                };
                view.update(app, |this, cx| this.set_cal_mode(mode, cx));
            })
            .render();
        // Plain buttons preserve the navigation actions' independent disabled states. Keep the
        // divider inside their group so it cannot wrap onto a line by itself.
        let nav = design::chrome_section(cx)
            .child(design::chrome_divider(cx, p))
            .child(nav_btn(
                "cal-prev",
                t!("analytics.cal.prev").to_string(),
                false,
                prev_off,
            ))
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(label),
            )
            .child(nav_btn(
                "cal-next",
                t!("analytics.cal.next").to_string(),
                true,
                next_off,
            ));
        // One auto margin keeps badge and title together. Avoid `chrome_section`'s `flex_none` so
        // the title can give width back and truncate without pushing the strip wider.
        let trailing = h_flex()
            .items_center()
            .gap(design::ui_px(cx, design::CHROME_GAP))
            .ml_auto()
            .min_w_0()
            .children(self.cal_days.unit().and_then(|unit| {
                match unit {
                    ProfitUnit::Quote(currency) => Some(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(moon(p.table_head))
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_soft))
                            .child(currency.ticker()),
                    ),
                    ProfitUnit::Percent => None,
                }
            }))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(t!("analytics.cal.title").to_string()),
            );
        // Wrap only between groups; clipping a zoom or navigation control would remove a function.
        h_flex()
            .flex_none()
            .w_full()
            .min_h(design::fit_h_px(cx, 34.0, 13.0, 10.5))
            .flex_wrap()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .gap_x(design::ui_px(cx, design::CHROME_GAP))
            .gap_y(design::ui_px(cx, 4.0))
            .items_center()
            .border_b_1()
            .border_color(moon(p.border))
            .child(modes)
            .child(nav)
            .child(trailing)
    }
}

/// Recomputes for the "Calendar" tab (moved out of `analytics::mod` — page
/// logic lives next to its page). The query builds its own window from the
/// mode (month/year/day); the window's period bar plays no part here.
impl AnalyticsView {
    /// Calendar `[from,to)` range per mode plus the current core, side, emulator, strategy-name
    /// mask, metric, and valuation filters.
    fn cal_query(&self) -> Query {
        let (from, to) = match self.cal_mode {
            CalMode::Month => month_range(self.cal_ym, self.bound_zone()),
            CalMode::Year => all_history_range(self.bound_zone()),
            // "Day" loads the 30-day window, with the selected day centered or at the bottom.
            CalMode::Day => {
                let (top, bottom) = day_window(self.cal_day, self.bound_zone());
                let to =
                    moon_core::util::display_time::next_bucket(bottom, 86_400, self.bound_zone())
                        .unwrap_or(bottom);
                (top, to)
            }
        };
        Query {
            axis: self.report_axis(),
            previous_period_basis: PreviousPeriodBasis::Civil,
            from,
            to,
            cores: self.cores_selected(),
            side: self.side,
            emulator: self.emu,
            strategies: Vec::new(),
            strategy_name_mask: self.strategy_mask.clone(),
            metric: self.metric,
            valuation: self.valuation_mode,
            prefer_usdt: self.prefer_usdt,
        }
    }

    /// Previous month's query under the same filters, including the strategy-name mask, and
    /// valuation mode, for Month KPI deltas only.
    fn cal_query_prev(&self) -> Option<Query> {
        if self.cal_mode != CalMode::Month {
            return None;
        }
        let (from, to) = prev_month_range(self.cal_ym, self.bound_zone());
        Some(Query {
            axis: self.report_axis(),
            previous_period_basis: PreviousPeriodBasis::Civil,
            from,
            to,
            cores: self.cores_selected(),
            side: self.side,
            emulator: self.emu,
            strategies: Vec::new(),
            strategy_name_mask: self.strategy_mask.clone(),
            metric: self.metric,
            valuation: self.valuation_mode,
            prefer_usdt: self.prefer_usdt,
        })
    }

    /// Recompute the daily, or hourly in Day mode, series on the background executor.
    ///
    /// Args:
    ///     cx: GPUI context used to start and publish the calendar query.
    pub(super) fn reload_calendar(&mut self, cx: &mut Context<Self>) {
        self.reload_calendar_inner(false, true, cx);
    }

    /// Recompute Calendar and its visible report metadata through the serialized catch-up path.
    ///
    /// Args:
    ///     show_overlay: Whether queued user work requires blocking progress feedback.
    ///     cx: GPUI context used to run the serialized background reads.
    pub(super) fn reload_calendar_after_report(
        &mut self,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        self.reload_calendar_inner(true, show_overlay, cx);
    }

    /// Start the Calendar query with optional post-commit metadata chaining.
    ///
    /// Args:
    ///     after_report: Whether to preserve report-style catch-up and retry semantics, including
    ///         keeping the visible cells over a same-scope outcome that is transient with a
    ///         scheduled correction left (retry allowance or a newer generation,
    ///         `refresh::preserve_on_catch_up`) — every other outcome still publishes.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     cx: GPUI context used to execute and publish the reads.
    fn reload_calendar_inner(
        &mut self,
        after_report: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        if !after_report {
            self.report_busy_retries.reset();
        }
        self.acknowledge_report_refresh();
        self.cal_dirty = false;
        // Calendar quote identity can change with its period, so a MANUAL navigation drops the
        // old cells: they must not render beneath the new label while the scope is unverified.
        // A report-driven catch-up keeps the same period and scope, so its cells stay on screen
        // until the replacement lands instead of blinking through "loading".
        if !after_report {
            self.cal_days = ProfitLoadState::default();
            self.cal_prev = LoadState::default();
        }
        self.cal_seq = self.cal_seq.wrapping_add(1);
        let req = self.cal_seq;
        let report_req = self.current_report_generation();
        let q = self.cal_query();
        let q_prev = self.cal_query_prev();
        let has_previous = q_prev.is_some();
        // "Day" loads HOURLY cells (a 24×N grid); the other modes load daily.
        let hourly = self.cal_mode == CalMode::Day;
        let refresh_cores = self.core_metadata_due(cx);
        self.spawn_latest_db(
            &[super::bg::ReadLane::Calendar],
            show_overlay,
            cx,
            move || {
                moon_core::db::analytics::calendar_data(&q, q_prev.as_ref(), hourly, refresh_cores)
            },
            move |this, data, cx| {
                if this.cal_seq != req {
                    return; // mode/filters already changed
                }
                // Computed before `data.period` moves into the call below.
                let period_outcome = super::refresh::CatchUpOutcome::of_scope(&data.period);
                let cores_outcome = data
                    .cores
                    .as_ref()
                    .map(super::refresh::CatchUpOutcome::of_read);
                let undated_outcome = super::refresh::CatchUpOutcome::of_read(&data.undated);
                let transient = period_outcome.is_transient()
                    || cores_outcome.is_some_and(super::refresh::CatchUpOutcome::is_transient)
                    || undated_outcome.is_transient();
                let metadata_failed = data.cores.as_ref().is_some_and(|cores| cores.is_err())
                    || data.undated.is_err();
                // Peek at `data.period`'s outcome before it moves into the call below: only a
                // transient outcome with a scheduled correction left may keep the visible cells.
                let preserve_transient = !matches!(this.cal_days, ProfitLoadState::Loading)
                    && this.keep_on_catch_up(after_report, period_outcome, report_req);
                let read_failed = apply_calendar_results(
                    &mut this.cal_days,
                    &mut this.cal_prev,
                    data.period,
                    has_previous,
                    preserve_transient,
                );
                if let Some(Ok(cores)) = data.cores {
                    this.cores = cores;
                    this.last_cores_at = Some(std::time::Instant::now());
                    this.core_refresh_needed = false;
                }
                this.apply_undated_result(data.undated, after_report, report_req);
                this.cal_dirty = super::refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    read_failed || metadata_failed,
                );
                this.settle_report_refresh_retry(transient, cx);
                cx.notify();
            },
        );
    }
}

#[cfg(test)]
mod tests;
