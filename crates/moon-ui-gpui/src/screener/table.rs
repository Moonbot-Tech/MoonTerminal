//! Screener table columns and rows: schema, value formatting, sorting, and `MoonDataRow` rendering.

use std::collections::HashSet;

use gpui::*;
use moon_ui::{MoonDataCell, MoonDataRow, MoonPalette, MoonTone};

use moon_core::market::ScreenerRow;
use moon_core::util::fmt::{self, DeltaSign};

use crate::panels::num;

use super::view::ScreenerView;

pub(super) use crate::design::{moon, moon_alpha};

/// Column definition: key, title, width, and whether the contents are right-aligned.
pub(super) type ColDef = (&'static str, &'static str, f32, bool);

/// Table columns using untranslated Moonbot field labels, consistent with Orders and Report.
pub(super) const COLS: &[ColDef] = &[
    ("market", "Market", 92.0, false),
    ("core", "Core", 76.0, false),
    ("vol24", "Vol.", 66.0, true),
    ("hvol", "H.vol", 66.0, true),
    ("vol1m", "1m vol", 62.0, true),
    ("vol3m", "3m vol", 62.0, true),
    ("vol5m", "5m vol", 62.0, true),
    ("ask", "ASK", 88.0, true),
    ("high1h", "H.High", 88.0, true),
    ("maxord", "Max.Order", 68.0, true),
    ("d24h", "24h Delta", 68.0, true),
    ("d3h", "3h.Delta", 62.0, true),
    ("d1h", "h.Delta", 62.0, true),
    ("d15m", "15m Delta", 68.0, true),
    ("d1m", "1m Delta", 62.0, true),
    ("d72h", "72h Delta", 66.0, true),
    ("funding", "Funding", 66.0, true),
    ("markd", "MarkPrice", 68.0, true),
    ("lev", "Leverage", 104.0, false),
    ("step", "PriceStep", 92.0, true),
    ("orders", "Orders", 54.0, true),
    // Two profit counters, named as MoonBot names them. `pnl` is the core's `TotalProfitB/L/S`
    // (the last figure the exchange pushed for the instrument); `session_profit` is the counter
    // the markets table's "Reset Session" clears. The first was titled "Session" and KEYED
    // `session` until 2026-09-12; that key is retired (see `LEGACY_SESSION_KEY`), never reused.
    ("pnl", "PnL", 76.0, true),
    ("session_profit", "Session", 76.0, true),
    ("pos", "Pos", 66.0, true),
];

/// Return the displayed title for a Screener column key.
///
/// Footer filters resolve their labels through the same schema as the header so the two surfaces
/// cannot drift back to different Moonbot terminology.
pub(super) fn column_title(key: &str) -> &'static str {
    COLS.iter()
        .find(|column| column.0 == key)
        .map_or("", |column| column.1)
}

/// The column key a layout saved before 2026-09-12 spelled as `session`.
///
/// That column printed the core's PnL counter under the title "Session". The key is RETIRED, not
/// reassigned: the real Session column is `session_profit`, so a saved `session` can only ever mean
/// the PnL column, and the rewrite below needs no guess about when the layout was written. Had the
/// new column taken the old key, a layout saved afterwards with PnL hidden and Session shown would
/// be indistinguishable from a pre-rename one, and every reopen would swap the two.
const LEGACY_SESSION_KEY: &str = "session";
const PNL_KEY: &str = "pnl";

/// Map a saved column key to its current spelling.
///
/// Read-side only — the visible-column list, the sort preference and the width map all pass their
/// keys through here on load, and the next write stores current keys.
///
/// Args:
///     key: A column key as it was stored.
///
/// Returns:
///     `pnl` for the retired `session` key; every other key unchanged.
pub(super) fn current_key(key: &str) -> &str {
    if key == LEGACY_SESSION_KEY {
        PNL_KEY
    } else {
        key
    }
}

/// Rewrite the retired `session` key of a saved column list to `pnl`.
///
/// Args:
///     keys: The saved visible-column keys, in whatever order they were stored.
///
/// Returns:
///     The same keys with the retired entry renamed.
pub(super) fn migrate_legacy_keys(keys: Vec<String>) -> Vec<String> {
    keys.into_iter()
        .map(|key| current_key(&key).to_string())
        .collect()
}

