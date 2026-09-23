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

use super::exit::{ExitParams, UnmodelledRule};
use super::mshot::{MarketSign, Modifiers, MshotParams, UsePrice};

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
    /// Strategy kinds the parameter is HIDDEN from, whatever `kinds` says — a field the core
    /// has but this kind's model does not read, so varying it would move no column. `SellPrice`
    /// against a MoonHook is the case it exists for: the field is there, and the hook's take
    /// comes from `HookSellLevel` instead.
    pub not_kinds: &'static [&'static str],
}

const MSHOT: &[&str] = &["MoonShot"];
const HOOK: &[&str] = &[super::hook::KIND_MOONHOOK];
/// The kinds whose take `SellPrice` does not place: MoonHook's is `HookSellLevel`, Spread's the
/// edge of the spread it detected (`exit::take_is_recorded`).
const NOT_SELL_PRICE: &[&str] = &[super::hook::KIND_MOONHOOK, super::exit::KIND_SPREAD];
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
/// Measured against the 1 713 live strategies that set it (2026-09-22): median 1 %, and 300 of
/// them sit outside 0.2…5 — up to 11 % — so the tail is covered rather than clipped.
const GRID_SELL_PRICE: &[f64] = &[
    0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.2, 1.4, 1.6, 1.8, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5,
    5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0,
];
const GRID_SELL_DELAY_MS: &[f64] = &[0.0, 100.0, 250.0, 500.0, 1000.0];
/// `HookSellLevel`, per cent of the detect depth: 100 sells at the top the move started from,
/// 50 in the middle. The live strategies on this machine use 50 and 100.
const GRID_HOOK_LEVEL: &[f64] = &[
    10.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 90.0, 100.0,
];
const GRID_PD_TIMER_S: &[f64] = &[
    0.0, 0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 15.0, 20.0, 30.0, 60.0, 120.0,
];
/// 1 865 live strategies set it; 121 of them below 5 %, which the old floor cut off.
const GRID_PD_PCT: &[f64] = &[
    1.0, 2.0, 3.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 70.0,
    80.0, 90.0, 100.0,
];
const GRID_PD_DELAY_S: &[f64] = &[0.0, 0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 30.0, 60.0];
const GRID_DROP: &[f64] = &[
    -1.0, -0.5, -0.2, -0.1, 0.0, 0.01, 0.05, 0.1, 0.15, 0.2, 0.3, 0.5, 0.7, 1.0, 1.5, 2.0,
];
const GRID_SL_DELAY_S: &[f64] = &[0.0, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];
const GRID_SL_TIME_S: &[f64] = &[0.0, 60.0, 300.0, 900.0, 1800.0, 3600.0, 7200.0];
const GRID_SL_COUNT: &[f64] = &[0.0, 1.0, 2.0, 3.0, 5.0, 10.0];
const GRID_SS_DISTANCE: &[f64] = &[
    0.05, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5, 0.6, 0.8, 1.0, 1.25, 1.5, 2.0,
];
const GRID_SS_CORRIDOR: &[f64] = &[10.0, 25.0, 50.0, 75.0, 90.0];
const GRID_SS_INTERVAL_S: &[f64] = &[0.2, 0.4, 0.6, 1.0, 2.0, 5.0, 10.0, 25.0];
const GRID_SS_WAIT_S: &[f64] = &[0.0, 0.1, 0.2, 0.5, 1.0, 2.0];
const GRID_SS_BOUND: &[f64] = &[
    -1.0, -0.5, -0.2, -0.1, 0.0, 0.2, 0.4, 0.5, 1.0, 2.0, 5.0, 10.0,
];
/// Live values run to −15 (29 of 1 869 strategies sit outside the old −10 floor).
const GRID_STOP: &[f64] = &[
    -15.0, -12.0, -10.0, -7.0, -5.0, -4.0, -3.0, -2.5, -2.0, -1.5, -1.0, -0.75, -0.5, -0.3, -0.2,
    -0.1,
];
const GRID_STOP_DELAY_S: &[f64] = &[0.0, 1.0, 2.0, 4.0, 6.0, 10.0, 20.0, 30.0];

