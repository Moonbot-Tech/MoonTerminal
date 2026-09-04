//! What BOTH comparison tables of the tuner share: the [`MetricCol`] descriptor, the pool of
//! metric columns they pick from, the body-cell renderer and the click-to-sort rule.
//!
//! Which columns each table actually shows — and how wide it lays them out — belongs to that
//! table: `strat_columns` for the strategy list, `coin_columns` for the coin table. Only the
//! things that MUST be identical in both live here, so a change meant for one table cannot
//! quietly reshape the other.
//!
//! Kept apart from the rendering so a column cannot exist in the heading and not in the body —
//! both read the SAME descriptor.

use gpui::*;
use moon_ui::MoonPalette;

use super::super::summary::{fmt_signed, sign_color};
use crate::design::moon;
use moon_core::db::analytics::GroupStat;

/// One NUMERIC column of the comparison tables.
///
/// The heading and the body cell come from the SAME descriptor, so such a column cannot exist in
/// one and not the other, and its value cannot end up rendered under a neighbour's heading — a
/// misalignment no layout would reveal, since the table renders cleanly either way and only the
/// titles are wrong.
///
/// The two leading identity columns (kind, core) stay hand-paired: they are left-aligned, carry
/// their own tones and caption size, and modelling that here would add three optional fields for
/// two columns that no reordering touches.
#[derive(Clone, Copy)]
pub(super) struct MetricCol {
    /// i18n key of the heading; doubles as the stable sort id for this column.
    pub(super) key: &'static str,
    /// Preferred column width, in font-scaled px — what it takes when there is room.
    pub(super) w: f32,
    /// Narrowest it may be squeezed to before its own values stop fitting (font-scaled px).
    ///
    /// Per column, not a shared fraction of `w`: "+1795.21" and "187" do not stop fitting at
    /// the same proportion, and one number silently clipped is worse than a table that has to
    /// give up a column.
    pub(super) min_w: f32,
    /// Cell text for one aggregate.
    pub(super) text: fn(&GroupStat) -> String,
    /// Value whose sign colours the cell; `None` renders neutral.
    pub(super) signed: Option<fn(&GroupStat) -> f64>,
    /// Value used when the list is sorted by this column.
    pub(super) sort: fn(&GroupStat) -> f64,
}

pub(super) const COL_TRADES: MetricCol = MetricCol {
    key: "analytics.kpi.trades",
    w: 56.0,
    min_w: 38.0,
    text: |g| g.n.to_string(),
    signed: None,
    sort: |g| g.n as f64,
};
pub(super) const COL_PROFIT: MetricCol = MetricCol {
    key: "analytics.col.profit",
    w: 96.0,
    min_w: 74.0,
    text: tuner_profit_text,
    signed: Some(|g| g.profit),
    sort: |g| g.profit,
};
/// Average positive order spend in quote currency over the lens-neutral strategy scope.
pub(super) const COL_AVG_ORDER: MetricCol = MetricCol {
    key: "analytics.col.avg_order",
    w: 96.0,
    min_w: 74.0,
    text: tuner_avg_order_text,
    signed: None,
    sort: |g| g.avg_order,
};
/// Raw total strategy profit as a percentage of its average positive order size.
pub(super) const COL_PROFIT_PCT: MetricCol = MetricCol {
    key: "analytics.col.profit_pct",
    w: 78.0,
    min_w: 58.0,
    text: |g| {
        moon_core::util::fmt::signed_pct(g.profit_pct_of_avg_order(), 1)
            .map(|(text, _)| text)
            .unwrap_or_default()
    },
    signed: Some(|g| g.profit_pct_of_avg_order()),
    sort: |g| g.profit_pct_of_avg_order(),
};
pub(super) const COL_AVG: MetricCol = MetricCol {
    key: "analytics.kpi.avg_short",
    w: 70.0,
    min_w: 48.0,
    text: |g| fmt_signed(g.avg()),
    signed: Some(|g| g.avg()),
    sort: |g| g.avg(),
};
pub(super) const COL_WINRATE: MetricCol = MetricCol {
    key: "analytics.kpi.winrate",
    w: 56.0,
    min_w: 44.0,
    text: |g| format!("{:.1}%", g.winrate()),
    signed: None,
    sort: |g| g.winrate(),
};
pub(super) const COL_PF: MetricCol = MetricCol {
    key: "analytics.col.pf",
    w: 52.0,
    min_w: 38.0,
    text: |g| format!("{:.2}", g.pf),
    signed: None,
    sort: |g| g.pf,
};
pub(super) const COL_BEST: MetricCol = MetricCol {
    key: "analytics.col.best",
    w: 70.0,
    min_w: 52.0,
    text: |g| fmt_signed(g.best),
    signed: Some(|g| g.best),
    sort: |g| g.best,
};
/// How many coins the strategy's blacklist / whitelist name. Zero renders EMPTY, not "0":
/// most strategies list nothing, and a column of zeros reads as data where there is none.
pub(super) const COL_BL: MetricCol = MetricCol {
    key: "analytics.coins.black",
    w: 40.0,
    min_w: 26.0,
    text: |g| blank_if_zero(g.bl),
    signed: None,
    sort: |g| g.bl as f64,
};
pub(super) const COL_WL: MetricCol = MetricCol {
    key: "analytics.coins.white",
    w: 40.0,
    min_w: 26.0,
    text: |g| blank_if_zero(g.wl),
    signed: None,
    sort: |g| g.wl as f64,
};

