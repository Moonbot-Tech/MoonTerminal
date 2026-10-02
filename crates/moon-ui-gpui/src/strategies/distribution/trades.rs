//! The bottom of the "WL distribution" tab: the closed trades of the coin whose chip was clicked,
//! for the selected strategies' names on the cores drawn as rows.
//!
//! Cells are formatted by the Report itself, so a trade reads here exactly as it reads there —
//! dates on the core's clock through the same axis, sides and profits in the same colours.

use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDataTableState,
    MoonPalette, h_flex, v_flex,
};
use rusqlite::types::Value;
use rust_i18n::t;

use super::stats::CoinStat;
use crate::design;
use crate::design::moon;
use crate::panels::{
    is_numeric_report_column, report_cell, report_header_label, report_row_quote, report_width_for,
};
use crate::strategies::StrategiesView;

/// The strategy-name column: not a report column, the rows carry the strategy id only.
const STRATEGY_COL: &str = "strategy";

/// Report columns the table shows, in order, at the Report's fallback widths (`width_for`; the
/// Report itself widens columns to their measured content). A column the replica does not carry
/// is skipped.
const COLUMNS: &[&str] = &[
    "closedate",
    "core_name",
    STRATEGY_COL,
    "isshort",
    "profitpct",
    "valuation_profit_usdt",
    "sellreason",
];

/// Signed profit with its unit, or a dash when the unit is unknown.
///
/// The sign follows the AMOUNT AS PRINTED: a loss too small to show at the unit's precision reads
/// as an unsigned zero, never as "-0.00".
pub(super) fn profit_text(stat: &CoinStat) -> String {
    match stat.currency {
        Some(currency) if stat.profit.is_finite() => {
            let decimals = currency.display_decimals();
            let amount = if decimals == 2 {
                moon_core::util::fmt::group_decimal(&format!("{:.2}", stat.profit.abs()))
            } else {
                moon_core::util::fmt::compact(stat.profit.abs(), decimals)
            };
            let zero = amount.chars().all(|c| matches!(c, '0' | '.' | ',' | ' '));
            let sign = match () {
                _ if zero => "",
                _ if stat.profit > 0.0 => "+",
                _ => "-",
            };
            format!("{sign}{amount} {}", currency.ticker())
        }
        _ => "—".to_string(),
    }
}

