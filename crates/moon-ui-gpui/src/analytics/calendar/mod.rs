//! "Profit calendar" tab of the "Analytics" window.
//! Three modes (toggle buttons, like the Summary period presets):
//! - "Month" — large day cards (date on the left, PnL + trades + W/L + winrate
//!   on the right), grey background + red/green overlay whose alpha tracks
//!   |PnL|; a KPI row on top (with the delta to the previous month), a
//!   plus/minus-day bar below. Clicking a day switches to "Day" mode;
//! - "Year" — every year at once, each one a grid of 12 month squares (current
//!   year on top, future months greyed out). Clicking a month switches to
//!   "Month" mode. See `year`;
//! - "Day" — hour-by-hour detail of a single day. See `day`.
//! Each mode builds its own query (`cal_query` in mod.rs); the window's period
//! bar is hidden here.

mod day;
mod month;
mod year;

use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use gpui::*;
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::AnalyticsView;
use crate::design;
use crate::design::moon;
use moon_core::db::analytics::Query;

/// Calendar mode (the tab's toggle buttons).
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

// ── Calendar date helpers (UTC) ─────────────────────────────────────────────

/// unix seconds (UTC midnight) → date (thin wrapper over the window's helper).
pub(super) fn date_of(secs: i64) -> NaiveDate {
    super::day_of_secs(secs).unwrap_or_default()
}

/// Current (real) month — the boundary "Next" refuses to page past.
pub(super) fn now_ym() -> (i32, u32) {
    let d = date_of(moon_core::util::now_unix_ms_i64() / 1000);
    (d.year(), d.month())
}

/// Midnight of the current day (UTC) — the grid's "future" day boundary.
pub(super) fn today_start() -> i64 {
    (moon_core::util::now_unix_ms_i64() / 1000).div_euclid(86_400) * 86_400
}

/// Midnight of the 1st of the month (unix seconds, UTC).
pub(super) fn month_start(y: i32, m: u32) -> i64 {
    super::secs_of_day(NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_default())
}

pub(super) fn next_month(y: i32, m: u32) -> (i32, u32) {
    if m == 12 { (y + 1, 1) } else { (y, m + 1) }
}

fn prev_month(y: i32, m: u32) -> (i32, u32) {
    if m == 1 { (y - 1, 12) } else { (y, m - 1) }
}

/// Days in a month (difference between the two 1st-of-month midnights).
fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = next_month(y, m);
    ((month_start(ny, nm) - month_start(y, m)) / 86_400).max(28) as u32
}

/// The month's `[from, to)` range — for `cal_query` (mod.rs).
pub(super) fn month_range((y, m): (i32, u32)) -> (i64, i64) {
    let (ny, nm) = next_month(y, m);
    (month_start(y, m), month_start(ny, nm))
}

/// The PREVIOUS month's `[from, to)` range — for the KPI deltas (mod.rs).
pub(super) fn prev_month_range((y, m): (i32, u32)) -> (i64, i64) {
    month_range(prev_month(y, m))
}

/// The whole history `[-1, tomorrow)` — "Year" mode shows every year at once.
pub(super) fn all_history_range() -> (i64, i64) {
    (-1, today_start() + 86_400)
}

/// How many day rows "Day" mode shows.
pub(super) const DAY_ROWS: i64 = 30;

/// Day window for "Day" mode: the selected day sits in the middle, but NEVER
/// higher — if the rows below it would run into the future, the window is
/// pinned so that its bottom is "today" (the selected day drops toward the
/// bottom row). Returns `(top, bottom)` day-starts; exactly `DAY_ROWS` rows,
/// `bottom = top + (DAY_ROWS - 1) days`.
pub(super) fn day_window(sel: i64) -> (i64, i64) {
    let bottom = (sel + (DAY_ROWS / 2) * 86_400).min(today_start());
    (bottom - (DAY_ROWS - 1) * 86_400, bottom)
}

/// Localized list parsed out of an "a,b,c" string.
pub(super) fn split_i18n(s: String) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).collect()
}

