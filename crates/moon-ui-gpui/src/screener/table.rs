//! Screener table columns and rows: schema, value formatting, sorting, and `MoonDataRow` rendering.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonDataCell, MoonDataRow, MoonPalette, MoonTheme, MoonTone, MoonTooltipView};
use rust_i18n::t;

use moon_core::config::{TableSortPreference, WindowLayout};
use moon_core::market::SharedScreenerRow;
use moon_core::session::CoreId;
use moon_core::util::fmt::{self, DeltaSign};

use crate::controls::{CoinMenuCtx, CoinMenuOrigin};
use crate::design;
use crate::display_text::{flatten_lines, plural_key};
use crate::panels::num;

use super::auto_figures::{AUTO_FIGURES_KEY, ScanBook, ScanLabels};
use super::grouped::{LevSummary, Mode, aggregate};
use super::prepared::{Prepared, SelectedRow, account};
use super::view::ScreenerView;

pub(super) use crate::design::{moon, moon_alpha};

/// Column definition: key, title, width, and whether the contents are right-aligned.
pub(super) type ColDef = (&'static str, &'static str, f32, bool);

/// Table columns using untranslated Moonbot field labels, consistent with Orders and Report.
pub(super) const COLS: &[ColDef] = &[
    ("market", "Market", 92.0, false),
    // The coin's cross-exchange identity (`ScreenerRow::identity`): `BONK` beside a Market cell
    // that reads `1kBONK` on one exchange and `1000BONK` on another. Sorting by it lines one coin
    // up across every exchange. Not a Moonbot column — Moonbot shows one exchange at a time.
    (COIN_KEY, "Coin", 70.0, false),
    // Three columns about WHERE the row is, none of them Moonbot's — Moonbot shows one exchange,
    // so it has nothing to say here. `exchange` is the venue caption every core list uses, `type`
    // the market kind of that venue, and `listed` whether the coin trades on both kinds of this
    // exchange or on this one alone, in the words of Moonbot's `ListedType` strategy filter.
    (EXCHANGE_KEY, "Exchange", 124.0, false),
    ("core", "Core", 76.0, false),
    (TYPE_KEY, "Type", 64.0, false),
    (LISTED_KEY, "Listed", 92.0, false),
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
    // The coin's best auto-figure event and the distance to its nearest line, from the last press
    // of the Calculate button (`super::auto_figures`). Not a Moonbot column, so its header and its
    // entry in the column menu are translated (`column_label`); the title here is the fallback.
    (AUTO_FIGURES_KEY, "Auto figures", 150.0, false),
];

/// Visible columns in the default view, in source order.
pub(super) const DEFAULT_COLS: &[&str] = &[
    "market",
    EXCHANGE_KEY,
    "core",
    "vol24",
    "hvol",
    "vol5m",
    "ask",
    "d24h",
    "d1h",
    "d15m",
    "funding",
    "lev",
    "orders",
    "pnl",
    "pos",
    AUTO_FIGURES_KEY,
];

/// Default sort as (key, descending).
pub(super) const DEFAULT_SORT: (&str, bool) = ("vol24", true);

/// Initial text in the session-only volume filter.
pub(super) const DEFAULT_MIN_VOL: &str = "1m";

/// The header and column-menu caption of a column: the untranslated Moonbot label of `COLS`, or
/// the translated name of a column of the terminal's own.
pub(super) fn column_label(key: &str, title: &'static str) -> SharedString {
    match key {
        AUTO_FIGURES_KEY => t!("auto_figures.title").to_string().into(),
        _ => title.into(),
    }
}

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

/// Key of the cross-exchange coin column.
const COIN_KEY: &str = "coin";
/// Key of the exchange column: the venue caption with its logo.
const EXCHANGE_KEY: &str = "exchange";
/// Key of the market-kind column.
const TYPE_KEY: &str = "type";
/// Key of the cross-listing column.
const LISTED_KEY: &str = "listed";

