//! The parameter descriptor of the axis — the ONE place a strategy field of the Entry/Exit
//! grid is declared. The UI grid, the "now" column, the search grid and the save dialog all
//! derive from [`TICK_PARAMS`]; the model structs are built from a strategy's values through
//! [`mshot_params`] and [`exit_params`] here, so a field name is spelled once.
//!
//! Field names are the `strategies.sqlite` keys (verified against the live file, 2026-09-20).
//! A key absent from a strategy's `raw_json` is at its schema default — the wire omits fields at
//! default — which the caller passes in from the live schema; a key absent from both falls to
//! the model's own default.

use std::collections::HashMap;

use super::exit::ExitParams;
use super::mshot::{Modifiers, MshotParams, UsePrice};

/// Which group of the grid a parameter belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamGroup {
    /// The entry model — available only for kinds that have one.
    Entry,
    /// The sell line — every kind.
    Exit,
}

/// How a parameter is typed and, for the search, which values it may take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// A number; `grid` is the search's discrete candidate set (§5.1 of the spec — a proposal
    /// to narrow to practice).
    Num { grid: &'static [f64] },
    /// `YES` / `NO`.
    Bool,
    /// One of a fixed spelling set.
    Enum(&'static [&'static str]),
}

/// One parameter of the axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickParam {
    /// The strategy field name, as stored and as shown in the grid.
    pub key: &'static str,
    pub group: ParamGroup,
    pub kind: ParamKind,
    /// Strategy kinds whose grid shows this parameter; empty means every kind.
    pub kinds: &'static [&'static str],
}

const MSHOT: &[&str] = &["MoonShot"];
const ANY: &[&str] = &[];

const GRID_PRICE: &[f64] = &[
    0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5,
    7.0, 7.5, 8.0, 8.5, 9.0, 9.5, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0,
];
const GRID_PRICE_MIN: &[f64] = &[
    0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.25, 1.5,
    1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
];
const GRID_WAIT_S: &[f64] = &[0.0, 0.1, 0.3, 0.5, 1.0, 2.0, 5.0];
const GRID_ADJUST: &[f64] = &[
    -0.5, -0.4, -0.3, -0.2, -0.1, 0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.2, 1.4,
    1.6, 1.8, 2.0,
];
const GRID_ADD: &[f64] = &[
    0.0, 0.01, 0.02, 0.03, 0.04, 0.05, 0.06, 0.07, 0.08, 0.09, 0.1, 0.12, 0.14, 0.16, 0.18, 0.2,
];
const GRID_DISTANCE: &[f64] = &[0.0, 25.0, 50.0, 100.0, 200.0];
const GRID_SELL_PRICE: &[f64] = &[
    0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.2, 1.4, 1.6, 1.8, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5,
    5.0,
];
const GRID_SELL_DELAY_MS: &[f64] = &[0.0, 100.0, 250.0, 500.0, 1000.0];

/// Every parameter of the axis, grid order: the Entry group first, then Exit.
pub const TICK_PARAMS: &[TickParam] = &[
    TickParam {
        key: "MShotPrice",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_PRICE },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotPriceMin",
        group: ParamGroup::Entry,
        kind: ParamKind::Num {
            grid: GRID_PRICE_MIN,
        },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotUsePrice",
        group: ParamGroup::Entry,
        kind: ParamKind::Enum(&["Trade", "ASK", "BID"]),
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotRaiseWait",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_WAIT_S },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotReplaceDelay",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_WAIT_S },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotMinusSatoshi",
        group: ParamGroup::Entry,
        kind: ParamKind::Bool,
        kinds: MSHOT,
    },
    TickParam {
        key: "FastShotAlgo",
        group: ParamGroup::Entry,
        kind: ParamKind::Bool,
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAddHourlyDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAdd3hDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAdd15minDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAdd5minDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAdd1minDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAdd24hDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAddMarkDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAddMarketDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAddBTCDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAddBTC5mDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAddPriceBug",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotAddDistance",
        group: ParamGroup::Entry,
        kind: ParamKind::Num {
            grid: GRID_DISTANCE,
        },
        kinds: MSHOT,
    },
    TickParam {
        key: "SellPrice",
        group: ParamGroup::Exit,
        kind: ParamKind::Num {
            grid: GRID_SELL_PRICE,
        },
        kinds: ANY,
    },
    TickParam {
        key: "MShotSellAtLastPrice",
        group: ParamGroup::Exit,
        kind: ParamKind::Bool,
        kinds: MSHOT,
    },
    TickParam {
        key: "MShotSellPriceAdjust",
        group: ParamGroup::Exit,
        kind: ParamKind::Num { grid: GRID_ADJUST },
        kinds: MSHOT,
    },
    TickParam {
        key: "SellDelay",
        group: ParamGroup::Exit,
        kind: ParamKind::Num {
            grid: GRID_SELL_DELAY_MS,
        },
        kinds: ANY,
    },
];