/// Every parameter of the axis, grid order: the Entry group first, then Exit.
pub const TICK_PARAMS: &[TickParam] = &[
    TickParam {
        key: "MShotPrice",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_PRICE },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotPriceMin",
        group: ParamGroup::Entry,
        kind: ParamKind::Num {
            grid: GRID_PRICE_MIN,
        },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotUsePrice",
        group: ParamGroup::Entry,
        kind: ParamKind::Enum(&["Trade", "ASK", "BID"]),
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotRaiseWait",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_WAIT_S },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotReplaceDelay",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_WAIT_S },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotMinusSatoshi",
        group: ParamGroup::Entry,
        kind: ParamKind::Bool,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "FastShotAlgo",
        group: ParamGroup::Entry,
        kind: ParamKind::Bool,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddHourlyDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd3hDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd15minDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd5minDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd1minDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd24hDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddMarkDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddMarketDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddBTCDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddBTC5mDelta",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddPriceBug",
        group: ParamGroup::Entry,
        kind: ParamKind::Num { grid: GRID_ADD },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddDistance",
        group: ParamGroup::Entry,
        kind: ParamKind::Num {
            grid: GRID_DISTANCE,
        },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "SellPrice",
        group: ParamGroup::Exit,
        kind: ParamKind::Num {
            grid: GRID_SELL_PRICE,
        },
        kinds: ANY,
        // A MoonHook carries no `SellPrice` at all — `HookSellLevel` below is its take — and a
        // Spread's take is the spread it detected, whatever the field says.
        not_kinds: NOT_SELL_PRICE,
    },
    TickParam {
        key: "MShotSellAtLastPrice",
        group: ParamGroup::Exit,
        kind: ParamKind::Bool,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotSellPriceAdjust",
        group: ParamGroup::Exit,
        kind: ParamKind::Num { grid: GRID_ADJUST },
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "HookSellLevel",
        group: ParamGroup::Exit,
        kind: ParamKind::Num {
            grid: GRID_HOOK_LEVEL,
        },
        kinds: HOOK,
        not_kinds: &[],
    },
    TickParam {
        key: "SellDelay",
        group: ParamGroup::Exit,
        kind: ParamKind::Num {
            grid: GRID_SELL_DELAY_MS,
        },
        kinds: ANY,
        not_kinds: &[],
    },
    exit_num("PriceDownTimer", GRID_PD_TIMER_S),
    exit_num("PriceDownPercent", GRID_PD_PCT),
    exit_num("PriceDownDelay", GRID_PD_DELAY_S),
    exit_bool("PriceDownRelative"),
    exit_num("PriceDownAllowedDrop", GRID_DROP),
    exit_num("SellLevelDelay", GRID_SL_DELAY_S),
    exit_num("SellLevelDelayNext", GRID_SL_DELAY_S),
    exit_num("SellLevelTime", GRID_SL_TIME_S),
    exit_num("SellLevelCount", GRID_SL_COUNT),
    exit_num("SellLevelAdjust", GRID_DROP),
    exit_bool("SellLevelRelative"),
    exit_num("SellLevelAllowedDrop", GRID_DROP),
    exit_num("SellLevelWorkTime", GRID_SL_TIME_S),
    exit_bool("IgnoreSellShot"),
    exit_num("SellShotDistance", GRID_SS_DISTANCE),
    exit_num("SellShotCorridor", GRID_SS_CORRIDOR),
    exit_num("SellShotCalcInterval", GRID_SS_INTERVAL_S),
    exit_num("SellShotRaiseWait", GRID_SS_WAIT_S),
    exit_num("SellShotReplaceDelay", GRID_SS_WAIT_S),
    exit_num("SellShotAllowedUp", GRID_SS_BOUND),
    exit_num("SellShotAllowedDown", GRID_SS_BOUND),
    exit_num("SellShotDelay", GRID_SS_WAIT_S),
    exit_num("StopLoss", GRID_STOP),
    exit_num("StopLossDelay", GRID_STOP_DELAY_S),
];

/// A numeric field of the Exit group every kind understands.
const fn exit_num(key: &'static str, grid: &'static [f64]) -> TickParam {
    TickParam {
        key,
        group: ParamGroup::Exit,
        kind: ParamKind::Num { grid },
        kinds: ANY,
        not_kinds: &[],
    }
}

/// A boolean field of the Exit group every kind understands.
const fn exit_bool(key: &'static str) -> TickParam {
    TickParam {
        key,
        group: ParamGroup::Exit,
        kind: ParamKind::Bool,
        kinds: ANY,
        not_kinds: &[],
    }
}

/// The parameters of one group that a kind's grid shows.
pub fn params_for<'k>(
    group: ParamGroup,
    kind: &'k str,
) -> impl Iterator<Item = &'static TickParam> + 'k {
    TICK_PARAMS.iter().filter(move |p| {
        p.group == group
            && (p.kinds.is_empty() || p.kinds.contains(&kind))
            && !p.not_kinds.contains(&kind)
    })
}

