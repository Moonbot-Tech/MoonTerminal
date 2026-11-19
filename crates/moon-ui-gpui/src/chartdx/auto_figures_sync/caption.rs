//! The words on an auto line: its caption, and the pieces the hover card shows.
//!
//! Every string here is built on a REVISION — a rebuild of the pane's auto lines, a price change
//! that moved a printed value, a tick of the age clock, a locale switch — and never on a frame:
//! the text pass draws what [`super::pane::refresh_auto_captions`] left in the line records. What
//! decides "moved" is [`caption_quantum`], which reduces each printed value to the number its text
//! is made from, so a price tick that changes no digit on screen builds no string.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use moon_chart::auto_figures::LineTag;
use moon_chart::auto_figures::events::EventSet;
use moon_core::config::auto_figures::{AutoEvent, AutoFigureRow, CaptionPart, DetectorKind, TfId};
use rust_i18n::t;

/// Separator between the parts of one caption. Not a dictionary entry: it is punctuation, the
/// same in every language, like the middle dot the figure readouts already join with.
const PART_SEP: &str = " · ";

/// Separator between two event names on one line.
const EVENT_SEP: &str = ", ";

/// What a row's lines are called.
///
/// Resolved to words only when a caption or the card is built, so a locale switch renames every
/// line on the next caption refresh without the row being published again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RowTitle {
    /// The user's own name for the row, verbatim.
    Named(Arc<str>),
    /// A dictionary key: the row's preset, or its detector kind when it has neither name nor
    /// preset.
    Key(&'static str),
}

impl RowTitle {
    /// The title `row` goes by; `kind` names a row with neither a name nor a preset.
    pub(crate) fn of(row: &AutoFigureRow, kind: DetectorKind) -> Self {
        if !row.name.is_empty() {
            return RowTitle::Named(row.name.as_str().into());
        }
        RowTitle::Key(row.title_key().unwrap_or_else(|| kind_key(kind)))
    }

    /// The title in the current locale.
    pub(crate) fn text(&self) -> String {
        match self {
            RowTitle::Named(name) => name.to_string(),
            RowTitle::Key(key) => t!(*key).to_string(),
        }
    }
}

/// Dictionary key of a detector kind's name — the ONE vocabulary the popup, the row editor, the
/// line captions and the hover card all print.
pub(crate) fn kind_key(kind: DetectorKind) -> &'static str {
    match kind {
        DetectorKind::Swing => "auto_figures.kind.swing",
        DetectorKind::PeriodExtreme => "auto_figures.kind.period_extreme",
        DetectorKind::Trend => "auto_figures.kind.trend",
        DetectorKind::Squeeze => "auto_figures.kind.squeeze",
        DetectorKind::VolumeProfile => "auto_figures.kind.volume_profile",
        DetectorKind::PeriodLevel => "auto_figures.kind.period_level",
        DetectorKind::RoundLevels => "auto_figures.kind.round_levels",
    }
}

/// Dictionary key of a timeframe's short label, shared like [`kind_key`].
pub(crate) fn tf_key(tf: TfId) -> &'static str {
    match tf {
        TfId::M1 => "auto_figures.tf.m1",
        TfId::M5 => "auto_figures.tf.m5",
        TfId::M15 => "auto_figures.tf.m15",
        TfId::M30 => "auto_figures.tf.m30",
        TfId::H1 => "auto_figures.tf.h1",
        TfId::H4 => "auto_figures.tf.h4",
        TfId::D1 => "auto_figures.tf.d1",
    }
}

/// Dictionary key of an event's name, shared like [`kind_key`]: the editor's checkbox for a mark
/// and the mark's name on the line say the same word.
pub(crate) fn event_key(event: AutoEvent) -> &'static str {
    match event {
        AutoEvent::Approach => "auto_figures.event.approach",
        AutoEvent::Squeeze => "auto_figures.event.squeeze",
        AutoEvent::Breakout => "auto_figures.event.breakout",
        AutoEvent::FalseBreak => "auto_figures.event.false_break",
        AutoEvent::Bounce => "auto_figures.event.bounce",
        AutoEvent::RoleFlip => "auto_figures.event.role_flip",
        AutoEvent::StopHunt => "auto_figures.event.stop_hunt",
        AutoEvent::Confluence => "auto_figures.event.confluence",
    }
}

