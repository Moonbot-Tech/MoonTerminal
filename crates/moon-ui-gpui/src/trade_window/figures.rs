//! The trade's own numbers, stated beside the chart.
//!
//! This rail is populated the instant the window opens and never waits for the network: every
//! figure comes from the report row that was clicked. That is what makes the window useful even
//! when the venue cannot be reached at all — the picture may be missing, the trade never is.

use gpui::*;
use moon_core::db::ChartTradeRecord;
use moon_core::util::fmt;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use crate::design;
use crate::design::moon;

/// Width of the side rail, in logical pixels before the font scale.
///
/// This is a PIXEL width, which is what [`design::font_w_px`] takes — it returns font-scaled
/// pixels, never a character count. The number is the widest cell the rail actually draws: the
/// widest localized caption at caption size over a monospace amount like `-1234.56 USDT` at
/// body size, plus the rail's own `px(10)` padding on each side. Named rather than quoted on
/// purpose: which caption is widest depends on the active language. It is deliberately generous
/// rather than exact, because a cell that does not fit does not clip here — it WRAPS, and a rail
/// one character wide is what a `22.0` read as "22 characters" produced.
const RAIL_W: f32 = 200.0;

/// Seconds in a minute, an hour and a day, for the duration reading.
const MINUTE_S: i64 = 60;
const HOUR_S: i64 = 60 * MINUTE_S;
const DAY_S: i64 = 24 * HOUR_S;

/// Render a held duration the way a trader reads one.
///
/// Coarse-first and at most two units: `4m 12s`, `3h 05m`, `2d 07h`. A position's duration is
/// scanned, not calculated from, so a bare second count would be worse than useless at the day
/// scale and three units would be noise at every scale.
///
/// Args:
///     seconds: Held duration; a non-positive value has no reading.
///
/// Returns:
///     ASCII duration, or a dash when there is nothing to state.
pub(super) fn format_duration_s(seconds: i64) -> String {
    if seconds <= 0 {
        return "-".to_string();
    }
    if seconds >= DAY_S {
        return format!("{}d {:02}h", seconds / DAY_S, (seconds % DAY_S) / HOUR_S);
    }
    if seconds >= HOUR_S {
        return format!(
            "{}h {:02}m",
            seconds / HOUR_S,
            (seconds % HOUR_S) / MINUTE_S
        );
    }
    if seconds >= MINUTE_S {
        return format!("{}m {:02}s", seconds / MINUTE_S, seconds % MINUTE_S);
    }
    format!("{seconds}s")
}