/// Strategy fields the models READ but the grid does not offer as knobs.
///
/// They still have to be fetched: `param_keys` is what a `strategy_values_at` read asks for, so
/// a field missing from this list reads as absent and the builder silently takes its fallback —
/// which is how the delta modifiers went unapplied through a whole measurement run on 2026-09-22
/// while every test passed.
///
/// `HookSellFixed` is here because it is not a knob (the branch it selects is not modelled) but
/// its value decides whether the take is known at all; the `Add*` family and its two
/// coefficients are here because they move the level of every kind, and none of them is
/// something the search should turn.
const MODEL_ONLY_KEYS: &[&str] = &[
    "HookSellFixed",
    // Read by `exit_params` and acted on by the SellShot walk, never a grid knob — and absent
    // from both lists until 2026-09-22, so the decay of the sell-shot distance has been running
    // on its fallback since the axis was written.
    "SellShotPriceDown",
    "SellShotPriceDownDelay",
    "SellModifier",
    "MaxModifier",
    "StopLossModifier",
    // The stop's switch and its trigger (see `ExitParams::fast_stop_loss`).
    "UseStopLoss",
    "FastStopLoss",
    "StopLossEMA",
    // The switches of the sell rules the model does not have: a trade under one is not
    // modelled (`exit::UnmodelledRule`).
    "UseTrailing",
    "UseSecondStop",
    "UseStopLoss3",
    // PumpsDetection's one sell move (see `line::PUMP_MOVE_LAG_MS`); `PumpMovePersent` is the
    // core's own spelling of the field.
    "PumpMoveTimer",
    "PumpMovePersent",
    "Add1minDelta",
    "Add5minDelta",
    "Add15minDelta",
    "AddHourlyDelta",
    "Add3hDelta",
    "Add24hDelta",
    "AddMarkDelta",
    "AddPriceBug",
    "AddBTCDelta",
    "AddBTC1mDelta",
    "AddBTC5mDelta",
    "AddMarketDelta",
    "AddMarket24Delta",
    "AddPump1h",
    "AddDump1h",
];

/// Every field name the models read — [`TICK_PARAMS`] plus [`MODEL_ONLY_KEYS`] — for a
/// `strategy_current_values` read.
pub fn param_keys() -> Vec<String> {
    TICK_PARAMS
        .iter()
        .map(|p| p.key)
        .chain(MODEL_ONLY_KEYS.iter().copied())
        .map(str::to_string)
        .collect()
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
            // MoonShot's corridor family has no 1-minute BTC term; the Delta Modifiers tab does.
            add_btc_1m: 0.0,
            add_btc_5m: v.num("MShotAddBTC5mDelta", 0.0),
            add_market_1h: v.num("MShotAddMarketDelta", 0.0),
            // The corridor family has none of the three (the exe's `MShotAdd*` list).
            add_market_24h: 0.0,
            add_pump_1h: 0.0,
            add_dump_1h: 0.0,
            market_sign: MarketSign::Signed,
            distance_pct: v.num("MShotAddDistance", 0.0),
        },
        latency_ms,
    }
}