/// A count that renders as nothing when it is zero.
pub(super) fn blank_if_zero(n: i64) -> String {
    if n > 0 { n.to_string() } else { String::new() }
}

pub(super) const COL_WORST: MetricCol = MetricCol {
    key: "analytics.col.worst",
    w: 70.0,
    min_w: 52.0,
    text: |g| fmt_signed(g.worst),
    signed: Some(|g| g.worst),
    sort: |g| g.worst,
};

/// Format active-lens profit with its exact quote ticker in the raw-money lens.
///
/// Args:
///     group: Aggregate whose active-lens profit should be displayed.
///
/// Returns:
///     Signed profit with an exact ticker in quote mode, `%` in percent mode, or an em dash
///     when raw money is not comparable.
fn tuner_profit_text(group: &GroupStat) -> String {
    if super::super::pnl_is_pct() {
        return fmt_signed(group.profit);
    }
    match group.quote {
        moon_core::db::QuoteScope::Single(currency) if group.profit.is_finite() => {
            format!(
                "{} {}",
                signed_quote_amount(group.profit, currency),
                currency.ticker()
            )
        }
        moon_core::db::QuoteScope::Empty
        | moon_core::db::QuoteScope::Single(_)
        | moon_core::db::QuoteScope::Mixed
        | moon_core::db::QuoteScope::Unknown => "—".to_string(),
    }
}

/// Format the lens-neutral average order with its exact quote ticker.
///
/// Args:
///     group: Aggregate whose average positive order spend should be displayed.
///
/// Returns:
///     Quote amount and ticker, or an em dash for mixed, unknown, or non-finite data.
fn tuner_avg_order_text(group: &GroupStat) -> String {
    match group.quote {
        moon_core::db::QuoteScope::Single(currency) if group.avg_order.is_finite() => format!(
            "{} {}",
            quote_amount(group.avg_order, currency),
            currency.ticker()
        ),
        moon_core::db::QuoteScope::Empty
        | moon_core::db::QuoteScope::Single(_)
        | moon_core::db::QuoteScope::Mixed
        | moon_core::db::QuoteScope::Unknown => "—".to_string(),
    }
}

/// Format a signed raw quote amount at its currency's display precision.
///
/// Args:
///     value: Signed quote amount.
///     currency: Currency controlling display precision.
///
/// Returns:
///     Signed amount with stable fiat decimals or compact crypto precision.
fn signed_quote_amount(value: f64, currency: moon_core::db::QuoteCurrency) -> String {
    let sign = if value >= 0.0 { "+" } else { "-" };
    format!("{sign}{}", quote_amount(value.abs(), currency))
}