/// How "nothing answers" is written, in the Type and Listed cells and in the lists that filter
/// them: a question mark rather than the dash, which elsewhere in this table means "nothing to
/// show". One constant, so a cell and the choice that selects it cannot be spelled differently.
pub(super) const UNKNOWN_CHOICE: &str = "?";

/// How a Listed answer that is still on its way is written: the donor that will give it has not
/// been connected for its warm-up yet. Not the question mark — that one stays for good, this one
/// turns into an answer within the minute.
const PENDING_MARK: &str = "…";

/// The migration version that installs the curated Screener view.
pub(super) const COLUMNS_MIGRATION: u32 = 4;

/// Replace the saved view for this table and release widths and order to the column schema.
pub(super) fn reset_layout_to_default(layout: &mut WindowLayout, id: &str) {
    layout.table_visible_columns.insert(
        id.to_string(),
        DEFAULT_COLS.iter().map(|&key| key.to_string()).collect(),
    );
    layout.screener_columns = None;
    layout.table_sorts.insert(
        id.to_string(),
        TableSortPreference {
            column: DEFAULT_SORT.0.to_string(),
            ascending: !DEFAULT_SORT.1,
        },
    );
    layout.table_column_widths.remove(id);
    layout.table_column_order.remove(id);
}

/// Reset a pre-curated layout once and report whether the caller must save the marker.
pub(super) fn migrate_layout(layout: &mut WindowLayout, id: &str) -> bool {
    if layout.screener_columns_migration.unwrap_or(0) >= COLUMNS_MIGRATION {
        return false;
    }
    reset_layout_to_default(layout, id);
    layout.screener_columns_migration = Some(COLUMNS_MIGRATION);
    true
}

/// Recover the visible set from saved keys, using the curated view if none remain valid.
pub(super) fn restore_visible_columns(saved: Option<Vec<String>>) -> HashSet<String> {
    let keys: HashSet<_> = saved
        .into_iter()
        .flatten()
        .filter(|key| COLS.iter().any(|column| column.0 == key))
        .collect();
    if keys.is_empty() {
        DEFAULT_COLS.iter().map(|&key| key.to_string()).collect()
    } else {
        keys
    }
}

/// The sort to fall back to once the column the table is sorted by has been hidden, as
/// `((key, descending), store)` — `store` says whether that sort is to be written over the stored
/// preference.
///
/// The auto-figures column is never stored as the sort (its answers live for one window session,
/// see `ScreenerView::set_sort`), so while it is the sort the STORED preference is still the one
/// the user chose before it. Hiding it therefore goes back to that stored sort, and stores nothing:
/// writing the fallback would erase the very preference the column's sort was careful to keep.
/// Any other hidden sort column falls back through [`restore_sort`] as before, and that fallback
/// is stored.
///
/// Args:
///     sort_key: The column the table is sorted by, now hidden.
///     sort_desc: Its direction.
///     stored: The stored sort preference, already brought current ([`migrate_legacy_sort`]).
///     visible: The visible columns after the hide.
pub(super) fn sort_after_hiding(
    sort_key: &str,
    sort_desc: bool,
    stored: Option<moon_core::config::TableSortPreference>,
    visible: &HashSet<String>,
) -> ((String, bool), bool) {
    if sort_key == AUTO_FIGURES_KEY {
        return (restore_sort(stored, visible), false);
    }
    let current = moon_core::config::TableSortPreference {
        column: sort_key.to_string(),
        ascending: !sort_desc,
    };
    (restore_sort(Some(current), visible), true)
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
            let key = if visible.contains(DEFAULT_SORT.0) {
                DEFAULT_SORT.0
            } else {
                COLS.iter()
                    .find(|column| visible.contains(column.0))
                    .map_or(DEFAULT_SORT.0, |column| column.0)
            };
            (key.to_string(), DEFAULT_SORT.1)
        })
}

