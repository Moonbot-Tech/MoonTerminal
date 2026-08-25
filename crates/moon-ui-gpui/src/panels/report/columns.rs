//! Report table columns, cells, and headers: column descriptors, DB-value display formatting,
//! raw-schema titles, and base widths.

use super::*;
use crate::controls::{CoinMenuCtx, CoinMenuOrigin};
use moon_core::db::{QuoteCurrency, ReportAxis};
use rust_i18n::t;

/// Report column that is redundant when a group-owned Auto workspace selects one core.
const CORE_NAME_COLUMN: &str = "core_name";

/// Exact Report row identity needed to open its chart and durable history.
#[derive(Clone)]
pub(super) struct ReportCoinTarget {
    /// Core that recorded the displayed row.
    pub(super) core_uid: u64,
    /// Shared published filter snapshot, copied into an owned request only on activation.
    pub(super) published_filter: Arc<ReportFilter>,
    /// Stable clicked-row identity used for the initial chart focus.
    pub(super) focus_record_id: Option<i64>,
}

/// Report-only context surrounding one row's shared coin menu.
pub(super) struct ReportCoinMenuScope {
    /// Exact row target and published query identity.
    pub(super) target: ReportCoinTarget,
    /// Cores represented by bulk menu actions in the current panel scope.
    pub(super) selected_cores: Vec<u64>,
    /// Group that owns workspace navigation, or `None` for standalone Report.
    pub(super) workspace_group: Option<String>,
}

/// Return whether one runtime column is available in the current display context.
///
/// Args:
///     column: Runtime Report column name.
///     hide_core_name: Whether the group-owned AutoCore lens suppresses `core_name`.
///
/// Returns:
///     `false` only for contextually hidden `core_name`; raw saved preferences are untouched.
pub(super) fn column_is_available(column: &str, hide_core_name: bool) -> bool {
    !(hide_core_name && column == CORE_NAME_COLUMN)
}

/// Iterate runtime columns available in the current display context.
///
/// Args:
///     cols: Complete runtime Report schema.
///     hide_core_name: Whether the group-owned AutoCore lens suppresses `core_name`.
///
/// Returns:
///     Source index and column reference pairs in runtime-schema order.
pub(super) fn available_columns(
    cols: &[String],
    hide_core_name: bool,
) -> impl Iterator<Item = (usize, &String)> {
    cols.iter()
        .enumerate()
        .filter(move |(_, column)| column_is_available(column, hide_core_name))
}