/// Rewrite a saved sort on the retired `session` key to `pnl`.
///
/// Args:
///     preference: The saved sort, if any.
///
/// Returns:
///     The preference with its column key brought current.
pub(super) fn migrate_legacy_sort(
    preference: Option<moon_core::config::TableSortPreference>,
) -> Option<moon_core::config::TableSortPreference> {
    preference.map(|mut p| {
        p.column = current_key(&p.column).to_string();
        p
    })
}

/// Rewrite the retired `session` key of a saved width map to `pnl`.
///
/// A width saved for the old PnL column stays with the PnL column; a layout that somehow carries
/// both keys keeps the current one, since that width was set after the rename.
///
/// Args:
///     widths: Column widths as they were stored.
///
/// Returns:
///     The map with the retired key renamed, or unchanged when it is absent.
pub(super) fn migrate_legacy_widths(
    mut widths: std::collections::HashMap<String, f32>,
) -> std::collections::HashMap<String, f32> {
    if let Some(width) = widths.remove(LEGACY_SESSION_KEY) {
        widths.entry(PNL_KEY.to_string()).or_insert(width);
    }
    widths
}

/// Restore a visible Screener sort as `(key, descending)`.
///
/// The historical Vol.-descending default remains preferred while that column is visible. If it is
/// hidden, the first visible canonical column becomes the descending fallback so sorting can never
/// remain active behind a header the user cannot click.
pub(super) fn restore_sort(
    preference: Option<moon_core::config::TableSortPreference>,
    visible: &HashSet<String>,
) -> (String, bool) {
    preference
        .filter(|preference| {
            visible.contains(&preference.column)
                && COLS.iter().any(|column| column.0 == preference.column)
        })
        .map(|preference| (preference.column, !preference.ascending))
        .unwrap_or_else(|| {
            let key = if visible.contains("vol24") {
                "vol24"
            } else {
                COLS.iter()
                    .find(|column| visible.contains(column.0))
                    .map_or("vol24", |column| column.0)
            };
            (key.to_string(), true)
        })
}

/// Screener row data plus the displayed name of its core, `row.core`, which is also the core a
/// click opens the chart on.
pub(super) struct Entry {
    pub(super) row: ScreenerRow,
    pub(super) core_name: SharedString,
}

/// Parse the minimum volume from the DVol filter, such as `500`, `500k`, or `2m`.
pub(super) fn parse_vol(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let lower = s.to_lowercase();
    let (num_part, mult) = match lower.strip_suffix(['k', 'к']) {
        Some(n) => (n, 1_000.0),
        None => match lower.strip_suffix(['m', 'м']) {
            Some(n) => (n, 1_000_000.0),
            None => (lower.as_str(), 1.0),
        },
    };
    num_part
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .unwrap_or(0.0)
        * mult
}

/// Format volume in Moonbot's compact form, such as `1.7m` or `320k`.
fn vol_fmt(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}b", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}m", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.0}k", v / 1e3)
    } else if a > 0.0 {
        format!("{v:.0}")
    } else {
        "0".to_string()
    }
}

pub(super) fn sort_entries(entries: &mut [Entry], key: &str, desc: bool) {
    // Compare string columns by name, leverage by its dedicated key, and remaining numeric
    // columns using total floating-point ordering.
    let cmp_str = |a: &Entry, b: &Entry, f: fn(&Entry) -> &str| f(a).cmp(f(b));
    entries.sort_by(|a, b| {
        let ord = match key {
            "market" => cmp_str(a, b, |e| &e.row.market),
            "core" => cmp_str(a, b, |e| &e.core_name),
            "lev" => (lev_sort(a)).total_cmp(&lev_sort(b)),
            _ => num_key(a, key).total_cmp(&num_key(b, key)),
        };
        if desc { ord.reverse() } else { ord }
    });
}

fn lev_sort(e: &Entry) -> f64 {
    if e.row.leverage_x > 0 {
        f64::from(e.row.leverage_x) + 10_000.0
    } else {
        f64::from(e.row.max_leverage)
    }
}