/// Sell-line parameters out of a strategy's values; `latency_ms` is the model's own.
pub fn exit_params(v: &StrategyValues<'_>) -> ExitParams {
    let base = ExitParams::default();
    ExitParams {
        sell_price_pct: v.num("SellPrice", base.sell_price_pct),
        sell_at_last_price: v.bool("MShotSellAtLastPrice", base.sell_at_last_price),
        sell_price_adjust_pct: v.num("MShotSellPriceAdjust", base.sell_price_adjust_pct),
        sell_delay_ms: v.num("SellDelay", base.sell_delay_ms),
        hook_sell_level_pct: v.num("HookSellLevel", base.hook_sell_level_pct),
        hook_sell_fixed: v.bool("HookSellFixed", base.hook_sell_fixed),
        sell_modifier: v.num("SellModifier", base.sell_modifier),
        max_modifier: v.num("MaxModifier", base.max_modifier),
        stop_loss_modifier: v.num("StopLossModifier", base.stop_loss_modifier),
        // The Delta Modifiers tab's own family — not `MShotAdd*`, which moves the entry
        // corridor; a strategy can carry both, and reading one for the other would move the
        // sell by the buy's coefficients.
        sell_mods: Modifiers {
            add_1m: v.num("Add1minDelta", 0.0),
            add_5m: v.num("Add5minDelta", 0.0),
            add_15m: v.num("Add15minDelta", 0.0),
            add_1h: v.num("AddHourlyDelta", 0.0),
            add_3h: v.num("Add3hDelta", 0.0),
            add_24h: v.num("Add24hDelta", 0.0),
            add_mark: v.num("AddMarkDelta", 0.0),
            add_pricebug: v.num("AddPriceBug", 0.0),
            add_btc_1h: v.num("AddBTCDelta", 0.0),
            add_btc_1m: v.num("AddBTC1mDelta", 0.0),
            add_btc_5m: v.num("AddBTC5mDelta", 0.0),
            add_market_1h: v.num("AddMarketDelta", 0.0),
            add_market_24h: v.num("AddMarket24Delta", 0.0),
            add_pump_1h: v.num("AddPump1h", 0.0),
            add_dump_1h: v.num("AddDump1h", 0.0),
            market_sign: MarketSign::Magnitude,
            distance_pct: 0.0,
        },
        price_down_timer_s: v.num("PriceDownTimer", base.price_down_timer_s),
        price_down_pct: v.num("PriceDownPercent", base.price_down_pct),
        price_down_delay_s: v.num("PriceDownDelay", base.price_down_delay_s),
        price_down_relative: v.bool("PriceDownRelative", base.price_down_relative),
        price_down_allowed_drop_pct: v
            .num("PriceDownAllowedDrop", base.price_down_allowed_drop_pct),
        sell_level_delay_s: v.num("SellLevelDelay", base.sell_level_delay_s),
        sell_level_delay_next_s: v.num("SellLevelDelayNext", base.sell_level_delay_next_s),
        sell_level_time_s: v.num("SellLevelTime", base.sell_level_time_s),
        sell_level_count: v
            .num("SellLevelCount", f64::from(base.sell_level_count))
            .max(0.0) as u32,
        sell_level_adjust_pct: v.num("SellLevelAdjust", base.sell_level_adjust_pct),
        sell_level_relative: v.bool("SellLevelRelative", base.sell_level_relative),
        sell_level_allowed_drop_pct: v
            .num("SellLevelAllowedDrop", base.sell_level_allowed_drop_pct),
        sell_level_work_time_s: v.num("SellLevelWorkTime", base.sell_level_work_time_s),
        ignore_sell_shot: v.bool("IgnoreSellShot", base.ignore_sell_shot),
        sell_shot_distance_pct: v.num("SellShotDistance", base.sell_shot_distance_pct),
        sell_shot_corridor_pct: v.num("SellShotCorridor", base.sell_shot_corridor_pct),
        sell_shot_calc_interval_s: v.num("SellShotCalcInterval", base.sell_shot_calc_interval_s),
        sell_shot_raise_wait_s: v.num("SellShotRaiseWait", base.sell_shot_raise_wait_s),
        sell_shot_replace_delay_s: v.num("SellShotReplaceDelay", base.sell_shot_replace_delay_s),
        sell_shot_price_down: v.num("SellShotPriceDown", base.sell_shot_price_down),
        sell_shot_price_down_delay_s: v
            .num("SellShotPriceDownDelay", base.sell_shot_price_down_delay_s),
        sell_shot_allowed_up_pct: v.num("SellShotAllowedUp", base.sell_shot_allowed_up_pct),
        sell_shot_allowed_down_pct: v.num("SellShotAllowedDown", base.sell_shot_allowed_down_pct),
        sell_shot_delay_s: v.num("SellShotDelay", base.sell_shot_delay_s),
        pump_move_timer_s: v.num("PumpMoveTimer", base.pump_move_timer_s),
        pump_move_pct: v.num("PumpMovePersent", base.pump_move_pct),
        // `StopLoss` means nothing with `UseStopLoss` off (param_deps.toml: every stop field
        // hangs on it), and the value stays in the dump when the switch goes off. A dump that
        // omits the switch keeps the stop, as the model did before it read the switch: 2 of
        // 1 422 live strategies omit it, and nothing says which way their core defaults.
        stop_loss_pct: if v.bool("UseStopLoss", true) {
            v.num("StopLoss", base.stop_loss_pct)
        } else {
            0.0
        },
        stop_loss_delay_s: v.num("StopLossDelay", base.stop_loss_delay_s),
        // Absent means the core default, NO: every live stop of a strategy that omits the field
        // closed as "StopLoss AutoActivated on price drop: BID = …" (173 of 184 with a tape),
        // the book-watching stop, never as the fast stop's "StopLoss Market Sell".
        fast_stop_loss: v.bool("FastStopLoss", false),
        stop_loss_ema: v.num("StopLossEMA", base.stop_loss_ema),
        unmodelled: unmodelled_rule(v),
        latency_ms: base.latency_ms,
        take_from_archive: base.take_from_archive,
    }
}

/// The first sell rule the strategy switched on that the model does not have, if any.
///
/// Read by the switch, never by its fields: the fields stay in a strategy's dump with the switch
/// off (`assets/param_deps.toml`). On this machine's reports (2026-09-23) the trailing stop was
/// on for the strategies of 44 trades of 2 042 and the stop ladder for 2; `SellSpread` and the EMA exit
/// were on for none, and are left out until a strategy turns them on.
fn unmodelled_rule(v: &StrategyValues<'_>) -> Option<UnmodelledRule> {
    if v.bool("UseTrailing", false) {
        Some(UnmodelledRule::Trailing)
    } else if v.bool("UseSecondStop", false) || v.bool("UseStopLoss3", false) {
        Some(UnmodelledRule::StopLadder)
    } else {
        None
    }
}