/// Iterate saved-visible runtime columns after applying the contextual display lens.
///
/// Args:
///     cols: Complete runtime Report schema.
///     visible: Raw user-saved visible-column names.
///     hide_core_name: Whether the group-owned AutoCore lens suppresses `core_name`.
///
/// Returns:
///     Effectively visible source index and column reference pairs in runtime-schema order.
pub(super) fn effective_visible_columns<'a>(
    cols: &'a [String],
    visible: &'a HashSet<String>,
    hide_core_name: bool,
) -> impl Iterator<Item = (usize, &'a String)> + 'a {
    available_columns(cols, hide_core_name).filter(|(_, column)| visible.contains(column.as_str()))
}

/// Return whether every contextually available runtime column is saved as visible.
///
/// Args:
///     cols: Complete runtime Report schema.
///     visible: Raw user-saved visible-column names.
///     hide_core_name: Whether the group-owned AutoCore lens suppresses `core_name`.
///
/// Returns:
///     `true` only when at least one column is available and all available columns are visible.
pub(super) fn all_available_columns_visible(
    cols: &[String],
    visible: &HashSet<String>,
    hide_core_name: bool,
) -> bool {
    let mut available = available_columns(cols, hide_core_name).map(|(_, column)| column);
    available
        .next()
        .is_some_and(|first| visible.contains(first.as_str()))
        && available.all(|column| visible.contains(column.as_str()))
}

/// Toggle all contextually available columns while preserving unavailable saved preferences.
///
/// Args:
///     cols: Complete runtime Report schema.
///     visible: Raw user-saved visible-column names.
///     hide_core_name: Whether the group-owned AutoCore lens suppresses `core_name`.
///
/// Returns:
///     Replacement saved set. Turning an all-on context off retains its first available column;
///     unavailable columns such as dormant `core_name` are copied through unchanged.
pub(super) fn toggled_all_columns(
    cols: &[String],
    visible: &HashSet<String>,
    hide_core_name: bool,
) -> HashSet<String> {
    let all_on = all_available_columns_visible(cols, visible, hide_core_name);
    let mut next = visible.clone();
    if all_on {
        let mut available = available_columns(cols, hide_core_name).map(|(_, column)| column);
        let Some(first) = available.next() else {
            return next;
        };
        next.remove(first.as_str());
        for column in available {
            next.remove(column.as_str());
        }
        next.insert(first.clone());
    } else {
        next.extend(available_columns(cols, hide_core_name).map(|(_, column)| column.clone()));
    }
    next
}

/// Build sortable table descriptors for visible indices in the cached schema.
///
/// Args:
///     cols: Complete runtime Report schema.
///     vis: Visible source-column indices in render order.
///     natural_widths: Content-derived bases keyed by column name.
///
/// Returns:
///     Sortable descriptors aligned and sized for the current result.
pub(super) fn report_columns(
    cols: &[String],
    vis: &[usize],
    natural_widths: &std::collections::HashMap<String, f32>,
) -> Vec<MoonDataTableColumn> {
    vis.iter()
        .map(|&i| {
            let col = cols[i].as_str();
            let width = natural_widths
                .get(col)
                .copied()
                .unwrap_or_else(|| width_for(col));
            let column =
                MoonDataTableColumn::new(col.to_string(), header_for(col), width).sortable(true);
            if is_numeric_report_column(col) {
                column.right()
            } else {
                column
            }
        })
        .collect()
}

/// Build one report row; missing values render empty while coin and core cells retain actions.
///
/// Args:
///     ri: Visible source-row index.
///     cols: Runtime report schema in source order.
///     data: Current report result containing the row and core identity.
///     vis: Visible source-column indices in render order.
///     backend: Shared backend used by interactive cells.
///     view: Owning Report panel entity.
///     selected: Whether the controlled selection contains this row.
///     p: Active Moon palette.
///     axis: Time axis for REPLICATED timestamp columns, whose stored seconds are the core's own
///         wall clock rather than UTC.
///     display_zone: User-selected zone for columns this terminal wrote itself.
///
/// Returns:
///     A MoonDataTable row whose highlight mirrors the controlled selection.
#[allow(clippy::too_many_arguments)]
pub(super) fn report_data_row(
    ri: usize,
    cols: &[String],
    data: &ReportData,
    vis: &[usize],
    backend: &Entity<Backend>,
    view: &Entity<ReportPanel>,
    selected: bool,
    p: MoonPalette,
    axis: &ReportAxis,
    display_zone: Tz,
) -> MoonDataRow {
    let mut cells = Vec::with_capacity(vis.len());
    if let Some(r) = data.rows.get(ri) {
        let core_uid = data.core_uids.get(ri).copied().unwrap_or(0);
        // Resolved from the whole row, not from the visible set: money precision must not change
        // because the user hid the currency column.
        let quote = row_quote(cols, r);
        for &i in vis {
            let cname = cols[i].as_str();
            let val = r.get(i).unwrap_or(&Value::Null);
            if cname == "coin" {
                let focus_record_id = data.row_keys.get(ri).and_then(|key| match key {
                    Some(selection::ReportRowKey::Replicated { rec_id, .. })
                    | Some(selection::ReportRowKey::Legacy { db_id: rec_id, .. }) => Some(*rec_id),
                    None => None,
                });
                cells.push(coin_cell(
                    ri,
                    val,
                    ReportCoinTarget {
                        core_uid,
                        published_filter: data.filter.clone(),
                        focus_record_id,
                    },
                    backend,
                    view,
                    p,
                ));
            } else if cname == "core_name" {
                cells.push(core_cell(ri, val, core_uid, view, p));
            } else if cname == "deleted" {
                cells.push(deleted_cell(ri, val));
            } else {
                cells.push(report_data_cell(ri, cname, val, quote, p, axis, core_uid, display_zone));
            }
        }
    }
    MoonDataRow::new(cells).selected(selected)
}

/// Build the shared coin-menu context for one report row.
///
/// The row, not the coin cell, is what the menu now belongs to: every cell opens the same one, so
/// the context is resolved once per right-click from the row's values. The token written into the
/// coin blacklists is read with the CORE's own exchange rules, because the core matches it against
/// that core's `market_currency`.
///
/// Args:
///     values: The row's cell values, parallel to `cols`.
///     cols: Runtime report schema in source order.
///     scope: Exact row target plus menu selection and workspace authority.
///     trailing: Entries only the Report can build, appended after the shared ones.
///     backend: Shared backend read for market, core, and strategy names.
///     cx: Application context.
///
/// Returns:
///     The context to hand to [`crate::controls::open_coin_menu`].
pub(super) fn row_coin_menu_ctx(
    values: &[Value],
    cols: &[String],
    scope: ReportCoinMenuScope,
    trailing: Vec<MoonMenuItem>,
    backend: &Entity<Backend>,
    cx: &App,
) -> CoinMenuCtx {
    let ReportCoinMenuScope {
        target,
        selected_cores,
        workspace_group,
    } = scope;
    let core_uid = target.core_uid;
    let column = |name: &str| {
        cols.iter()
            .position(|col| col == name)
            .and_then(|ix| values.get(ix))
    };
    let coin = column("coin").map(value_to_string).unwrap_or_default();
    let strat_id = column("strategyid")
        .and_then(|value| match value {
            Value::Integer(id) => Some(*id as u64),
            _ => None,
        })
        .filter(|id| *id != 0);
    let b = backend.read(cx);
    // An empty coin still yields a context: the shared menu drops its token actions and the row
    // keeps whatever the caller appended.
    let market = if coin.is_empty() {
        String::new()
    } else {
        resolve_market(b, core_uid, &coin).unwrap_or_default()
    };
    let coin_base = if market.is_empty() {
        String::new()
    } else {
        b.session
            .market_source()
            .market_label(core_uid, &market)
            .coin
    };
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
    CoinMenuCtx {
        core: core_uid,
        core_name,
        market,
        coin: coin_base,
        selected_cores,
        strat_id,
        strat_name,
        order_uid: None,
        workspace_group,
        side: None,
        short: false,
        origin: CoinMenuOrigin::OrderTable,
        history: Some(report_chart_history(
            (*target.published_filter).clone(),
            coin,
            target.focus_record_id,
        )),
        trailing,
    }
}

/// Build a full-cell clickable coin cell.
///
/// Left-click opens the resolved market on the transaction's core in Main without activating Main.
/// The right-click menu is not here: it belongs to the ROW, so that every cell opens the same one.
///
/// Args:
///     ri: Visible row index used to make the cell identity stable.
///     val: Report coin value rendered by this cell.
///     target: Exact core, published filter, and stable row identity.
///     backend: Shared backend used to resolve and queue the market.
///     view: Owning Report panel used to distinguish group and standalone authority.
///     p: Active Moon palette.
///
/// Returns:
///     A clickable data cell whose delayed navigation revalidates the current Auto scope.
fn coin_cell(
    ri: usize,
    val: &Value,
    target: ReportCoinTarget,
    backend: &Entity<Backend>,
    view: &Entity<ReportPanel>,
    p: MoonPalette,
) -> MoonDataCell {
    let core_uid = target.core_uid;
    let published_filter = target.published_filter;
    let focus_record_id = target.focus_record_id;
    let coin = value_to_string(val);
    let backend = backend.clone();
    let view = view.clone();
    let el = div()
        .id(SharedString::from(format!("rep-coin-{ri}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        // Tone and weight make the scanned coin column the row's visual anchor.
        .text_color(rgb(MoonTone::Accent.color(p)))
        .font_weight(FontWeight::BOLD)
        .child(coin.clone())
        .on_click(move |_, window, app| {
            // A Shift or Ctrl click is a selection gesture wherever it lands, so let those bubble
            // to the row handler. A plain click opens the chart and stops there: otherwise it would
            // also reach the row handler, where clicking the sole selected row deselects it.
            // Stopped before the empty check, so a blank coin cell behaves like a filled one.
            let modifiers = window.modifiers();
            if modifiers.shift || modifiers.secondary() {
                return;
            }
            app.stop_propagation();
            if coin.is_empty() {
                return;
            }
            // The report DB may store `coin` as a base (`M`) or a full market (`VINEUSDT`).
            // The chart expects an exact full key, so resolve it using the core quote and
            // market universe.
            let market = backend.read(app);
            let Some(market) = resolve_market(market, core_uid, &coin) else {
                window.push_notification(
                    MoonNotification::warning(t!("coin_menu.view_trades_not_ready").to_string()),
                    app,
                );
                return;
            };
            let workspace_group = {
                let panel = view.read(app);
                (!panel.standalone).then(|| panel.group.clone())
            };
            backend.update(app, |b, bcx| {
                if b.open_report_on_main_if_authorized(
                    workspace_group.as_deref(),
                    (core_uid, market.clone()),
                    report_chart_history(
                        (*published_filter).clone(),
                        coin.clone(),
                        focus_record_id,
                    ),
                    false,
                ) {
                    bcx.notify();
                }
            });
        });
    MoonDataCell::element(el)
}

/// Build the Report-only history intent shared by direct coin clicks and row context menus.
///
/// Args:
///     filter: Published filter that produced the displayed row.
///     exact_coin: Raw historical coin identity from that row.
///     focus_record_id: Stable row identity for initial viewport focus.
///
/// Returns:
///     Report-refined history scope for the shared atomic Main request.
fn report_chart_history(
    filter: ReportFilter,
    exact_coin: String,
    focus_record_id: Option<i64>,
) -> crate::backend::ChartHistoryScope {
    crate::backend::ChartHistoryScope::Report {
        filter,
        exact_coin,
        focus_record_id,
    }
}

/// Build a market candidate for the core from the DB `coin` value.
///
/// Supports both historical formats: a base (`M`) and an already complete market (`MUSDT`).
/// A base is spelled into a market by `symbol::parse::market_names_for`, which knows each
/// exchange's form; this used to concatenate coin and quote by hand and therefore proposed a
/// market that exists on no Gate, OKX or Hyperliquid core. The selected core's catalog must verify
/// the result; an empty universe returns `None` instead of opening a clean chart for a guessed key.
///
/// Args:
///     b: Backend containing the exact core's configuration and market catalog.
///     core: Core that recorded the Report row.
///     coin: Historical base coin or full market value from that row.
///
/// Returns:
///     Catalog-verified market, or `None` while the selected core cannot resolve it.
pub(super) fn resolve_market(b: &Backend, core: u64, coin: &str) -> Option<String> {
    let exchange = b.session.market_source().exchange_of(core);
    let quote = b
        .config
        .servers
        .iter()
        .find(|s| s.id == core)
        .map(|s| moon_core::symbol::resolve_quote_on(&s.market, exchange))
        .unwrap_or_default();
    // A complete market is one carrying a HIP-3 DEX prefix or THIS CORE'S quote. Deliberately not
    // "carries any recognized quote": a coin whose own name ends in one — `WBTC`, `STETH`,
    // `PYUSD` — would then be taken for a finished market and never get its quote appended.
    let parsed = moon_core::symbol::parse::split_market(coin, exchange);
    let already_full =
        parsed.dex.is_some() || (!quote.is_empty() && parsed.quote.eq_ignore_ascii_case(&quote));
    let candidate = if already_full || quote.is_empty() {
        coin.to_string()
    } else {
        moon_core::symbol::parse::market_names_for(coin, &quote, exchange)
            .next()
            .unwrap_or_else(|| coin.to_string())
    };
    // An empty universe is catalog-not-ready, never permission to open an unverified key.
    let ms = b.session.market_source();
    let universe = ms.search_markets(core, coin, 32);
    if universe.is_empty() {
        return None;
    }
    // Ask the CATALOG which of these markets is this coin. The report stores the core's own
    // token, and for a folded one — `1kRATS` for the market `1000RATSUSDT` — no reading of the
    // market name can connect the two, so this used to fall through to a spelled candidate that
    // exists nowhere and opened an empty chart.
    let refs: Vec<&str> = universe.iter().map(String::as_str).collect();
    let labelled: Vec<(String, moon_core::market::MarketLabel)> = universe
        .iter()
        .cloned()
        .zip(ms.market_labels(core, &refs))
        .collect();
    if let Some(name) = moon_core::market::pick_market_for_coin(&labelled, coin) {
        return Some(name.to_string());
    }
    // The historical format where the stored value is already a full market name.
    universe
        .iter()
        .find(|market| market.eq_ignore_ascii_case(&candidate))
        .cloned()
}

/// Build a full-cell core cell with the shared muted tone.
///
/// In standalone or Classic mode, clicking filters to this core and a repeat click clears the sole
/// selection. In group Auto mode, clicking is ignored because only the Shell rail selects cores;
/// the retained Classic filter is never changed.
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
        .on_click(move |_, window, app| {
            // Changing the Classic filter, or consuming an ignored Auto shortcut, is the whole
            // gesture; see the coin cell above for why a plain click must not also reach the row
            // handler, and why a modified one must.
            let modifiers = window.modifiers();
            if modifiers.shift || modifiers.secondary() {
                return;
            }
            app.stop_propagation();
            view.update(app, |t, c| t.filter_to_core(core_uid, c));
        });
    MoonDataCell::element(el)
}

/// The `deleted` soft-delete flag as a checkbox.
///
/// The checkbox is display-only; row mutation belongs to the controlled commands in the totals row.
///
/// Args:
///     ri: Visible row index used to give the checkbox a stable element id.
///     val: Database value whose nonzero integer state means deleted.
///
/// Returns:
///     A disabled checkbox cell that communicates state without offering inline mutation.
fn deleted_cell(ri: usize, val: &Value) -> MoonDataCell {
    let checked = as_i64(val).unwrap_or(0) != 0;
    let cb = MoonCheckbox::new(SharedString::from(format!("rep-del-{ri}")))
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .disabled(true);
    MoonDataCell::element(
        div()
            .w_full()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .child(cb),
    )
}

/// Render one Report value with column-specific formatting and alignment.
///
/// Args:
///     row: Visible row index used to give the hover target a stable identity.
///     col: Runtime report column name.
///     val: SQLite value from the row.
///     quote: The row's quote currency, deciding money precision.
///     p: Active MoonUI palette.
///     zone: Selected IANA display zone for timestamp columns.
///
/// Returns:
///     Clipped table cell ready for MoonDataTable.
fn report_data_cell(
    row: usize,
    col: &str,
    val: &Value,
    quote: Option<QuoteCurrency>,
    p: MoonPalette,
    axis: &ReportAxis,
    core_uid: u64,
    display_zone: Tz,
) -> MoonDataCell {
    let (text, color) = cell(col, val, quote, p, axis, core_uid, display_zone);
    // Clip formatted content to the column's actual width. Alignment matches the column, while
    // MoonDataTable also protects cell boundaries at the container level. Every other column's
    // font styling comes from the cell style through MoonUI cascading.
    let right = is_numeric_report_column(col);
    let color = color.unwrap_or_else(|| MoonTone::Default.color(p));
    let tooltip = cell_tooltip(col, &text);
    let inner = div()
        .id(SharedString::from(format!("report-cell-{row}-{col}")))
        .flex()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .when(right, |d| d.justify_end())
        .text_color(rgb(color))
        .font_weight(cell_weight(col))
        .child(text)
        .when_some(tooltip, |cell, tip| {
            cell.tooltip(crate::panels::common::text_tooltip(tip))
        });
    MoonDataCell::element(inner)
}

/// Preserve complete historical-rate provenance when the compact source column clips it.
///
/// Args:
///     col: Runtime report column name.
///     text: Fully formatted cell text.
///
/// Returns:
///     Full source text only for a non-empty valuation provenance cell.
fn cell_tooltip(col: &str, text: &str) -> Option<String> {
    (col == db::VALUATION_SOURCE_COLUMN && !text.is_empty()).then(|| text.to_string())
}

/// Return the font weight a GENERIC Report data cell's text is drawn — and MEASURED — with.
///
/// The two profit columns carry the number the table exists to be read for, so they are weighted
/// above the rest of the row. Their sibling `profitbtc`/`gainedbtc` share the same sign colouring
/// but not the weight: weighting every numeric column would flatten the emphasis back out.
///
/// The coin column is deliberately not part of this rule. It renders through the specialized
/// `coin_cell` element at `BOLD`; this predicate governs generic Report cells and the corresponding
/// natural-width measurement change only.
///
/// `SEMIBOLD` specifically, not `MEDIUM` or `BOLD`: `design::MonoBodyFontSignature` encodes only the
/// normal and semibold `FontId`s, and that signature keys the natural-width cache. A third weight's
/// resolved font would sit outside the key, so a theme change altering only that weight's resolution
/// would leave stale widths cached with nothing to invalidate them.
///
/// Args:
///     col: Runtime report column name.
///
/// Returns:
///     The weight for that column's body text.
pub(super) fn cell_weight(col: &str) -> FontWeight {
    match col {
        db::VALUATION_PROFIT_COLUMN | db::PROFIT_PERCENT_COLUMN => FontWeight::SEMIBOLD,
        _ => FontWeight::NORMAL,
    }
}

/// Return whether a Report column uses right-aligned numeric presentation.
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
            | "profitpct"
            | "valuation_profit_usdt"
            | "valuation_rate"
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
///
/// Args:
///     col: Runtime Report column key.
///     v: Database value to format.
///     quote: The row's quote currency, which decides money precision; `None` when the row does not
///         name one, and the cell then falls back to two decimals.
///     p: Active palette used for signed coloring.
///     axis: Time axis for REPLICATED columns, whose stored seconds are the core's own wall clock.
///     core_uid: The row's owning core. The axis is per-core, so dropping this would silently
///         convert every row by whatever the first core's offset happened to be.
///     display_zone: User-selected zone for columns this terminal wrote itself, which are already
///         true UTC and must not travel through the axis.
///
/// Returns:
///     Display text and optional text color.
pub(super) fn cell(
    col: &str,
    v: &Value,
    quote: Option<QuoteCurrency>,
    p: MoonPalette,
    axis: &ReportAxis,
    core_uid: u64,
    display_zone: Tz,
) -> (String, Option<u32>) {
    match col {
        // Replicated from the core, on the core's clock: the axis owns both halves of the
        // projection, so the selected zone must NOT be applied on top of it.
        "buydate" | "closedate" | "sellsetdate" => (
            as_i64(v)
                .map(|secs| {
                    let utc = axis.to_utc(secs, core_uid);
                    moon_core::util::display_time::format_minute(utc, axis.zone())
                })
                .unwrap_or_default(),
            None,
        ),
        // Written by THIS terminal as a freshness marker (see `strat_db::stats`), so it is genuine
        // UTC and belongs in the user's selected zone like any other local timestamp.
        "last_update_at" => (
            as_i64(v)
                .map(|secs| moon_core::util::display_time::format_minute(secs, display_zone))
                .unwrap_or_default(),
            None,
        ),
        "isshort" => match as_i64(v) {
            Some(1) => (t!("report.side.short").to_string(), Some(p.red)),
            Some(0) => (t!("report.side.long").to_string(), Some(p.green)),
            _ => (String::new(), Some(p.text_soft)),
        },
        "emulator" => match as_i64(v) {
            Some(1) => (t!("report.cell.emu").to_string(), Some(p.text_soft)),
            _ => (String::new(), None),
        },
        "basecurrency" => (basecurrency_text(v), None),
        // `valuation_rate` deliberately has no arm: applied rates span roughly 1e5 (BTC) down to
        // 1e-2 (IDR), and the generic numeric path already answers that with eight significant
        // digits, where the two decimals the profit cells use would flatten most rates to `0.00`.
        "profitbtc" | "gainedbtc" | "profitpct" | "valuation_profit_usdt" => {
            let n = as_f64(v);
            let color = match n {
                Some(x) if x > 0.0 => Some(p.green),
                Some(x) if x < 0.0 => Some(p.red),
                _ => None,
            };
            // Profit cells carry no currency marker — the totals row owns it for the aggregate —
            // so precision comes from the ROW's quote, exactly as the totals resolve theirs. Two
            // decimals on every column would print a whole BTC-denominated core as `-0.00`: those
            // rows live in the fourth decimal and below (`-0.00041380`), which is the precision
            // MoonBot's own report shows them at. Percent and the USDT conversion are fixed at two:
            // one is a ratio, the other is always USDT whatever the row is denominated in.
            let decimals = match col {
                "profitpct" | db::VALUATION_PROFIT_COLUMN => 2,
                _ => quote.map_or(2, |currency| currency.display_decimals()),
            };
            let text = n
                .map(|x| {
                    if col == "profitpct" {
                        format!("{x:+.2}%")
                    } else {
                        format!("{x:+.decimals$}")
                    }
                })
                .unwrap_or_default();
            (text, color)
        }
        _ => (cell_display_text(v), None),
    }
}

/// Resolve the quote currency one report row's money is denominated in.
///
/// The read layer already publishes the EFFECTIVE ordinal in this column, so a COIN-M row arrives
/// as BTC and this stays a plain decode rather than a second place stating the correction.
///
/// Args:
///     cols: Complete runtime Report schema, in row order.
///     row: The row's values, parallel to `cols`.
///
/// Returns:
///     The row's currency, or `None` when the schema omits it or the value is untrusted.
pub(super) fn row_quote(cols: &[String], row: &[Value]) -> Option<QuoteCurrency> {
    let index = cols.iter().position(|column| column == "basecurrency")?;
    QuoteCurrency::from_report_value(row.get(index)?)
}

/// Resolve one row's owning core from the runtime schema.
///
/// Width measurement formats through the very same path the renderer does, so it must resolve the
/// same core: measuring a row under a different offset would size the column for text that is
/// never painted. A source that cannot name a core yields `0`, matching the axis's own treatment
/// of an unidentifiable row.
///
/// Args:
///     cols: Complete runtime Report schema.
///     row: One result row in schema order.
///
/// Returns:
///     The row's core uid, or `0` when the schema carries none.
pub(super) fn row_core_uid(cols: &[String], row: &[Value]) -> u64 {
    cols.iter()
        .position(|column| column == "core_uid")
        .and_then(|index| row.get(index))
        .and_then(as_i64)
        .unwrap_or(0) as u64
}

/// Format a persisted MoonBot base-currency ordinal as its exact quote ticker.
///
/// Args:
///     value: Runtime Report value carrying the persisted ordinal.
///
/// Returns:
///     Known quote ticker, or the original cell text when the identity is untrusted.
fn basecurrency_text(value: &Value) -> String {
    moon_core::db::QuoteCurrency::from_report_value(value)
        .map(|currency| currency.ticker().to_string())
        .unwrap_or_else(|| cell_display_text(value))
}

/// Reads a database value as a whole number, or `None` when it holds something else.
///
/// A core writes identifiers and dates as integers, but a value can arrive as a real from an older
/// or hand-repaired database. Text is NOT accepted here: the cells this serves render a text value
/// as text on purpose, and only the trade log wants a text-stored number read as one — it parses at
/// its own call site.
pub(super) fn as_i64(v: &Value) -> Option<i64> {
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
pub(super) fn value_to_string(v: &Value) -> String {
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
        "profitpct" => "profit %".to_string(),
        "spentbtc" => "spent".to_string(),
        "gainedbtc" => "gained".to_string(),
        "valuation_profit_usdt" => "profit USDT".to_string(),
        "valuation_rate" => "rate".to_string(),
        "valuation_rate_source" => "rate src".to_string(),
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
        "profitbtc"
        | "profitpct"
        | "gainedbtc"
        | "spentbtc"
        | "valuation_profit_usdt"
        | "valuation_rate" => 96.0,
        "valuation_rate_source" => 130.0,
        "lev" | "isshort" | "emulator" => 52.0,
        _ => 82.0,
    }
}

#[cfg(test)]
mod tests;
