//! Screener table columns and rows: schema, value formatting, sorting, and `MoonDataRow` rendering.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonDataCell, MoonDataRow, MoonPalette, MoonTone};
use rust_i18n::t;

use moon_core::market::SharedScreenerRow;
use moon_core::session::CoreId;
use moon_core::util::fmt::{self, DeltaSign};

use crate::controls::{CoinMenuCtx, CoinMenuOrigin};
use crate::panels::num;

use super::auto_figures::{AUTO_FIGURES_KEY, ScanBook, ScanLabels};
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
    ("core", "Core", 76.0, false),
    // Three columns about WHERE the row is, none of them Moonbot's — Moonbot shows one exchange,
    // so it has nothing to say here. `exchange` is the venue caption every core list uses, `type`
    // the market kind of that venue, and `listed` whether the coin trades on both kinds of this
    // exchange or on this one alone, in the words of Moonbot's `ListedType` strategy filter.
    (EXCHANGE_KEY, "Exchange", 124.0, false),
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

/// Columns added after users could already hold a stored visible set, each with the migration
/// version that introduced it, oldest first.
///
/// The version is what lets a later migration add ITS columns without re-adding an earlier one:
/// a layout already at version 1 has had its chance at `coin`, and a user who hid it since must
/// not get it back because three more columns arrived.
const COLS_ADDED_LATE: &[(u32, &str)] = &[
    (1, COIN_KEY),
    (2, EXCHANGE_KEY),
    (2, TYPE_KEY),
    (2, LISTED_KEY),
    // A display feature ships ON (CONTRIBUTING § UI): a stored set that predates the column is a
    // set with the value ABSENT, not one where the user turned it off, so it is offered once like
    // every late column — and a user who hides it afterwards keeps it hidden.
    (3, AUTO_FIGURES_KEY),
];

/// The migration version a layout is at once every late column has been offered to it.
pub(super) const COLUMNS_MIGRATION: u32 = 3;

/// Add the columns a stored visible set predates.
///
/// A stored set is an explicit list, so a column introduced later is simply missing from it and
/// would stay hidden for everyone who ever arranged their columns — the users most likely to want
/// it. The caller runs this once per layout version and records that it did
/// (`WindowLayout::screener_columns_migration`): run on every open, it would put back a column the
/// user has since hidden.
///
/// Args:
///     keys: One stored visible set, in any order.
///     applied: The migration version the layout already records; zero for never migrated.
///
/// Returns:
///     Nothing; `keys` gains the columns introduced after `applied`. An empty set is left alone:
///     it is not a choice a user can make, and filling it would turn "all columns" into "only the
///     new ones".
pub(super) fn add_new_columns(keys: &mut Vec<String>, applied: u32) {
    if keys.is_empty() {
        return;
    }
    for (version, column) in COLS_ADDED_LATE {
        if *version > applied && !keys.iter().any(|key| key == column) {
            keys.push((*column).to_string());
        }
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

/// The venue caption with its brand logo, clipped rather than wrapped in a narrow column.
fn exchange_cell(venue: &EntryVenue, style: &RowStyle) -> impl IntoElement + 'static {
    let logo = style
        .logos_ready
        .then_some(venue.brand)
        .flatten()
        .and_then(crate::media::exchange_logos::exchange_logo);
    let (logo_px, logo_gap) = (style.logo_px, style.logo_gap);
    div()
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .gap(logo_gap)
        .when_some(logo, |cell, logo| {
            cell.child(img(logo).flex_none().w(logo_px).h(logo_px))
        })
        .child(div().min_w_0().truncate().child(venue.label.clone()))
}

pub(super) fn screener_row(
    e: &Entry,
    view: &Entity<ScreenerView>,
    p: MoonPalette,
    cols: &[&ColDef],
    style: &RowStyle,
) -> MoonDataRow {
    use moon_core::market::listing::Listing;
    let r = &e.row;
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
            "market" => MoonDataCell::element(market_cell(e, view, p)),
            COIN_KEY => match r.identity.is_empty() {
                true => MoonDataCell::text("—").tone(MoonTone::Muted),
                false => MoonDataCell::text(r.identity.clone()),
            },
            "core" => MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
            EXCHANGE_KEY => match e.venue.label.is_empty() {
                true => MoonDataCell::text("—").tone(MoonTone::Muted),
                false => MoonDataCell::element(exchange_cell(&e.venue, style)),
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
            AUTO_FIGURES_KEY => super::auto_figures::cell(
                style.scan_book.get(r.provider, &r.market),
                style.scan_running,
                &style.scan_labels,
            ),
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

/// Open the shared coin context menu from a Screener row's market right-click.
///
/// A Screener row carries no strategy and no order, so the menu holds navigation and the blacklist
/// entries. Its "N cores" targets are the cores the table can currently show — the Singleton display
/// preset narrowed by the source selector, the filter the rows themselves went through — as the
/// Assets and Orders menus use their own panel filter. The window belongs to no group, so the
/// actions keep the unscoped authority the row's left click already has.
///
/// Args:
///     core: Core that owns the clicked row; its chart and its blacklists are the menu's targets.
///     market: Canonical market name used by the navigation entries.
///     coin: Core-native coin spelling written to blacklist values.
///     view: The Screener view, read for its backend and source selector.
///     pos: Window-coordinate popup position.
///     window: Window that owns the context menu.
///     app: Application context used to read the live scope and open the menu.
///
/// Returns:
///     Nothing; an applicable menu is opened at `pos`.
fn open_screener_coin_menu(
    core: CoreId,
    market: String,
    coin: String,
    view: &Entity<ScreenerView>,
    pos: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    app.stop_propagation();
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
            .collect();
        let name = sessions
            .iter()
            .find(|session| session.id == core)
            .map(|session| session.name.clone())
            .unwrap_or_default();
        (selected, name)
    };
    let ctx = CoinMenuCtx {
        core,
        core_name,
        market,
        coin,
        selected_cores,
        strat_id: None,
        strat_name: None,
        order_uid: None,
        workspace_group: None,
        side: None,
        short: false,
        origin: CoinMenuOrigin::OrderTable,
        history: None,
        trailing: Vec::new(),
        filter_here: None,
    };
    crate::controls::open_coin_menu(ctx, backend, pos, window, app);
}

/// Build a full-cell clickable market cell.
///
/// Clicking asks the ChartTabs group owning `row.core` to open or focus the market in Main and
/// select `Tab::Main`, without raising or focusing that group's OS window. A right click opens the
/// shared coin menu for the row's core and market.
fn market_cell(
    e: &Entry,
    view: &Entity<ScreenerView>,
    p: MoonPalette,
) -> impl IntoElement + 'static {
    let market = e.row.market.clone();
    let core = e.row.core;
    let view = view.clone();
    let view_menu = view.clone();
    let market_menu = market.clone();
    let coin_menu = e.row.coin.clone();
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
        .on_mouse_down(
            MouseButton::Right,
            move |event: &MouseDownEvent, window, app| {
                open_screener_coin_menu(
                    core,
                    market_menu.clone(),
                    coin_menu.clone(),
                    &view_menu,
                    event.position,
                    window,
                    app,
                );
            },
        )
}

#[cfg(test)]
pub(super) mod tests;