/// Screener row data plus the displayed name of its core, `row.core`, which is also the core a
/// click opens the chart on.
pub(super) struct Entry {
    pub(super) row: SharedScreenerRow,
    pub(super) core_name: SharedString,
    /// Normalized search text, leaving the market's display and chart identity intact.
    pub(super) market_uc: std::sync::Arc<str>,
    /// Normalized coin text shared by all filter comparisons for this entry.
    pub(super) coin_uc: std::sync::Arc<str>,
    /// Where the row's core is connected, for the Exchange and Type columns and the Type list.
    pub(super) venue: EntryVenue,
}

/// What the table shows about the exchange a row's core is connected to.
///
/// Resolved once per core on the UI thread, where the session's venue map and the translator
/// live, and copied onto that core's rows: the caption is the same text every core list in the
/// terminal prints for it. The default — no caption, no brand, no kind — is a core the session has
/// not identified yet.
#[derive(Clone, Default)]
pub(super) struct EntryVenue {
    /// Market-data identity, independent of its caption or whether it can be named.
    pub(super) id: Option<moon_core::feed::ExchangeId>,
    /// The venue caption, such as `Binance Futures`; empty until the core is identified.
    pub(super) label: SharedString,
    /// The brand whose logo stands beside the caption; `None` for a platform this build cannot
    /// name, which then shows its own caption and no logo.
    pub(super) brand: Option<moon_core::venue::Brand>,
    /// The venue's market kind; `None` wherever the brand is.
    pub(super) kind: Option<moon_core::venue::MarketKind>,
}