/// Dictionary key of what a tagged line is (`moon_chart::auto_figures::LineTag`), shared like
/// [`kind_key`]: the caption, the hover card and the row editor's checkboxes for the same line say
/// the same word. POC, VAH and VAL are the market's own abbreviations and stay Latin in every
/// language (`locales/README.md`).
pub(crate) fn tag_key(tag: LineTag) -> &'static str {
    match tag {
        LineTag::High => "auto_figures.tag.high",
        LineTag::Low => "auto_figures.tag.low",
        LineTag::Poc => "auto_figures.tag.poc",
        LineTag::Vah => "auto_figures.tag.vah",
        LineTag::Val => "auto_figures.tag.val",
        LineTag::NakedPoc => "auto_figures.tag.naked_poc",
        LineTag::DayOpen => "auto_figures.tag.day_open",
        LineTag::PrevDayClose => "auto_figures.tag.prev_day_close",
        LineTag::WeekOpen => "auto_figures.tag.week_open",
        LineTag::PrevWeekClose => "auto_figures.tag.prev_week_close",
    }
}

/// A line's name: its row's title, then what the line is when its detector says ("POC", "Week
/// open"), so two lines of one row standing side by side are told apart.
pub(crate) fn line_title(title: &RowTitle, tag: Option<LineTag>) -> String {
    let mut out = title.text();
    if let Some(tag) = tag {
        if !out.is_empty() {
            out.push_str(PART_SEP);
        }
        out.push_str(&t!(tag_key(tag)));
    }
    out
}

/// Whether a line has a moment it was born at: a round price has none (`first_ms` is `0`), and an
/// age measured from the epoch would print fifty years.
pub(crate) fn has_anchor(first_ms: f64) -> bool {
    first_ms.is_finite() && first_ms > 0.0
}

/// The events of a line in display order, joined; empty when it shows none.
pub(crate) fn events_text(events: EventSet) -> String {
    let mut out = String::new();
    for event in events.iter() {
        if !out.is_empty() {
            out.push_str(EVENT_SEP);
        }
        out.push_str(&t!(event_key(event)));
    }
    out
}

/// The unit an age is printed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AgeUnit {
    Minute,
    Hour,
    Day,
}

/// An age as the whole count of its largest unit: `3` days, `5` hours, `40` minutes. What the
/// caption prints, and therefore what has to move before it is printed again — once a minute at
/// most, once a day for an old level.
pub(crate) fn age_bucket(age_ms: f64) -> (i64, AgeUnit) {
    const MIN: f64 = 60_000.0;
    let age = if age_ms.is_finite() {
        age_ms.max(0.0)
    } else {
        0.0
    };
    if age >= 1_440.0 * MIN {
        ((age / (1_440.0 * MIN)) as i64, AgeUnit::Day)
    } else if age >= 60.0 * MIN {
        ((age / (60.0 * MIN)) as i64, AgeUnit::Hour)
    } else {
        ((age / MIN) as i64, AgeUnit::Minute)
    }
}

/// An age in the chart's own unit spelling (`3д`, `5h`, `40m`).
pub(crate) fn age_text(age_ms: f64) -> String {
    let (n, unit) = age_bucket(age_ms);
    let unit = match unit {
        AgeUnit::Minute => t!("chart_labels.unit_minute"),
        AgeUnit::Hour => t!("chart_labels.unit_hour"),
        AgeUnit::Day => t!("chart_labels.unit_day"),
    };
    format!("{n}{unit}")
}

/// The touch count as the caption prints it.
pub(crate) fn touches_text(n: usize) -> String {
    t!("auto_figures_chart.caption.touches", n = n).to_string()
}

/// Everything one caption is made from, apart from its configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptionValues<'a> {
    pub title: &'a RowTitle,
    /// What the line is, for detectors that draw several kinds of line; printed with the name.
    pub tag: Option<LineTag>,
    /// The line's price at `now_ms`, when it has one.
    pub line_now: Option<f64>,
    /// The pane's current price.
    pub price: Option<f64>,
    pub touches: usize,
    pub tf: TfId,
    /// When the line was born: its first anchor, Unix ms.
    pub first_ms: f64,
    pub events: EventSet,
    pub now_ms: f64,
    /// The market's price tick, for the axis's own price precision.
    pub tick: Option<f64>,
}