/// The parameters of one group that a kind's grid shows.
pub fn params_for<'k>(
    group: ParamGroup,
    kind: &'k str,
) -> impl Iterator<Item = &'static TickParam> + 'k {
    TICK_PARAMS
        .iter()
        .filter(move |p| p.group == group && (p.kinds.is_empty() || p.kinds.contains(&kind)))
}

/// The field names of [`TICK_PARAMS`], for a `strategy_current_values` read.
pub fn param_keys() -> Vec<String> {
    TICK_PARAMS.iter().map(|p| p.key.to_string()).collect()
}

/// Strategy values as `strategy_current_values` hands them (strings, `YES`/`NO` booleans) plus
/// the schema defaults for the keys the strategy left at default (lowercase key → number).
pub struct StrategyValues<'a> {
    pub values: &'a HashMap<String, String>,
    pub defaults: &'a HashMap<String, f64>,
}

impl StrategyValues<'_> {
    /// A numeric field: the strategy's value, else the schema default, else `fallback`.
    pub fn num(&self, key: &str, fallback: f64) -> f64 {
        self.values
            .get(key)
            .and_then(|s| parse_num(s))
            .or_else(|| self.defaults.get(&key.to_ascii_lowercase()).copied())
            .unwrap_or(fallback)
    }

    /// A boolean field (`YES`/`NO`, `true`/`false`, `1`/`0`); absent → the schema default read
    /// as a number, else `fallback`.
    pub fn bool(&self, key: &str, fallback: bool) -> bool {
        match self.values.get(key).map(|s| s.trim().to_ascii_uppercase()) {
            Some(s) if s == "YES" || s == "TRUE" || s == "1" => true,
            Some(s) if s == "NO" || s == "FALSE" || s == "0" => false,
            _ => self
                .defaults
                .get(&key.to_ascii_lowercase())
                .map(|d| *d != 0.0)
                .unwrap_or(fallback),
        }
    }

    /// A string field, or `fallback` when absent.
    pub fn text<'b>(&'b self, key: &str, fallback: &'b str) -> &'b str {
        self.values.get(key).map(String::as_str).unwrap_or(fallback)
    }
}

/// Parse a strategy number: `1.5`, `1,5`, `1.5%`.
fn parse_num(s: &str) -> Option<f64> {
    s.trim()
        .trim_end_matches('%')
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

/// MoonShot entry parameters out of a strategy's values; `latency_ms` is the model's own.
pub fn mshot_params(v: &StrategyValues<'_>, latency_ms: f64) -> MshotParams {
    let base = MshotParams::default();
    MshotParams {
        price_pct: v.num("MShotPrice", base.price_pct),
        price_min_pct: v.num("MShotPriceMin", base.price_min_pct),
        use_price: UsePrice::parse(v.text("MShotUsePrice", "Trade")),
        raise_wait_s: v.num("MShotRaiseWait", base.raise_wait_s),
        replace_delay_s: v.num("MShotReplaceDelay", base.replace_delay_s),
        minus_satoshi: v.bool("MShotMinusSatoshi", base.minus_satoshi),
        fast_algo: v.bool("FastShotAlgo", base.fast_algo),
        modifiers: Modifiers {
            add_1m: v.num("MShotAdd1minDelta", 0.0),
            add_5m: v.num("MShotAdd5minDelta", 0.0),
            add_15m: v.num("MShotAdd15minDelta", 0.0),
            add_1h: v.num("MShotAddHourlyDelta", 0.0),
            add_3h: v.num("MShotAdd3hDelta", 0.0),
            add_24h: v.num("MShotAdd24hDelta", 0.0),
            add_mark: v.num("MShotAddMarkDelta", 0.0),
            add_pricebug: v.num("MShotAddPriceBug", 0.0),
            add_btc_1h: v.num("MShotAddBTCDelta", 0.0),
            add_btc_5m: v.num("MShotAddBTC5mDelta", 0.0),
            add_market_1h: v.num("MShotAddMarketDelta", 0.0),
            distance_pct: v.num("MShotAddDistance", 0.0),
        },
        latency_ms,
    }
}

/// Sell-line parameters out of a strategy's values.
pub fn exit_params(v: &StrategyValues<'_>) -> ExitParams {
    let base = ExitParams::default();
    ExitParams {
        sell_price_pct: v.num("SellPrice", base.sell_price_pct),
        sell_at_last_price: v.bool("MShotSellAtLastPrice", base.sell_at_last_price),
        sell_price_adjust_pct: v.num("MShotSellPriceAdjust", base.sell_price_adjust_pct),
        sell_delay_ms: v.num("SellDelay", base.sell_delay_ms),
    }
}