fn num_key(e: &Entry, key: &str) -> f64 {
    let r = &e.row;
    match key {
        "vol24" => r.vol_24h,
        "hvol" => r.vol_1h,
        "vol1m" => r.vol_1m,
        "vol3m" => r.vol_3m,
        "vol5m" => r.vol_5m,
        "ask" => r.ask,
        "high1h" => r.high_1h,
        "maxord" => r.max_order,
        "d24h" => r.d_24h,
        "d3h" => r.d_3h,
        "d1h" => r.d_1h,
        "d15m" => r.d_15m,
        "d1m" => r.d_1m,
        "d72h" => r.d_72h,
        "funding" => r.funding_pct,
        "markd" => r.mark_delta_pct.unwrap_or(0.0),
        "step" => r.price_step,
        "orders" => f64::from(r.orders),
        "pnl" => r.core_pnl,
        // An unstated counter sorts with the zeros, as `markd` does above: there is no order among
        // "unknown", and pinning it to either end would push a whole older core to the top.
        "session_profit" => r.session.unwrap_or(0.0),
        "pos" => r.pos_size,
        _ => 0.0,
    }
}

/// Signed-percentage cell for values whose direction is semantically meaningful.
///
/// A value rounding to zero renders unsigned and muted: a raw `{:+.1}` prints "-0.0%" for any
/// small negative, which reads as a decline that is not there.
fn signed_pct_cell(v: f64) -> MoonDataCell {
    pct_cell(fmt::signed_pct(v, 1))
}

/// Render a formatted percentage with the standard sign→tone mapping.
fn pct_cell(formatted: Option<(String, DeltaSign)>) -> MoonDataCell {
    match formatted {
        Some((text, sign)) => MoonDataCell::text(text).tone(sign.pick(
            MoonTone::Positive,
            MoonTone::Danger,
            MoonTone::Muted,
        )),
        None => MoonDataCell::text("—").tone(MoonTone::Muted),
    }
}

/// Signed dollar cell for a profit counter, at the two fixed places a money column lines up on.
///
/// Sign and tone come from the ROUNDED value, through the shared rule: a loss of a tenth of a cent
/// would otherwise print `-0.00$` in red, a minus wearing a zero. A value that is not a number
/// prints the dash.
fn money_cell(v: f64) -> MoonDataCell {
    match fmt::signed_fixed(v, 2) {
        Some((text, sign)) => MoonDataCell::text(format!("{text}$")).tone(sign.pick(
            MoonTone::Positive,
            MoonTone::Danger,
            MoonTone::Muted,
        )),
        None => MoonDataCell::text("—").tone(MoonTone::Muted),
    }
}