/// Signed distance from the current price to the line, in % of the price. `None` without both.
fn distance_pct(values: &CaptionValues<'_>) -> Option<f64> {
    let (line, price) = (values.line_now?, values.price?);
    (price.is_finite() && price > 0.0 && line.is_finite()).then(|| (line - price) / price * 100.0)
}

/// The caption of one line: `parts` in their configured order, each that has something to say,
/// joined. Empty when no part does.
///
/// A part with nothing true to say is left out rather than printed as a fact: the touch count of a
/// line nothing touched (a volume-profile line, a round price) and the age of a line with no
/// moment of birth (a round price).
pub(crate) fn caption_text(parts: &[CaptionPart], values: &CaptionValues<'_>) -> String {
    let mut out = String::new();
    for part in parts {
        let piece = match part {
            CaptionPart::RowName => line_title(values.title, values.tag),
            CaptionPart::Price => match values.line_now {
                Some(p) => moon_chart::axes::format_price(p, values.tick),
                None => continue,
            },
            CaptionPart::DistancePct => match distance_pct(values) {
                Some(d) => crate::chartdx::text::fmt_pct(d as f32),
                None => continue,
            },
            CaptionPart::Touches if values.touches == 0 => continue,
            CaptionPart::Touches => touches_text(values.touches),
            CaptionPart::Tf => t!(tf_key(values.tf)).to_string(),
            CaptionPart::Age if !has_anchor(values.first_ms) => continue,
            CaptionPart::Age => age_text(values.now_ms - values.first_ms),
            CaptionPart::Event => {
                let text = events_text(values.events);
                if text.is_empty() {
                    continue;
                }
                text
            }
        };
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(PART_SEP);
        }
        out.push_str(&piece);
    }
    out
}

/// The printed price as an integer: `price` scaled by `10^d`, where `d` is the number of decimals
/// `format_price` prints it with for this tick. Equal quanta print equal text; `None` without a
/// price. The decimals depend on the price only before a tick is known, and then they are part of
/// the quantum too.
pub(crate) fn price_quantum(price: Option<f64>, tick: Option<f64>) -> Option<(usize, i64)> {
    let price = price.filter(|p| p.is_finite())?;
    let decimals = moon_chart::axes::format_price_decimals(price, tick);
    let scale = 10f64.powi(decimals.min(18) as i32);
    Some((decimals, (price * scale).round() as i64))
}

/// Folds into `h` the numbers `caption_text` would print for these `parts`: equal quanta mean an
/// equal caption, so the caller rebuilds the string only when this moved.
///
/// The price is quantized at the precision it is PRINTED with ([`price_quantum`]), the distance
/// to the hundredth of a percent `fmt_pct` prints, the age to its bucket — and only for a line
/// with a moment of birth ([`has_anchor`]): `caption_text` prints no age for one without (a round
/// price), and a bucket of the time since the epoch would rebuild that caption every minute for
/// nothing.
/// The name, touches, timeframe and events only change with a rebuild of the lines, which resets
/// the caption anyway, but they are folded in too, so the function is the whole truth on its own.
pub(crate) fn caption_quantum(
    parts: &[CaptionPart],
    values: &CaptionValues<'_>,
    h: &mut impl Hasher,
) {
    for part in parts {
        match part {
            CaptionPart::RowName => (values.title, values.tag).hash(h),
            CaptionPart::Price => price_quantum(values.line_now, values.tick).hash(h),
            CaptionPart::DistancePct => {
                distance_pct(values)
                    .map(|d| (d * 100.0).round() as i64)
                    .hash(h);
            }
            CaptionPart::Touches => values.touches.hash(h),
            CaptionPart::Tf => values.tf.hash(h),
            CaptionPart::Age => has_anchor(values.first_ms)
                .then(|| age_bucket(values.now_ms - values.first_ms))
                .hash(h),
            CaptionPart::Event => values.events.hash(h),
        }
    }
}