/// Where a market kind sorts in the Type column: spot, then perpetuals, then coin-margined, with
/// an unidentified exchange last in ascending order.
fn kind_rank(kind: Option<moon_core::venue::MarketKind>) -> u8 {
    use moon_core::venue::MarketKind;
    match kind {
        Some(MarketKind::Spot) => 0,
        Some(MarketKind::Futures) => 1,
        Some(MarketKind::Quarterly) => 2,
        None => 3,
    }
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

/// Stable index sorting preserves the source order within equal keys without moving row data.
pub(super) fn sort_indices(entries: &[Entry], order: &mut [usize], key: &str, desc: bool) {
    let direction = |ord: std::cmp::Ordering| if desc { ord.reverse() } else { ord };
    match key {
        "market" => {
            order.sort_by(|&a, &b| direction(entries[a].row.market.cmp(&entries[b].row.market)))
        }
        "core" => {
            order.sort_by(|&a, &b| direction(entries[a].core_name.cmp(&entries[b].core_name)))
        }
        COIN_KEY => {
            order.sort_by(|&a, &b| direction(entries[a].row.identity.cmp(&entries[b].row.identity)))
        }
        EXCHANGE_KEY => {
            order.sort_by(|&a, &b| direction(entries[a].venue.label.cmp(&entries[b].venue.label)))
        }
        TYPE_KEY => order.sort_by(|&a, &b| {
            direction(kind_rank(entries[a].venue.kind).cmp(&kind_rank(entries[b].venue.kind)))
        }),
        LISTED_KEY => {
            order.sort_by(|&a, &b| direction(entries[a].row.listing.cmp(&entries[b].row.listing)))
        }
        _ => {
            let values: Vec<_> = entries
                .iter()
                .map(|entry| {
                    float_sort_key(if key == "lev" {
                        lev_sort(entry)
                    } else {
                        num_key(entry, key)
                    })
                })
                .collect();
            order.sort_by(|&a, &b| direction(values[a].cmp(&values[b])));
        }
    }
}

/// The signed integer ordering used by `f64::total_cmp`, including NaN payloads and signed zero.
fn float_sort_key(value: f64) -> i64 {
    let bits = value.to_bits() as i64;
    bits ^ (((bits >> 63) as u64 >> 1) as i64)
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

/// What every row of one frame shares: the translated captions and the logo geometry.
///
/// Resolved once per table build on the UI thread — the row closure runs per visible row per
/// frame, and a dictionary lookup and a theme read in each of them would be paid forty times for
/// values that cannot differ between rows.
pub(super) struct RowStyle {
    /// Whether the exchange logos have been rasterized; until then the cell draws its caption
    /// alone rather than decoding an image inside a frame.
    pub(super) logos_ready: bool,
    /// Edge of a logo and the gap after it, already in theme pixels.
    pub(super) logo_px: Pixels,
    pub(super) logo_gap: Pixels,
    /// Base font size passed to MoonDataCell to render the exchange caption.
    pub(super) exchange_font_base: f32,
    /// Rendered ellipsis width at the exchange caption's font size.
    pub(super) exchange_ellipsis: Pixels,
    /// Captions of the three market kinds, in [`kind_rank`] order.
    pub(super) kinds: [SharedString; 3],
    pub(super) both: SharedString,
    pub(super) spot_only: SharedString,
    pub(super) futures_only: SharedString,
    /// The latest press's answers, its words, and whether its run is still being worked off.
    pub(super) scan_book: std::rc::Rc<ScanBook>,
    pub(super) scan_labels: ScanLabels,
    pub(super) scan_running: bool,
}

/// Localized Type cell captions in the same order as `kind_rank`.
pub(super) fn short_kind_captions() -> [SharedString; 3] {
    [
        t!("screener.kind_short_spot").to_string().into(),
        t!("screener.kind_short_futures").to_string().into(),
        t!("screener.kind_short_quarterly").to_string().into(),
    ]
}

/// Localized Listed cell captions for Both, Spot Only, and Futures Only.
pub(super) fn short_listed_captions() -> [SharedString; 3] {
    [
        t!("screener.listed_short_both").to_string().into(),
        t!("screener.listed_short_spot_only").to_string().into(),
        t!("screener.listed_short_futures_only").to_string().into(),
    ]
}

/// Exchange captions sit half a design pixel below the terminal body text.
const EXCHANGE_FONT_STEP: f32 = -0.5;

/// Resolve the cell's base font size and measure its ellipsis at the same density-tier size.
pub(super) fn exchange_caption_metrics(cx: &App) -> (f32, Pixels) {
    let text = cx.text_system();
    let font = text.resolve_font(&font(MoonTheme::active_tokens(cx).font_family(true)));
    let size = design::t_body_step_px(cx, EXCHANGE_FONT_STEP);
    let ellipsis = text.layout_width(font, size, '…').ceil();
    (design::body_font_base(cx, EXCHANGE_FONT_STEP), ellipsis)
}

/// Keep the caption on the first line even when the logo has to wrap out of sight.
fn exchange_cell(venue: &EntryVenue, style: &RowStyle) -> MoonDataCell {
    let logo = style
        .logos_ready
        .then_some(venue.brand)
        .flatten()
        .and_then(crate::media::exchange_logos::exchange_logo);
    let (logo_px, logo_gap) = (style.logo_px, style.logo_gap);
    let minimum_caption = style.exchange_ellipsis;
    let inset = if logo.is_some() {
        logo_px + logo_gap
    } else {
        px(0.0)
    };
    MoonDataCell::element(
        div()
            .min_w_0()
            .w_full()
            .h_full()
            .overflow_hidden()
            // The caption is the first flex item, so wrapping keeps it and clips the logo.
            .flex()
            .flex_row_reverse()
            .flex_wrap()
            .content_start()
            .items_center()
            .gap(logo_gap)
            .child(
                div()
                    .flex_1()
                    .min_w(minimum_caption)
                    .h_full()
                    .flex()
                    .items_center()
                    .child(div().min_w_0().truncate().child(venue.label.clone())),
            )
            .when_some(logo, |cell, logo| {
                cell.child(
                    div()
                        .debug_selector(|| "screener-exchange-logo".to_string())
                        .flex_none()
                        .size(logo_px)
                        .child(img(logo).size_full()),
                )
            }),
    )
    .font_size(style.exchange_font_base)
    .overflow_tooltip_with_inset(venue.label.clone(), inset, minimum_caption)
}

/// Format a leverage summary with a compact suffix for its known margin modes.
pub(super) fn lev_text(lev: LevSummary) -> String {
    let suffix = |mode| match mode {
        Mode::I => " I",
        Mode::C => " C",
        Mode::Mixed => " I/C",
        Mode::Unknown => "",
    };
    match lev {
        LevSummary::None => "—".into(),
        LevSummary::Max(max) => max.to_string(),
        LevSummary::Same { x, max, mode } => format!("{x} / {max}{}", suffix(mode)),
        LevSummary::Range { lo, hi, max, mode } => format!("{lo}–{hi} / {max}{}", suffix(mode)),
    }
}

/// Show the lead name for a singleton and a localized count for a multi-member group.
fn core_label(name: &SharedString, n: usize) -> SharedString {
    if n < 2 {
        name.clone()
    } else {
        let key = plural_key("screener.core_group", n as u64);
        t!(key.as_str(), n = n).to_string().into()
    }
}

/// List at most eight visible members, with their own leverage and PnL.
fn core_tooltip(entries: &[Entry], members: &[usize]) -> String {
    const LIMIT: usize = 8;
    let mut lines: Vec<_> = members
        .iter()
        .take(LIMIT)
        .map(|&index| {
            let entry = &entries[index];
            let name = flatten_lines(&entry.core_name);
            let lev = lev_text(aggregate([account(&entry.row)]).lev);
            let pnl = fmt::signed_fixed(entry.row.core_pnl, 2)
                .map_or_else(|| "—".into(), |(text, _)| format!("{text}$"));
            format!("{name} · {lev} · {pnl}")
        })
        .collect();
    if members.len() > LIMIT {
        let n = members.len() - LIMIT;
        let key = plural_key("screener.core_group_more", n as u64);
        lines.push(t!(key.as_str(), n = n).to_string());
    }
    lines.join("\n")
}

/// Truncate the core count above its dotted underline and build its breakdown on hover.
fn core_cell(
    e: &Entry,
    row: &SelectedRow,
    prepared: &Arc<Prepared>,
    p: MoonPalette,
) -> MoonDataCell {
    let label = core_label(
        &e.core_name,
        row.group.as_ref().map_or(1, |group| group.members.len()),
    );
    let Some(group) = row.group.as_ref().filter(|group| group.members.len() > 1) else {
        return MoonDataCell::text(label).tone(MoonTone::Muted);
    };
    let members = group.members.clone();
    let prepared = prepared.clone();
    let color = rgb(MoonTone::Accent.color(p));
    MoonDataCell::element(
        div()
            .id(SharedString::from(format!(
                "scr-cores-{}-{}",
                e.row.core, e.row.market
            )))
            .relative()
            .min_w_0()
            .text_color(color)
            .child(div().min_w_0().truncate().child(label))
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(1.0))
                    .bg(checkerboard(color, 1.0)),
            )
            .tooltip(move |_, cx| {
                let text = core_tooltip(&prepared.entries, &members);
                cx.new(|_| MoonTooltipView::new(text).max_width(440.0))
                    .into()
            }),
    )
}

