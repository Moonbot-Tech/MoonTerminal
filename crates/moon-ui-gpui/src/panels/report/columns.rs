//! Report table columns, cells, and headers: column descriptors, DB-value display formatting,
//! raw-schema titles, and base widths.

use super::*;
use crate::controls::{CoinMenuCtx, CoinMenuOrigin};
use rust_i18n::t;

/// Build sortable table descriptors for visible indices in the cached schema.
pub(super) fn report_columns(cols: &[String], vis: &[usize]) -> Vec<MoonDataTableColumn> {
    vis.iter()
        .map(|&i| {
            let col = cols[i].as_str();
            let column = MoonDataTableColumn::new(col.to_string(), header_for(col), width_for(col))
                .sortable(true);
            if is_numeric_report_column(col) {
                column.right()
            } else {
                column
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
/// Build one report row; missing values render as empty cells, while coin and core cells retain actions.
pub(super) fn report_data_row(
    ri: usize,
    cols: &[String],
    data: &ReportData,
    vis: &[usize],
    selected_cores: &Rc<Vec<u64>>,
    backend: &Entity<Backend>,
    view: &Entity<ReportPanel>,
    p: MoonPalette,
) -> MoonDataRow {
    let mut cells = Vec::with_capacity(vis.len());
    if let Some(r) = data.rows.get(ri) {
        let core_uid = data.core_uids.get(ri).copied().unwrap_or(0);
        // Read strategyid from all columns, even when hidden. Zero or missing means manual/no
        // strategy and suppresses the strategy section of the context menu.
        let strat_id = cols
            .iter()
            .position(|c| c == "strategyid")
            .and_then(|idx| r.get(idx))
            .and_then(|v| match v {
                Value::Integer(i) => Some(*i as u64),
                _ => None,
            })
            .filter(|id| *id != 0);
        for &i in vis {
            let cname = cols[i].as_str();
            let val = r.get(i).unwrap_or(&Value::Null);
            if cname == "coin" {
                cells.push(coin_cell(
                    ri,
                    val,
                    core_uid,
                    strat_id,
                    selected_cores.clone(),
                    backend,
                    p,
                ));
            } else if cname == "core_name" {
                cells.push(core_cell(ri, val, core_uid, view, p));
            } else {
                cells.push(report_data_cell(cname, val, p));
            }
        }
    }
    MoonDataRow::new(cells)
}

/// Build a full-cell clickable coin cell.
///
/// Left-click opens the resolved market on the transaction's core in Main without activating
/// Main; right-click opens the shared coin context menu.
fn coin_cell(
    ri: usize,
    val: &Value,
    core_uid: u64,
    strat_id: Option<u64>,
    selected_cores: Rc<Vec<u64>>,
    backend: &Entity<Backend>,
    p: MoonPalette,
) -> MoonDataCell {
    let coin = value_to_string(val);
    let backend = backend.clone();
    let backend_menu = backend.clone();
    let coin_menu = coin.clone();
    let el = div()
        .id(SharedString::from(format!("rep-coin-{ri}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        // Inherit font styling from the cell's MoonUI cascade.
        .text_color(rgb(MoonTone::Accent.color(p)))
        .child(coin.clone())
        .on_click(move |_, _window, app| {
            if coin.is_empty() {
                return;
            }
            // The report DB may store `coin` as a base (`M`) or a full market (`VINEUSDT`).
            // The chart expects an exact full key, so resolve it using the core quote and
            // market universe.
            let market = backend.read(app);
            let market = resolve_market(market, core_uid, &coin);
            backend.update(app, |b, bcx| {
                b.open_request = Some((core_uid, market.clone()));
                b.open_request_rev = b.open_request_rev.wrapping_add(1);
                b.open_request_activate = false;
                bcx.notify();
            });
        })
        // Right-click opens the shared coin menu. A nonzero strategyid enables the strategy
        // blacklist, while selected_cores reflects the report's core filter.
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                if coin_menu.is_empty() {
                    return;
                }
                app.stop_propagation();
                let market = {
                    let b = backend_menu.read(app);
                    resolve_market(b, core_uid, &coin_menu)
                };
                let coin_base = moon_core::symbol::coin_of_market(&market).to_string();
                let (core_name, strat_name) = {
                    let b = backend_menu.read(app);
                    let core_name = b
                        .session
                        .sessions()
                        .iter()
                        .find(|s| s.id == core_uid)
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    let strat_name = strat_id.and_then(|sid| {
                        b.session
                            .store()
                            .core(core_uid)
                            .and_then(|cd| cd.strategies.iter().find(|s| s.id == sid))
                            .map(|s| s.name.clone())
                    });
                    (core_name, strat_name)
                };
                let ctx = CoinMenuCtx {
                    core: core_uid,
                    core_name,
                    market,
                    coin: coin_base,
                    selected_cores: (*selected_cores).clone(),
                    strat_id,
                    strat_name,
                    order_uid: None,
                    side: None,
                    short: false,
                    origin: CoinMenuOrigin::OrderTable,
                };
                crate::controls::open_coin_menu(ctx, backend_menu.clone(), e.position, window, app);
            },
        );
    MoonDataCell::element(el)
}

/// Build a market candidate for the core from the DB `coin` value.
///
/// Supports both historical formats: a base (`M`) and an already complete market (`MUSDT`).
/// A base receives the core quote. When the market universe is nonempty, the candidate is
/// validated and may be replaced by a market with the same base; an empty universe leaves the
/// candidate unverified.
fn resolve_market(b: &Backend, core: u64, coin: &str) -> String {
    let quote = b
        .config
        .servers
        .iter()
        .find(|s| s.id == core)
        .map(|s| moon_core::symbol::resolve_quote(&s.market))
        .unwrap_or_default();
    let upper = coin.to_ascii_uppercase();
    // Treat a value ending in the core quote or carrying a HIP-3 DEX prefix (`xyz:BIRD`) as a
    // complete market. HL/HIP-3 names must not receive a quote suffix.
    let already_full = moon_core::symbol::is_hip3(coin)
        || (!quote.is_empty() && upper.len() > quote.len() && upper.ends_with(&quote));
    let candidate = if already_full || quote.is_empty() {
        coin.to_string()
    } else {
        format!("{coin}{quote}")
    };
    // An empty universe returns the candidate without verification. Otherwise accept an exact
    // candidate match or fall back to a market with the same base, including prefixed markets
    // and DEX perpetuals.
    let universe = b.session.market_source().search_markets(core, coin, 32);
    if universe.is_empty() || universe.iter().any(|m| m == &candidate) {
        return candidate;
    }
    universe
        .iter()
        .find(|m| moon_core::symbol::coin_of_market(m).eq_ignore_ascii_case(coin))
        .cloned()
        .unwrap_or(candidate)
}

/// Build a full-cell core cell with the shared muted tone.
///
/// Clicking filters by exactly this core; clicking the sole selected core again clears the
/// filter and shows all cores.
fn core_cell(
    ri: usize,
    val: &Value,
    core_uid: u64,
    view: &Entity<ReportPanel>,
    p: MoonPalette,
) -> MoonDataCell {
    let name = value_to_string(val);
    let view = view.clone();
    let el = div()
        .id(SharedString::from(format!("rep-core-{ri}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(rgb(MoonTone::Muted.color(p)))
        .child(name)
        .on_click(move |_, _window, app| {
            view.update(app, |t, c| t.filter_to_core(core_uid, c));
        });
    MoonDataCell::element(el)
}

fn report_data_cell(col: &str, val: &Value, p: MoonPalette) -> MoonDataCell {
    let (text, color) = cell(col, val, p);
    // Clip formatted content to the column's actual width. Alignment matches the column, while
    // MoonDataTable also protects cell boundaries at the container level. Font styling comes
    // from the cell style through MoonUI cascading.
    let right = is_numeric_report_column(col);
    let color = color.unwrap_or_else(|| MoonTone::Default.color(p));
    let inner = div()
        .flex()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .when(right, |d| d.justify_end())
        .text_color(rgb(color))
        .child(text);
    MoonDataCell::element(inner)
}

fn is_numeric_report_column(col: &str) -> bool {
    matches!(
        col,
        "quantity"
            | "boughtq"
            | "buyprice"
            | "sellprice"
            | "spentbtc"
            | "gainedbtc"
            | "profitbtc"
            | "lev"
            | "id"
            | "newrecid"
            | "taskid"
    ) || col.ends_with("delta")
        || col.ends_with("ratio")
}

/// Formats a database value and optional text color for display in a report cell.
///
/// Generic text values are trimmed and folded to one line. Report exports bypass this
/// presentation formatting and use raw database values.
fn cell(col: &str, v: &Value, p: MoonPalette) -> (String, Option<u32>) {
    match col {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => {
            (as_i64(v).map(db::fmt_unix).unwrap_or_default(), None)
        }
        "isshort" => match as_i64(v) {
            Some(1) => (t!("report.side.short").to_string(), Some(p.red)),
            Some(0) => (t!("report.side.long").to_string(), Some(p.green)),
            _ => (String::new(), Some(p.text_soft)),
        },
        "emulator" => match as_i64(v) {
            Some(1) => (t!("report.cell.emu").to_string(), Some(p.text_soft)),
            _ => (String::new(), None),
        },
        "profitbtc" | "gainedbtc" => {
            let n = as_f64(v);
            let color = match n {
                Some(x) if x > 0.0 => Some(p.green),
                Some(x) if x < 0.0 => Some(p.red),
                _ => None,
            };
            // Profit cells use two decimals without a currency marker; the
            // totals row owns the marker for the aggregate.
            (n.map(|x| format!("{x:+.2}")).unwrap_or_default(), color)
        }
        _ => (cell_display_text(v), None),
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(*i),
        Value::Real(r) => Some(*r as i64),
        _ => None,
    }
}
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Real(r) => Some(*r),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// Converts a database value to a string without display-only text normalization.
///
/// Text is returned verbatim because coin and core callers use it as an identity.
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => moon_core::util::fmt::compact(*r, 8),
        Value::Text(t) => t.clone(),
        Value::Blob(_) => "<blob>".into(),
    }
}

/// Converts a database value to display text for a generic report cell.
///
/// Text values are trimmed and folded because report rows are fixed-height single-line
/// surfaces. Non-text values retain [`value_to_string`] formatting.
fn cell_display_text(v: &Value) -> String {
    match v {
        Value::Text(t) => crate::display_text::flatten_lines(t.trim()),
        other => value_to_string(other),
    }
}

/// Return the raw DB column name as the table and export header, without i18n.
///
/// This makes dynamically added core fields available automatically. The legacy Moonbot
/// `profitbtc`, `spentbtc`, and `gainedbtc` names are the exception: their `btc` suffix is
/// historical, while values are denominated in each row's quote currency. Neutral `profit`,
/// `spent`, and `gained` headers avoid implying BTC on non-BTC pairs.
pub(super) fn header_for(col: &str) -> String {
    match col {
        "profitbtc" => "profit".to_string(),
        "spentbtc" => "spent".to_string(),
        "gainedbtc" => "gained".to_string(),
        _ => col.to_string(),
    }
}

pub(super) fn width_for(col: &str) -> f32 {
    match col {
        "buydate" | "closedate" => 120.0,
        "sellsetdate" | "last_update_at" => 116.0,
        "comment" => 280.0,
        "sellreason" => 170.0,
        "channelname" | "signaltype" | "fname" | "exorderid" => 110.0,
        "core_name" | "coin" => 88.0,
        "profitbtc" | "gainedbtc" | "spentbtc" => 96.0,
        "lev" | "isshort" | "emulator" => 52.0,
        _ => 82.0,
    }
}

#[cfg(test)]
/// Tests for raw identity values and generic report-cell display text.
mod tests;