pub(super) fn screener_row(
    e: &Entry,
    view: &Entity<ScreenerView>,
    p: MoonPalette,
    cols: &[&ColDef],
) -> MoonDataRow {
    let r = &e.row;
    // Every Screener delta is an unsigned retained-range magnitude. One formatter keeps all six
    // periods free of a misleading forced sign while preserving the shared zero rounding.
    let delta_cell = |v: f64| pct_cell(fmt::pct(v, 1));
    let vol_cell = |v: f64| {
        if v > 0.0 {
            MoonDataCell::text(vol_fmt(v))
        } else {
            MoonDataCell::text("0").tone(MoonTone::Muted)
        }
    };
    let cells: Vec<MoonDataCell> = cols
        .iter()
        .map(|&&(key, ..)| match key {
            "market" => MoonDataCell::element(market_cell(e, view, p)),
            "core" => MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
            "vol24" => vol_cell(r.vol_24h),
            "hvol" => vol_cell(r.vol_1h),
            "vol1m" => vol_cell(r.vol_1m),
            "vol3m" => vol_cell(r.vol_3m),
            "vol5m" => vol_cell(r.vol_5m),
            "ask" => MoonDataCell::text(num(r.ask)),
            "high1h" => {
                if r.high_1h > 0.0 {
                    MoonDataCell::text(num(r.high_1h))
                } else {
                    MoonDataCell::text("—").tone(MoonTone::Muted)
                }
            }
            "maxord" => vol_cell(r.max_order),
            "d24h" => delta_cell(r.d_24h),
            "d3h" => delta_cell(r.d_3h),
            "d1h" => delta_cell(r.d_1h),
            "d15m" => delta_cell(r.d_15m),
            "d1m" => delta_cell(r.d_1m),
            "d72h" => delta_cell(r.d_72h),
            // Moonbot convention: negative funding is green and positive funding is orange; the
            // first two arguments are swapped to express that inverted sign mapping.
            "funding" => match fmt::signed_pct(r.funding_pct, 2) {
                Some((text, sign)) => MoonDataCell::text(text).tone(sign.pick(
                    MoonTone::Negative,
                    MoonTone::Positive,
                    MoonTone::Muted,
                )),
                None => MoonDataCell::text("—").tone(MoonTone::Muted),
            },
            "markd" => match r.mark_delta_pct {
                Some(v) => signed_pct_cell(v),
                None => MoonDataCell::text("—").tone(MoonTone::Muted),
            },
            "lev" => {
                let text = if r.leverage_x > 0 {
                    let mode = match r.isolated {
                        Some(true) => " Isolated",
                        Some(false) => " Cross",
                        None => "",
                    };
                    format!("{} / {}{}", r.leverage_x, r.max_leverage, mode)
                } else if r.max_leverage > 0 {
                    format!("{}", r.max_leverage)
                } else {
                    "—".to_string()
                };
                MoonDataCell::text(text).tone(if r.leverage_x > 0 {
                    MoonTone::Default
                } else {
                    MoonTone::Muted
                })
            }
            "step" => {
                // Show the price step as a percentage of ask and as an absolute value, matching
                // Moonbot's `0.01% / 0.001` format.
                if r.price_step > 0.0 && r.ask > 0.0 {
                    MoonDataCell::text(format!(
                        "{:.2}% / {}",
                        r.price_step / r.ask * 100.0,
                        num(r.price_step)
                    ))
                    .tone(MoonTone::Muted)
                } else {
                    MoonDataCell::text("—").tone(MoonTone::Muted)
                }
            }
            "orders" => {
                if r.orders > 0 {
                    MoonDataCell::text(r.orders.to_string()).tone(MoonTone::Accent)
                } else {
                    MoonDataCell::text("0").tone(MoonTone::Muted)
                }
            }
            "pnl" => money_cell(r.core_pnl),
            // A dash, not a zero: `None` is "this core does not state the counter" (a build
            // predating the protocol field, or a base currency the terminal cannot value), and a
            // real zero arrives as `Some(0.0)` — that one prints, because the core states it.
            "session_profit" => match r.session {
                Some(v) => money_cell(v),
                None => MoonDataCell::text("—").tone(MoonTone::Muted),
            },
            "pos" => {
                if r.pos_size != 0.0 {
                    MoonDataCell::text(num(r.pos_size))
                } else {
                    MoonDataCell::text("0").tone(MoonTone::Muted)
                }
            }
            _ => MoonDataCell::text(""),
        })
        .collect();
    MoonDataRow::new(cells)
}

/// Build a full-cell clickable market cell.
///
/// Clicking asks the ChartTabs group owning `row.core` to open or focus the market in Main and
/// select `Tab::Main`, without raising or focusing that group's OS window.
fn market_cell(
    e: &Entry,
    view: &Entity<ScreenerView>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    let market = e.row.market.clone();
    let core = e.row.core;
    let view = view.clone();
    div()
        .id(SharedString::from(format!("scr-mkt-{core}-{market}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        // Inherit font family and size from the cell's MoonUI cascade; override the text color
        // with Accent and the font weight with Medium.
        .text_color(rgb(MoonTone::Accent.color(p)))
        .font_weight(FontWeight::MEDIUM)
        .child(e.row.coin.clone())
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| {
                this.backend.update(cx, |b, bcx| {
                    b.open_on_main((core, market.clone()), false);
                    bcx.notify();
                });
            });
        })
}

#[cfg(test)]
mod tests;