/// One label-over-value block of the rail.
///
/// Args:
///     id: Stable element id.
///     label: Already-localized caption.
///     value: Already-formatted value.
///     tone: Colour for the value; the label is always muted.
///     p: Active palette.
///     cx: Render context, for scaled type.
///
/// Returns:
///     The block.
fn cell(
    id: &'static str,
    label: String,
    value: String,
    tone: Hsla,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .id(id)
        .gap(design::ui_px(cx, 1.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(label),
        )
        .child(
            div()
                .text_size(design::t_body(cx))
                .font_family(design::mono())
                .text_color(tone)
                .child(value),
        )
}

/// Render the figures rail for one trade.
///
/// Rebuilt on every render rather than cached, deliberately. The values are fixed for the
/// window's life, but their COLOURS come from the live palette and their captions from the live
/// dictionary, so a cached rail would keep the old theme's tones after a theme switch and the old
/// language's words after a locale switch. This view repaints on state changes, resize and hover
/// — never at chart frame rate, which the chart's own GPU pass deliberately keeps off the shell —
/// so nine dictionary lookups is not a cost worth buying two invalidation bugs with.
///
/// `narrow` decides the axis rather than a second layout: below the window's own threshold the
/// same blocks wrap into a horizontal strip under the header, which is the defined narrow
/// behaviour a panel owes — a horizontal scrollbar is not one.
///
/// Args:
///     record: The clicked trade.
///     zone_label: Already-formatted entry and exit stamps, in the Report's own clock. Borrowed
///         rather than owned: they are fixed for the window's life, so a caller has no reason to
///         hand over a copy per render.
///     narrow: Whether to lay the blocks out horizontally.
///     p: Active palette.
///     cx: Render context.
///
/// Returns:
///     The rail.
pub(super) fn rail(
    record: &ChartTradeRecord,
    zone_label: &(String, String),
    narrow: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let side_key = match record.is_short {
        true => "trade_window.side.short",
        false => "trade_window.side.long",
    };
    let side_tone = match record.is_short {
        true => moon(p.red_text),
        false => moon(p.green_text),
    };
    // A quote's own ticker and decimal count are already decided by the quote identity; inventing
    // a "USD family" test here would be a second opinion about the same thing.
    let ticker = record
        .quote
        .map(|q| q.ticker().to_string())
        .unwrap_or_default();
    let unknown = t!("trade_window.figure.unknown").to_string();
    // The TONE comes from the sign these helpers return, never from the raw value. They classify
    // the ROUNDED figure on purpose — that is the whole reason they answer with a tuple — so
    // re-deriving the colour from the unrounded input is exactly the disagreement `DeltaSign`
    // exists to prevent: a loss of -0.001 prints as `0.00` and would have been painted red.
    let (profit_text, profit_tone) = match record.profit {
        Some(value) => {
            let (text, sign) = fmt::signed_amount(value, 2);
            let tone = sign.pick(moon(p.green_text), moon(p.red_text), moon(p.text));
            (format!("{text} {ticker}").trim_end().to_string(), tone)
        }
        // A missing profit is UNKNOWN, never a zero: printing 0 would state a break-even trade
        // that nothing in the row supports.
        None => (unknown.clone(), moon(p.text_muted)),
    };
    let (pct_text, pct_tone) = match record.profit_pct.and_then(|v| fmt::signed_pct(v, 2)) {
        Some((text, sign)) => (
            text,
            sign.pick(moon(p.green_text), moon(p.red_text), moon(p.text)),
        ),
        None => (unknown.clone(), moon(p.text_muted)),
    };
    let cells = vec![
        cell(
            "tw-side",
            t!("trade_window.figure.side").to_string(),
            t!(side_key).to_string(),
            side_tone,
            p,
            cx,
        )
        .into_any_element(),
        cell(
            "tw-entry",
            t!("trade_window.figure.entry").to_string(),
            fmt::adaptive(record.buy_price),
            moon(p.text),
            p,
            cx,
        )
        .into_any_element(),
        cell(
            "tw-exit",
            t!("trade_window.figure.exit").to_string(),
            fmt::adaptive(record.sell_price),
            moon(p.text),
            p,
            cx,
        )
        .into_any_element(),
        cell(
            "tw-qty",
            t!("trade_window.figure.quantity").to_string(),
            fmt::qty(record.quantity),
            moon(p.text),
            p,
            cx,
        )
        .into_any_element(),
        cell(
            "tw-profit",
            t!("trade_window.figure.profit").to_string(),
            profit_text,
            profit_tone,
            p,
            cx,
        )
        .into_any_element(),
        cell(
            "tw-profit-pct",
            t!("trade_window.figure.profit_pct").to_string(),
            pct_text,
            pct_tone,
            p,
            cx,
        )
        .into_any_element(),
        cell(
            "tw-opened",
            t!("trade_window.figure.opened").to_string(),
            zone_label.0.clone(),
            moon(p.text_soft),
            p,
            cx,
        )
        .into_any_element(),
        cell(
            "tw-closed",
            t!("trade_window.figure.closed").to_string(),
            zone_label.1.clone(),
            moon(p.text_soft),
            p,
            cx,
        )
        .into_any_element(),
        cell(
            "tw-held",
            t!("trade_window.figure.held").to_string(),
            format_duration_s(record.close_date - record.buy_date),
            moon(p.text_soft),
            p,
            cx,
        )
        .into_any_element(),
    ];
    let gap = design::ui_px(cx, design::CHROME_GAP);
    match narrow {
        true => h_flex()
            .id("tw-rail-narrow")
            .w_full()
            .flex_wrap()
            .gap(gap)
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 6.0))
            .border_b_1()
            .border_color(moon(p.border))
            .bg(moon(p.panel))
            .children(cells)
            .into_any_element(),
        false => v_flex()
            .id("tw-rail")
            .h_full()
            .w(design::font_w_px(cx, RAIL_W))
            .flex_none()
            .gap(gap)
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .border_l_1()
            .border_color(moon(p.border))
            .bg(moon(p.panel))
            .children(cells)
            .into_any_element(),
    }
}
