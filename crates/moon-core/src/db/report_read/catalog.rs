//! catalog items for report reads.

/// Columns and ordering displayed in the Reports window; the window owns titles and widths.
///
/// `core_uid` and `newrecid` are hidden service columns. `id` is the shared display key for the
/// typed source's `id` and the legacy source's `db_id`.
pub const DISPLAY_COLUMNS: &[&str] = &[
    "buydate",
    "closedate",
    "sellsetdate",
    "core_name",
    "id",
    "taskid",
    "exorderid",
    "coin",
    "isshort",
    "quantity",
    "boughtq",
    "buyprice",
    "sellprice",
    "spentbtc",
    "gainedbtc",
    "profitbtc",
    "valuation_profit_usdt",
    "profitpct",
    "valuation_rate",
    "valuation_rate_source",
    "lev",
    "strategyid",
    "source",
    "channel",
    "channelname",
    "signaltype",
    "fname",
    "basecurrency",
    "emulator",
    "status",
    "sellreason",
    "comment",
    "btc1hdelta",
    "exchange1hdelta",
    "btc24hdelta",
    "exchange24hdelta",
    "btc5mdelta",
    "bvsvratio",
    "pump1h",
    "dump1h",
    "d24h",
    "d3h",
    "d1h",
    "d15m",
    "d5m",
    "d1m",
    "dbtc1m",
    "vd1m",
    "pricebug",
    "hvol",
    "hvolf",
    "dvol",
    "takeprofitlag",
    "last_update_at",
];

/// Synthetic report column containing per-trade return on positive spent capital.
pub const PROFIT_PERCENT_COLUMN: &str = "profitpct";

/// Synthetic report column carrying one trade's profit converted to USDT.
pub const VALUATION_PROFIT_COLUMN: &str = "valuation_profit_usdt";

/// Synthetic report column carrying the USDT rate applied to one trade.
pub const VALUATION_RATE_COLUMN: &str = "valuation_rate";

/// Mini App-only rate for an entry notional proven safe by the Report volume gates.
pub const MINI_ENTRY_VOLUME_RATE_COLUMN: &str = "mini_entry_volume_rate";

/// Bot-only: a closed trade's settled profit in its own currency ([`query_notify_trades`]).
pub const NOTIFY_PROFIT_NATIVE_COLUMN: &str = "notify_profit_native";

/// Bot-only: a closed trade's entry notional in its own currency, where the Report volume gates
/// prove it ([`query_notify_trades`]).
pub const NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN: &str = "notify_entry_volume_native";

/// Bot-only: the effective quote ordinal both native columns are in ([`query_notify_trades`]).
pub const NOTIFY_QUOTE_COLUMN: &str = "notify_quote";

/// Synthetic report column naming where that rate came from.
pub const VALUATION_SOURCE_COLUMN: &str = "valuation_rate_source";

/// Report columns introduced after the `v2` visible-column schema.
///
/// A saved visible-column set is explicit, so schema generations that include these columns must
/// restore them when reading an earlier set. Both [`crate::db::load_visible`] and the per-context
/// window-layout migration read this list, which keeps the two persisted stores aligned.
pub const COLUMNS_ADDED_SINCE_V2: &[&str] = &[VALUATION_PROFIT_COLUMN];

/// One Report column computed by a SQL expression rather than read from a source table.
pub(super) struct Synthetic {
    /// Runtime column key, also used as the raw export header.
    pub(super) name: &'static str,
    /// Report columns the expression reads. A source missing any of them cannot compute it.
    pub(super) inputs: &'static [&'static str],
}

/// Every synthetic Report column, and everything that distinguishes one from a stored column.
///
/// One entry per column, and one [`synthetic_expression`] serving BOTH the projection and the
/// `ORDER BY`, so a column cannot ship half-wired. That failure mode is silent and expensive: each
/// physical source is truncated by `LIMIT` before the Rust merge, so a column that could project
/// but not sort would return the wrong global top rows with no error anywhere.
pub(super) const SYNTHETIC: &[Synthetic] = &[
    Synthetic {
        name: PROFIT_PERCENT_COLUMN,
        inputs: &["profitbtc", "spentbtc"],
    },
    Synthetic {
        name: VALUATION_PROFIT_COLUMN,
        inputs: crate::db::valuation::REQUIRED_TRADE_INPUTS,
    },
    Synthetic {
        name: VALUATION_RATE_COLUMN,
        inputs: crate::db::valuation::REQUIRED_TRADE_INPUTS,
    },
    Synthetic {
        name: VALUATION_SOURCE_COLUMN,
        inputs: crate::db::valuation::REQUIRED_TRADE_INPUTS,
    },
];

/// Look one column up in the synthetic table.
pub(super) fn synthetic(col: &str) -> Option<&'static Synthetic> {
    SYNTHETIC.iter().find(|entry| entry.name == col)
}