impl AnalyticsView {
    /// Mode switch (+ persist) — the query range changes, so we refetch.
    fn set_cal_mode(&mut self, m: CalMode, cx: &mut Context<Self>) {
        if self.cal_mode == m {
            return;
        }
        self.cal_mode = m;
        self.cal_hover = None;
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
    fn cal_shift(&mut self, forward: bool, cx: &mut Context<Self>) {
        match self.cal_mode {
            CalMode::Year => return,
            CalMode::Month => {
                let (cy, cm) = now_ym();
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
                if forward && self.cal_day >= today_start() {
                    return;
                }
                self.cal_day += if forward { 86_400 } else { -86_400 };
            }
        }
        self.cal_hover = None;
        self.reload_calendar(cx);
        cx.notify();
    }

    /// Jump to a specific month (click on a year cell).
    pub(super) fn cal_goto_month(&mut self, y: i32, m: u32, cx: &mut Context<Self>) {
        self.cal_ym = (y, m);
        self.cal_mode = CalMode::Month;
        self.cal_hover = None;
        self.persist_cal_mode(cx);
        self.reload_calendar(cx);
        cx.notify();
    }

    /// Jump to a specific day (click on a month cell).
    fn cal_goto_day(&mut self, dsec: i64, cx: &mut Context<Self>) {
        self.cal_day = dsec;
        self.cal_mode = CalMode::Day;
        self.cal_hover = None;
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

    /// Cell hover listener: keeps `cal_hover` at the day under the cursor.
    fn cell_hover(
        &self,
        secs: i64,
        cx: &Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        cx.listener(move |this, hov: &bool, _, cx| {
            if *hov {
                if this.cal_hover != Some(secs) {
                    this.cal_hover = Some(secs);
                    cx.notify();
                }
            } else if this.cal_hover == Some(secs) {
                this.cal_hover = None;
                cx.notify();
            }
        })
    }

    pub(super) fn calendar_tab(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let Some(days) = self.cal_days.clone() else {
            return v_flex()
                .size_full()
                .child(self.cal_nav(p, cx))
                .child(
                    div()
                        .p(design::ui_px(cx, 18.0))
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.loading").to_string()),
                )
                .into_any_element();
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
            CalMode::Day => super::fmt_day(self.cal_day),
        };
        // "Next" greys out: both in "Year", on the current/future one otherwise.
        let (cy, cm) = now_ym();
        let (prev_off, next_off) = match self.cal_mode {
            CalMode::Year => (true, true),
            CalMode::Month => (false, (y, m) >= (cy, cm)),
            CalMode::Day => (false, self.cal_day >= today_start()),
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
        let mode_btn = |mode: CalMode| {
            let on = self.cal_mode == mode;
            MoonButton::new(SharedString::from(format!("cm-{}", mode.id())))
                .variant(if on {
                    MoonButtonVariant::Amber
                } else {
                    MoonButtonVariant::Soft
                })
                .size(MoonButtonSize::Micro)
                .selected(on)
                .label(mode.title())
                .on_click(cx.listener(move |this, _, _, cx| this.set_cal_mode(mode, cx)))
                .render()
        };
        // Left: Year · Month · Day · Prev · <period> · Next; title on the right.
        h_flex()
            .flex_none()
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .child(mode_btn(CalMode::Year))
            .child(mode_btn(CalMode::Month))
            .child(mode_btn(CalMode::Day))
            .child(div().w(design::ui_px(cx, 8.0)))
            .child(nav_btn(
                "cal-prev",
                t!("analytics.cal.prev").to_string(),
                false,
                prev_off,
            ))
            .child(
                div()
                    .px(design::ui_px(cx, 8.0))
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
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(t!("analytics.cal.title").to_string()),
            )
    }
}

/// Recomputes for the "Calendar" tab (moved out of `analytics::mod` — page
/// logic lives next to its page). The query builds its own window from the
/// mode (month/year/day); the window's period bar plays no part here.
impl AnalyticsView {
    /// Calendar `[from,to)` range per mode + the current core/side/emu filters.
    fn cal_query(&self) -> Query {
        let (from, to) = match self.cal_mode {
            CalMode::Month => month_range(self.cal_ym),
            CalMode::Year => all_history_range(),
            // "Day" loads a 7-day window (selected day centered/at the bottom).
            CalMode::Day => {
                let (top, bottom) = day_window(self.cal_day);
                (top, bottom + 86_400)
            }
        };
        Query {
            from,
            to,
            cores: self.cores_selected(),
            side: self.side,
            emulator: self.emu,
            strategy: None,
            strat_core: None,
        }
    }

    /// PREVIOUS month's query (for the KPI deltas) — "Month" mode only.
    fn cal_query_prev(&self) -> Option<Query> {
        if self.cal_mode != CalMode::Month {
            return None;
        }
        let (from, to) = prev_month_range(self.cal_ym);
        Some(Query {
            from,
            to,
            cores: self.cores_selected(),
            side: self.side,
            emulator: self.emu,
            strategy: None,
            strat_core: None,
        })
    }

    /// Background recompute of the daily (or hourly, in "Day" mode) series.
    pub(super) fn reload_calendar(&mut self, cx: &mut Context<Self>) {
        self.cal_dirty = false;
        // The hovered day from the old series is no longer under the cursor —
        // clear the stuck highlight (the new series may not contain that day).
        self.cal_hover = None;
        self.cal_seq = self.cal_seq.wrapping_add(1);
        let req = self.cal_seq;
        let q = self.cal_query();
        let q_prev = self.cal_query_prev();
        // "Day" loads HOURLY cells (a 24×N grid); the other modes load daily.
        let hourly = self.cal_mode == CalMode::Day;
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let data = executor
                .spawn(async move {
                    let cur = if hourly {
                        moon_core::db::analytics::calendar_hours(&q)
                    } else {
                        moon_core::db::analytics::calendar_cells(&q)
                    };
                    // Previous month's aggregate (profit, trades, wins) for the KPI deltas.
                    let prev = q_prev
                        .and_then(|qp| moon_core::db::analytics::calendar_cells(&qp))
                        .map(|d| {
                            d.iter().fold((0.0f64, 0i64, 0i64), |a, c| {
                                (a.0 + c.profit, a.1 + c.trades, a.2 + c.wins)
                            })
                        });
                    (cur, prev)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.cal_seq != req {
                        return; // mode/filters already changed
                    }
                    let (cur, prev) = data;
                    this.cal_days = cur.map(Arc::new);
                    this.cal_prev = prev;
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
