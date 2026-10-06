//! Read layer for the Reports window: filters, source projection, sort/merge, and aggregates.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::Connection;
use rusqlite::types::{Value, ValueRef};

use crate::strategy_query::StrategyQuery;

use super::read_fail::read_fail;
use super::rep;
use super::report_axis::ReportStamp;
use super::sql_sum::{SumColumn, SumZero};
use super::valuation::ValuationMode;
use super::{
    QuoteBreakdown, QuoteCurrency, ReadResult, ReadSource, read_sources_res, table_columns_res,
};

/// catalog items for report reads.
mod catalog;
use catalog::*;
pub use catalog::{
    COLUMNS_ADDED_SINCE_V2, DISPLAY_COLUMNS, MINI_ENTRY_VOLUME_RATE_COLUMN,
    NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN, NOTIFY_PROFIT_NATIVE_COLUMN, NOTIFY_QUOTE_COLUMN,
    PROFIT_PERCENT_COLUMN, VALUATION_PROFIT_COLUMN, VALUATION_RATE_COLUMN, VALUATION_SOURCE_COLUMN,
};

/// types items for report reads.
mod types;
use types::*;
pub use types::{
    ChartTradeHistory, ChartTradeRecord, PeriodBasis, ProfitMetric, ReportFilter, ReportStrategy,
    ReportStrategyKey, ReportTable, ReportTotals, RowScope, SideFilter, report_coin_is_exact,
};

/// select items for report reads.
mod select;
pub use select::display_columns;
use select::*;

/// scope items for report reads.
mod scope;
pub use scope::open_rows_for_bound;
pub(crate) use scope::*;

/// strategy mask items for report reads.
mod strategy_mask;
use strategy_mask::*;

/// sums items for report reads.
mod sums;
pub(in crate::db) use sums::*;
pub use sums::{StrategyPurgeRows, query_totals, strategy_purge_rows};

/// passes items for report reads.
mod passes;
use passes::*;
pub use passes::{query_mini_trades, query_notify_trades, query_reports};

/// chart items for report reads.
mod chart;
pub use chart::{
    CHART_TRADE_HISTORY_ATTACH, query_chart_trade_history, query_chart_trade_history_for_cores,
};

/// inventory items for report reads.
mod inventory;
pub(crate) use inventory::*;
pub use inventory::{distinct_cores, distinct_strategies, max_core_uid, rows_by_core};

mod totals;
pub use totals::{TotalsSlice, query_totals_sliced};

#[cfg(test)]
mod tests;
