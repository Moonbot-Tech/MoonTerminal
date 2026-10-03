//! The inline calendar of the custom-period screen: one month, Monday first.
//!
//! The first press picks the first day (the screen comes back with it marked); the second picks
//! the last and opens the report. A day after today, or more than a year after the first day, is
//! out of reach and does nothing; a day before the first one starts the pick again from there.

use chrono::{Datelike, Days, Months, NaiveDate};
use moon_core::config::telegram_menu::ReportView;
use moon_core::telegram::api::InlineKeyboardButton;
use moon_core::telegram::menu_action::MenuAction;
use moon_core::telegram::report::{MAX_SPAN_DAYS, ReportRequest};
use rust_i18n::t;

/// A cell that shows nothing; Telegram refuses an empty button text.
const BLANK: &str = "\u{2800}";
/// A day out of reach.
const OUT: &str = "\u{00b7}";

/// The calendar rows: the month bar, the weekday captions, and the weeks.
///
/// Args:
///     month: The month to show; the first day's month, else today's, when absent.
///     from: The first day, once picked.
///     today: Today in the report zone; nothing after it can be picked or browsed to.
///     view: The view the report opens in.
pub(super) fn rows(
    month: Option<NaiveDate>,
    from: Option<NaiveDate>,
    today: NaiveDate,
    view: ReportView,
) -> Vec<Vec<InlineKeyboardButton>> {
    let this_month = first_of(today);
    let month = first_of(month.or(from).unwrap_or(today)).min(this_month);
    let noop = || MenuAction::Noop.callback();
    let browse = |month: NaiveDate| MenuAction::Custom {
        month: Some(month),
        from,
    };
    let previous = month
        .checked_sub_months(Months::new(1))
        .filter(|m| m.year() >= 1970);
    let next = month
        .checked_add_months(Months::new(1))
        .filter(|m| *m <= this_month);
    let arrow = |glyph: &str, target: Option<NaiveDate>| match target {
        Some(target) => InlineKeyboardButton::callback(glyph, browse(target).callback()),
        None => InlineKeyboardButton::callback(BLANK, noop()),
    };
    let mut rows = vec![vec![
        arrow("\u{25c0}", previous),
        InlineKeyboardButton::callback(month_caption(month), noop()),
        arrow("\u{25b6}", next),
    ]];
    rows.push(
        t!("telegram.menu.weekdays")
            .split_whitespace()
            .take(7)
            .map(|day| InlineKeyboardButton::callback(day, noop()))
            .collect(),
    );
    let lead = month.weekday().num_days_from_monday() as usize;
    let mut week: Vec<InlineKeyboardButton> = (0..lead)
        .map(|_| InlineKeyboardButton::callback(BLANK, noop()))
        .collect();
    let mut day = month;
    while day.month() == month.month() {
        week.push(day_cell(day, from, today, view));
        if week.len() == 7 {
            rows.push(std::mem::take(&mut week));
        }
        let Some(after) = day.checked_add_days(Days::new(1)) else {
            break;
        };
        day = after;
    }
    if !week.is_empty() {
        week.resize_with(7, || InlineKeyboardButton::callback(BLANK, noop()));
        rows.push(week);
    }
    rows
}

/// One day: picks the first day, picks the last and opens the report, or does nothing.
fn day_cell(
    day: NaiveDate,
    from: Option<NaiveDate>,
    today: NaiveDate,
    view: ReportView,
) -> InlineKeyboardButton {
    let number = day.day().to_string();
    let noop = || InlineKeyboardButton::callback(OUT, MenuAction::Noop.callback());
    if day > today {
        return noop();
    }
    let pick_first = || {
        InlineKeyboardButton::callback(
            number.clone(),
            MenuAction::Custom {
                month: Some(first_of(day)),
                from: Some(day),
            }
            .callback(),
        )
    };
    let Some(from) = from.filter(|from| *from <= day) else {
        return pick_first();
    };
    if (day - from).num_days() >= MAX_SPAN_DAYS {
        return noop();
    }
    let Some(request) = ReportRequest::span(from, day) else {
        return noop();
    };
    let text = if day == from {
        format!("\u{2022}{number}")
    } else {
        number
    };
    InlineKeyboardButton::callback(text, request.in_view(view).callback())
}

/// The first day of `day`'s month.
fn first_of(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap_or(day)
}

/// "September 2026" in the chat's language.
fn month_caption(month: NaiveDate) -> String {
    let names = t!("telegram.menu.months");
    let name = names
        .split_whitespace()
        .nth(month.month0() as usize)
        .unwrap_or_default()
        .to_string();
    format!("{name} {}", month.year())
}