/// Render market columns from the lead and account columns from its visible group.
pub(super) fn screener_row(
    row: &SelectedRow,
    prepared: &Arc<Prepared>,
    view: &WeakEntity<ScreenerView>,
    p: MoonPalette,
    cols: &[&ColDef],
    style: &RowStyle,
) -> MoonDataRow {
    use moon_core::market::listing::Listing;
    let e = &prepared.entries[row.lead];
    let r = &e.row;
    let acct = row
        .group
        .as_ref()
        .map_or_else(|| aggregate([account(r)]), |group| group.agg);
    // Every Screener delta is an unsigned retained-range magnitude. One formatter keeps all six
    // periods free of a misleading forced sign while preserving the shared zero rounding.
    let delta_cell = |v: f64| pct_cell(fmt::pct(v, 1));
    // Volumes are lone figures beside a coin, MoonBot's markets-table spelling: the label form,
    // the same one the detect cards print the same volume in.
    let vol_cell = |v: f64| {
        if v > 0.0 {
            MoonDataCell::text(fmt::short(v, fmt::Form::Label))
        } else {
            MoonDataCell::text("0").tone(MoonTone::Muted)
        }
    };
    let cells: Vec<MoonDataCell> = cols
        .iter()
        .map(|&&(key, ..)| match key {
            "market" => market_cell(row, prepared, view, p),
            COIN_KEY => match r.identity.is_empty() {
                true => MoonDataCell::text("—").tone(MoonTone::Muted),
                false => MoonDataCell::text(r.identity.clone()),
            },
            "core" => core_cell(e, row, prepared, p),
            EXCHANGE_KEY => match e.venue.label.is_empty() {
                true => MoonDataCell::text("—").tone(MoonTone::Muted),
                false => exchange_cell(&e.venue, style),
            },
            TYPE_KEY => match e.venue.kind {
                Some(kind) => {
                    MoonDataCell::text(style.kinds[usize::from(kind_rank(Some(kind)))].clone())
                }
                None => MoonDataCell::text(UNKNOWN_CHOICE).tone(MoonTone::Muted),
            },
            // The one-sided answers are the reason the column exists, so they carry the accent;
            // `Both` is the unremarkable case, unknown is the muted question mark and an answer
            // still on its way the muted ellipsis.
            LISTED_KEY => match r.listing {
                Listing::Pending => MoonDataCell::text(PENDING_MARK).tone(MoonTone::Muted),
                Listing::Both => MoonDataCell::text(style.both.clone()).tone(MoonTone::Muted),
                Listing::SpotOnly => {
                    MoonDataCell::text(style.spot_only.clone()).tone(MoonTone::Accent)
                }
                Listing::FuturesOnly => {
                    MoonDataCell::text(style.futures_only.clone()).tone(MoonTone::Accent)
                }
                Listing::Unknown => MoonDataCell::text(UNKNOWN_CHOICE).tone(MoonTone::Muted),
            },
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
            "lev" => MoonDataCell::text(lev_text(acct.lev)).tone(match acct.lev {
                LevSummary::Same { .. } | LevSummary::Range { .. } => MoonTone::Default,
                LevSummary::None | LevSummary::Max(_) => MoonTone::Muted,
            }),
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
                if acct.orders > 0 {
                    MoonDataCell::text(acct.orders.to_string()).tone(MoonTone::Accent)
                } else {
                    MoonDataCell::text("0").tone(MoonTone::Muted)
                }
            }
            "pnl" => money_cell(acct.pnl),
            // The amount is absent only when every visible member leaves it unstated.
            "session_profit" => match acct.session {
                Some(v) => money_cell(v),
                None => MoonDataCell::text("—").tone(MoonTone::Muted),
            },
            AUTO_FIGURES_KEY => super::auto_figures::cell(
                style.scan_book.get(r.provider, &r.market),
                style.scan_running,
                &style.scan_labels,
            ),
            "pos" => {
                if acct.pos != 0.0 {
                    MoonDataCell::text(num(acct.pos))
                } else {
                    MoonDataCell::text("0").tone(MoonTone::Muted)
                }
            }
            _ => MoonDataCell::text(""),
        })
        .collect();
    MoonDataRow::new(cells)
}