impl StrategiesView {
    /// The trades of the clicked coin, or `None` while no chip is selected.
    pub(super) fn distribution_trades(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let coin = self.dist.stats.coin.clone()?;
        let p = MoonPalette::active(cx);
        let stat = self
            .dist
            .stats
            .coin_stats()
            .and_then(|stats| stats.get(&coin))
            .copied()
            .unwrap_or_default();
        let mut summary = t!(
            "strat.dist_trades_summary",
            n = stat.trades,
            profit = profit_text(&stat)
        )
        .to_string();
        // The read stops at the newest TRADES_LIMIT rows, and a header sort orders only those.
        if self
            .dist
            .stats
            .trades()
            .is_some_and(|t| t.rows.len() >= super::stats::TRADES_LIMIT)
        {
            summary.push_str(" · ");
            summary.push_str(&t!(
                "strat.dist_trades_capped",
                n = super::stats::TRADES_LIMIT
            ));
        }
        let header = h_flex()
            .w_full()
            .flex_none()
            .gap(design::ui_px(cx, 12.0))
            .items_center()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 4.0))
            .border_t_1()
            .border_color(moon(p.border))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(coin.clone()))
            .child(div().text_color(moon(p.text_soft)).child(summary))
            .child(div().flex_1())
            .child(
                MoonButton::new("strat-dist-trades-close")
                    .ghost()
                    .label("×")
                    .tooltip(t!("strat.dist_trades_close").to_string())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_distribution_coin(&coin, cx)),
                    )
                    .render(),
            );

        let body = match (self.dist.stats.trades(), self.dist.stats.trades_error()) {
            (_, Some(error)) => div()
                .p(design::ui_px(cx, 12.0))
                .text_color(moon(p.red))
                .child(error.to_string())
                .into_any_element(),
            (None, None) => div()
                .p(design::ui_px(cx, 12.0))
                .text_color(moon(p.text_muted))
                .child(t!("strat.dist_trades_loading").to_string())
                .into_any_element(),
            (Some(table), None) => {
                let table = table.clone();
                self.trades_table(table, cx)
            }
        };
        Some(
            v_flex()
                .w_full()
                .flex_none()
                .h(design::ui_px(cx, 260.0))
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    /// The rows of one read, through the Report's cell formatting, in the order the table's
    /// header asks for.
    fn trades_table(
        &mut self,
        table: Rc<moon_core::db::ReportTable>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let state = self
            .dist
            .trades_table
            .get_or_insert_with(|| cx.new(|_| MoonDataTableState::new()))
            .clone();
        let shown: Vec<Col> = COLUMNS
            .iter()
            .filter_map(|name| {
                let source = if *name == STRATEGY_COL {
                    "strategyid"
                } else {
                    name
                };
                let ix = table.cols.iter().position(|c| c == source)?;
                Some(if *name == STRATEGY_COL {
                    Col::Strategy
                } else {
                    Col::Report(ix, name)
                })
            })
            .collect();
        let columns: Vec<MoonDataTableColumn> = shown
            .iter()
            .map(|col| {
                let column = match col {
                    Col::Strategy => MoonDataTableColumn::new(
                        STRATEGY_COL,
                        t!("strat.dist_col_strategy").to_string(),
                        report_width_for("fname"),
                    ),
                    Col::Report(_, name) => {
                        let column = MoonDataTableColumn::new(
                            *name,
                            report_header_label(name),
                            report_width_for(name),
                        );
                        if is_numeric_report_column(name) {
                            column.right()
                        } else {
                            column
                        }
                    }
                };
                column.sortable(true)
            })
            .collect();

        let sort = {
            let st = state.read(cx);
            st.sort_column.clone().map(|c| (c, st.sort_ascending))
        };
        let rows = match &self.dist.trades_rows {
            Some(r) if Rc::ptr_eq(&r.source, &table) && r.sort == sort => r.clone(),
            _ => {
                let id_ix = table.cols.iter().position(|c| c == "strategyid");
                let names: Vec<String> = (0..table.rows.len())
                    .map(|ix| {
                        let core = table.core_uids.get(ix).copied().unwrap_or(0);
                        let id = id_ix
                            .and_then(|c| table.rows[ix].get(c))
                            .and_then(|v| match v {
                                Value::Integer(id) => Some(*id),
                                _ => None,
                            });
                        id.map(|id| {
                            self.dist
                                .stats
                                .strategy_name(core, id)
                                .map_or_else(|| id.to_string(), str::to_string)
                        })
                        .unwrap_or_default()
                    })
                    .collect();
                let mut order: Vec<usize> = (0..table.rows.len()).collect();
                if let Some((key, ascending)) = &sort
                    && let Some(col) = shown.iter().find(|c| c.key() == key.as_ref())
                {
                    order.sort_by(|a, b| {
                        let ord = match col {
                            Col::Strategy => caseless(&names[*a], &names[*b]),
                            Col::Report(ix, _) => {
                                cmp_values(&table.rows[*a][*ix], &table.rows[*b][*ix])
                            }
                        };
                        if *ascending { ord } else { ord.reverse() }
                    });
                }
                let rows = Rc::new(TradeRows {
                    source: table.clone(),
                    sort,
                    order,
                    names,
                });
                self.dist.trades_rows = Some(rows.clone());
                rows
            }
        };

        let axis = self.backend.read(cx).report_axis(self.display_zone);
        let zone = self.display_zone;
        let count = rows.order.len();
        let shown = Rc::new(shown);
        let view = cx.entity();
        let table_view =
            MoonDataTable::new("strat-dist-trades", count, move |ix, _window, _app| {
                let ix = rows.order[ix];
                let table = &rows.source;
                let row = &table.rows[ix];
                let core_uid = table.core_uids.get(ix).copied().unwrap_or(0);
                let quote = report_row_quote(&table.cols, row);
                MoonDataRow::new(shown.iter().map(|col| match col {
                    Col::Strategy => MoonDataCell::text(rows.names[ix].clone()),
                    Col::Report(col_ix, name) => {
                        let (text, color) =
                            report_cell(name, &row[*col_ix], quote, p, &axis, core_uid, zone);
                        match color {
                            Some(color) => MoonDataCell::element(
                                div().text_color(rgb(color)).truncate().child(text),
                            ),
                            None => MoonDataCell::text(text),
                        }
                    }
                }))
            })
            .columns(columns)
            .state(&state)
            // The table flips its own sort state; the rows are ordered here, on the next render.
            .on_sort(move |_, _, _window, app| view.update(app, |_, cx| cx.notify()))
            .style(design::table_style(p));
        crate::panels::common::data_table_host(
            "strat-dist-trades-host",
            count == 0,
            t!("strat.dist_trades_empty").to_string(),
            p,
            cx,
            table_view,
        )
        .into_any_element()
    }
}

/// One drawn column: a report column by its index, or the strategy name resolved from the row's
/// `strategyid`.
#[derive(Clone, Copy)]
enum Col {
    Report(usize, &'static str),
    Strategy,
}

impl Col {
    /// The column's key in the table, which its sort state names.
    fn key(&self) -> &'static str {
        match self {
            Col::Report(_, name) => name,
            Col::Strategy => STRATEGY_COL,
        }
    }
}

/// One read's rows as drawn: the order the header asked for and each row's strategy name, kept
/// until the read or the sort changes so a hover does not sort again.
pub(in crate::strategies) struct TradeRows {
    source: Rc<moon_core::db::ReportTable>,
    sort: Option<(SharedString, bool)>,
    order: Vec<usize>,
    names: Vec<String>,
}

/// Order two report values: an empty value first, then numbers by value, then text caselessly,
/// then blobs.
///
/// A TOTAL order on purpose: SQLite lets one column hold values of several types, and a
/// comparator calling a number "equal" to a text while ordering numbers among themselves is not
/// transitive — which the standard sort is allowed to answer with a panic.
pub(super) fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    let rank = |v: &Value| match v {
        Value::Null => 0,
        Value::Integer(_) | Value::Real(_) => 1,
        Value::Text(_) => 2,
        Value::Blob(_) => 3,
    };
    let num = |v: &Value| match v {
        Value::Integer(i) => *i as f64,
        Value::Real(r) => *r,
        _ => 0.0,
    };
    rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
        (Value::Text(x), Value::Text(y)) => caseless(x, y),
        (Value::Blob(x), Value::Blob(y)) => x.cmp(y),
        _ => num(a).total_cmp(&num(b)),
    })
}

/// Compare two texts without regard to case, the way every text column of the table sorts.
fn caseless(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}