/// Format an unsigned quote amount with grouped thousands and currency-aware precision.
///
/// Args:
///     value: Non-negative quote amount.
///     currency: Currency controlling display precision.
///
/// Returns:
///     Readable amount without a ticker. Fiat and stable quotes retain exactly two decimals;
///     crypto quotes retain up to eight meaningful decimals without width-padding zeros.
fn quote_amount(value: f64, currency: moon_core::db::QuoteCurrency) -> String {
    let decimals = currency.display_decimals();
    if decimals == 2 {
        moon_core::util::fmt::group_decimal(&format!("{value:.2}"))
    } else {
        moon_core::util::fmt::compact(value, decimals)
    }
}

/// Click a sortable column heading: the first click sorts descending, clicking the column that
/// is already the key flips it to ascending.
///
/// A free function over the state slot rather than a method, because both comparison tables on
/// this page keep their own `(key, descending)` and the rule for turning a click into one must
/// not be able to differ between them.
pub(super) fn toggle_sort_key(sort: &mut Option<(String, bool)>, key: &str) {
    let desc = match sort {
        Some((k, d)) if k == key => !*d,
        _ => true,
    };
    *sort = Some((key.to_string(), desc));
}

/// Arrow suffix for a heading (`" ▼"` / `" ▲"`), empty when that column is not the sort key.
pub(super) fn sort_arrow_of(sort: &Option<(String, bool)>, key: &str) -> &'static str {
    match sort {
        Some((k, d)) if k == key => {
            if *d {
                " ▼"
            } else {
                " ▲"
            }
        }
        _ => "",
    }
}

/// Body cell of a [`MetricCol`]: text and sign colour both derived from the same descriptor, so
/// a reordered column carries its colouring with it.
///
/// `scale` is the caller's hoisted font scale shared by every cell in the rendered table.
pub(super) fn metric_cell(
    col: &MetricCol,
    g: &GroupStat,
    p: MoonPalette,
    scale: f32,
) -> impl IntoElement {
    let color = match col.signed {
        Some(value) => sign_color(p, value(g)),
        None => p.text_soft,
    };
    num_cell(scale, col, (col.text)(g), color)
}

/// Body cell of a strategy metric at a content-measured fixed width.
///
/// The strategy table measures its current formatted values once per render and passes the same
/// widths to its rows and header. The coin table keeps using [`metric_cell`] because its separate
/// layout contract still permits proportional shrinking from descriptor widths.
///
/// Args:
///     col: Metric descriptor supplying text and sign colour.
///     group: Aggregate rendered in this row.
///     palette: Active palette used for sign-aware text colour.
///     width: Width of the longest currently rendered value in this metric column, in pixels.
pub(super) fn fixed_metric_cell(
    col: &MetricCol,
    group: &GroupStat,
    palette: MoonPalette,
    width: f32,
) -> impl IntoElement {
    let color = match col.signed {
        Some(value) => sign_color(palette, value(group)),
        None => palette.text_soft,
    };
    div()
        .w(px(width))
        .flex_none()
        .text_right()
        .text_color(moon(color))
        .child((col.text)(group))
}

/// Render one right-aligned numeric cell using a caller-hoisted font `scale`.
///
/// Shrinkable down to the column's own floor: when the row runs out of width every column
/// gives a little, rather than the whole rigid cluster sliding off the right edge. Below the
/// floor it stops — a number squeezed past legibility is worse than an honest overflow.
fn num_cell(scale: f32, col: &MetricCol, text: String, color: u32) -> impl IntoElement {
    div()
        .w(px(col.w * scale))
        .min_w(px(col.min_w * scale))
        .flex_shrink_1()
        .truncate()
        .text_right()
        .text_color(moon(color))
        .child(text)
}

// Explicit imports, never `use super::*`: the parent imports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