/// Keep the visible lead's copy identity alongside the chart member and group menu scope.
pub(super) struct MarketTarget {
    lead_core: CoreId,
    core: CoreId,
    market: String,
    coin: String,
    members: Option<Vec<CoreId>>,
}

impl MarketTarget {
    /// Capture the clicked row's lead and resolve navigation and blacklist targets.
    pub(super) fn new(row: &SelectedRow, entries: &[Entry]) -> Self {
        let lead = &entries[row.lead].row;
        let chart = &row.chart_entry(entries).row;
        Self {
            lead_core: lead.core,
            core: chart.core,
            market: chart.market.clone(),
            coin: chart.coin.clone(),
            members: row.group.as_ref().map(|group| {
                group
                    .members
                    .iter()
                    .map(|&index| entries[index].row.core)
                    .collect()
            }),
        }
    }

    /// Limit grouped menu actions to cores represented by the clicked row.
    pub(super) fn admits_core(&self, core: CoreId) -> bool {
        self.members
            .as_ref()
            .is_none_or(|members| members.contains(&core))
    }

    /// Identify the displayed row for copy, even when a different member owns the chart.
    pub(super) fn copy_identity(&self) -> (CoreId, &str) {
        (self.lead_core, &self.market)
    }
}

/// Open the shared menu for the chart member, intersecting group members with the live scope.
fn open_screener_coin_menu(
    target: &MarketTarget,
    view: &Entity<ScreenerView>,
    pos: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    app.stop_propagation();
    let core = target.core;
    let panel = view.read(app);
    let backend = panel.backend.clone();
    let (selected_cores, core_name) = {
        let b = backend.read(app);
        let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton);
        let sessions = b.session.sessions();
        let venues = b.session.core_venues();
        let selected: Vec<CoreId> = sessions
            .iter()
            .map(|session| session.id)
            .filter(|&id| b.core_displayed(preset, id) && panel.source_admits(id, venues))
            .filter(|&id| target.admits_core(id))
            .collect();
        let name = sessions
            .iter()
            .find(|session| session.id == core)
            .map(|session| session.name.clone())
            .unwrap_or_default();
        (selected, name)
    };
    let screener_copy = super::coin_copy::copy_items(panel.coin_lists(target.copy_identity()));
    let ctx = CoinMenuCtx {
        core,
        core_name,
        market: target.market.clone(),
        coin: target.coin.clone(),
        selected_cores,
        strat_id: None,
        strat_name: None,
        order_uid: None,
        workspace_group: None,
        side: None,
        short: false,
        origin: CoinMenuOrigin::OrderTable,
        history: None,
        report: None,
        screener_copy,
    };
    crate::controls::open_coin_menu(ctx, backend, pos, window, app);
}

/// Open the preferred member's chart on click and the group's menu on right click.
fn market_cell(
    row: &SelectedRow,
    prepared: &Arc<Prepared>,
    view: &WeakEntity<ScreenerView>,
    p: MoonPalette,
) -> MoonDataCell {
    let lead = &prepared.entries[row.lead].row;
    let id = SharedString::from(format!("scr-mkt-{}-{}", lead.core, lead.market));
    let label = SharedString::from(lead.coin.as_str());
    let row = row.clone();
    let prepared = prepared.clone();
    let view = view.clone();
    let row_menu = row.clone();
    let prepared_menu = prepared.clone();
    let view_menu = view.clone();
    MoonDataCell::element(
        div()
            .id(id)
            .min_w_0()
            .w_full()
            .h_full()
            .flex()
            .items_center()
            .cursor_pointer()
            .text_color(rgb(MoonTone::Accent.color(p)))
            .child(div().min_w_0().truncate().child(label.clone()))
            .on_click(move |_, _window, app| {
                let chart = &row.chart_entry(&prepared.entries).row;
                let _ = view.update(app, |this, cx| {
                    this.open_on_main(chart.core, chart.market.clone(), cx);
                });
            })
            .on_mouse_down(
                MouseButton::Right,
                move |event: &MouseDownEvent, window, app| {
                    if let Some(view) = view_menu.upgrade() {
                        let target = MarketTarget::new(&row_menu, &prepared_menu.entries);
                        open_screener_coin_menu(&target, &view, event.position, window, app);
                    }
                },
            ),
    )
    .weight(500.0)
    .overflow_tooltip(label)
}

#[cfg(test)]
pub(super) mod tests;
