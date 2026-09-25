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

use super::exit::{ExitParams, StopStep, UnmodelledRule};
use super::mshot::{MSHOT_PRICEBUG_CAP_PCT, MarketSign, Modifiers, MshotParams, UsePrice};
use super::settings::ModelSettings;

/// Which group of the grid a parameter belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamGroup {
    /// The entry model — available only for kinds that have one.
    Entry,
    /// The sell line — every kind.
    Exit,
}

/// The strategy editor's section a field sits in — where Moonbot's Strategies window shows it.
///
/// The grid lays its rows out by section, not by [`ParamGroup`]: a MoonShot's
/// `MShotSellAtLastPrice` moves the exit and sits in "Strategy settings" beside the entry
/// corridor, and it is found where the Strategies window has it. The group still decides what the
/// search gates on; the section only decides where the row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamSection {
    StrategySettings,
    Stops,
    SellOrder,
    SellShot,
    SellSpread,
    DeltaModifiers,
}

impl ParamSection {
    /// The sections of the grid, in the order it draws them.
    pub const GRID_ORDER: [ParamSection; 6] = [
        ParamSection::StrategySettings,
        ParamSection::Stops,
        ParamSection::SellOrder,
        ParamSection::SellShot,
        ParamSection::SellSpread,
        ParamSection::DeltaModifiers,
    ];

    /// The section's title as the strategy schema spells it (`assets/param_deps.toml`).
    pub fn schema_title(self) -> &'static str {
        match self {
            ParamSection::StrategySettings => "Strategy settings",
            ParamSection::Stops => "Stops",
            ParamSection::SellOrder => "Sell order",
            ParamSection::SellShot => "Sell order / SellShot",
            ParamSection::SellSpread => "Sell order / SellSpread",
            ParamSection::DeltaModifiers => "Delta Modifiers",
        }
    }

    /// Whether the model has the section's rules at all. SellShot and SellSpread it does not (the
    /// developer's call, 2026-09-24): the grid draws them without knobs and says so, and a trade
    /// of a strategy that switches one on is not judged (`exit::UnmodelledRule`).
    pub fn modelled(self) -> bool {
        !matches!(self, ParamSection::SellShot | ParamSection::SellSpread)
    }
}

/// How a parameter is typed. A number's candidate values are not fixed here: they come from
/// what the live strategies hold of it, or from the range the user typed ([`range`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// A number.
    Num,
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
    /// Where the grid draws the row when the live schema does not place it — no core with a
    /// schema is connected. The schema, when there is one, wins.
    pub section: ParamSection,
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
/// edge of the spread it detected (`exit::sell_order::take_is_recorded`).
const NOT_SELL_PRICE: &[&str] = &[
    super::hook::KIND_MOONHOOK,
    super::exit::sell_order::KIND_SPREAD,
];
const ANY: &[&str] = &[];

/// Every parameter of the axis, grid order: the Entry group first, then Exit.
pub const TICK_PARAMS: &[TickParam] = &[
    TickParam {
        key: "MShotPrice",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotPriceMin",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotUsePrice",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Enum(&["Trade", "ASK", "BID"]),
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotRaiseWait",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotReplaceDelay",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotMinusSatoshi",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Bool,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "FastShotAlgo",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Bool,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddHourlyDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd3hDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd15minDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd5minDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd1minDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAdd24hDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddMarkDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddMarketDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddBTCDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddBTC5mDelta",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddPriceBug",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotAddDistance",
        group: ParamGroup::Entry,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "SellPrice",
        group: ParamGroup::Exit,
        section: ParamSection::SellOrder,
        kind: ParamKind::Num,
        kinds: ANY,
        // A MoonHook carries no `SellPrice` at all — `HookSellLevel` below is its take — and a
        // Spread's take is the spread it detected, whatever the field says.
        not_kinds: NOT_SELL_PRICE,
    },
    TickParam {
        key: "MShotSellAtLastPrice",
        group: ParamGroup::Exit,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Bool,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "MShotSellPriceAdjust",
        group: ParamGroup::Exit,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: MSHOT,
        not_kinds: &[],
    },
    TickParam {
        key: "HookSellLevel",
        group: ParamGroup::Exit,
        section: ParamSection::StrategySettings,
        kind: ParamKind::Num,
        kinds: HOOK,
        not_kinds: &[],
    },
    TickParam {
        key: "SellDelay",
        group: ParamGroup::Exit,
        section: ParamSection::SellOrder,
        kind: ParamKind::Num,
        kinds: ANY,
        not_kinds: &[],
    },
    exit_num("PriceDownTimer", ParamSection::SellOrder),
    exit_num("PriceDownPercent", ParamSection::SellOrder),
    exit_num("PriceDownDelay", ParamSection::SellOrder),
    exit_bool("PriceDownRelative", ParamSection::SellOrder),
    exit_num("PriceDownAllowedDrop", ParamSection::SellOrder),
    exit_num("SellLevelDelay", ParamSection::SellOrder),
    exit_num("SellLevelDelayNext", ParamSection::SellOrder),
    exit_num("SellLevelTime", ParamSection::SellOrder),
    exit_num("SellLevelCount", ParamSection::SellOrder),
    exit_num("SellLevelAdjust", ParamSection::SellOrder),
    exit_bool("SellLevelRelative", ParamSection::SellOrder),
    exit_num("SellLevelAllowedDrop", ParamSection::SellOrder),
    exit_num("SellLevelWorkTime", ParamSection::SellOrder),
    // The Stops section in the strategy window's order. `FastStopLoss` is read with the strategy's
    // value — the trigger hangs on it — but is no knob; `UseMarketOrder` is read by nothing (the
    // verdict tells a market stop by the fact's own reason, `StopLoss Market Sell`); nor are the panic sell's execution fields (`StopLossSpread`,
    // `StopSpreadAdd1mDelta`, `AllowedDrop`, `AllowedDrop3`, `TrailingSpread`): where a panic
    // sell fills is the book's, which the tape does not carry (the developer's call, 2026-09-24).
    exit_bool("UseStopLoss", ParamSection::Stops),
    TickParam {
        key: "StopLossEMA",
        group: ParamGroup::Exit,
        section: ParamSection::Stops,
        // The core averages at 3, 5 and 10 only (`exit::stops::stop_average_weight`).
        kind: ParamKind::Enum(&["0", "3", "5", "10"]),
        kinds: ANY,
        not_kinds: &[],
    },
    exit_num("StopLossDelay", ParamSection::Stops),
    exit_num("StopLoss", ParamSection::Stops),
    exit_bool("UseSecondStop", ParamSection::Stops),
    exit_num("TimeToSwitch2Stop", ParamSection::Stops),
    exit_num("PriceToSwitch2Stop", ParamSection::Stops),
    exit_num("SecondStopLoss", ParamSection::Stops),
    exit_bool("UseStopLoss3", ParamSection::Stops),
    exit_num("TimeToSwitchStop3", ParamSection::Stops),
    exit_num("PriceToSwitchStop3", ParamSection::Stops),
    exit_num("StopLoss3", ParamSection::Stops),
    exit_bool("UseTrailing", ParamSection::Stops),
    exit_num("TrailingPercent", ParamSection::Stops),
    exit_num("TrailingEMA", ParamSection::Stops),
    exit_bool("UseTakeProfit", ParamSection::Stops),
    exit_num("TakeProfit", ParamSection::Stops),
    // The Delta Modifiers section: one capped sum of the trade's deltas, spent on the sell and on
    // the stop (`exit::delta_mods`). It is a product, and the search walks it as one
    // (`search::coupled`). `BuyModifier` and `DetectModifier` move the entry and the detect,
    // which the model takes from the fact for every kind but MoonShot, whose core ignores them.
    exit_num("SellModifier", ParamSection::DeltaModifiers),
    exit_num("StopLossModifier", ParamSection::DeltaModifiers),
    TickParam {
        key: "MaxModifier",
        group: ParamGroup::Exit,
        section: ParamSection::DeltaModifiers,
        kind: ParamKind::Num,
        kinds: ANY,
        // One field, two families: a MoonShot's cap also bounds its `MShotAdd*` corridor
        // (`mshot_params`), so turning it in an Exit search would move the entry the search
        // leaves alone, past every corridor guard. There it stays at the strategy's value.
        not_kinds: MSHOT,
    },
    delta_add("Add1minDelta"),
    delta_add("Add5minDelta"),
    delta_add("Add15minDelta"),
    delta_add("AddHourlyDelta"),
    delta_add("Add3hDelta"),
    delta_add("Add24hDelta"),
    delta_add("AddMarketDelta"),
    delta_add("AddMarket24Delta"),
    delta_add("AddBTCDelta"),
    delta_add("AddBTC5mDelta"),
    delta_add("AddBTC1mDelta"),
    delta_add("AddMarkDelta"),
    delta_add("AddPump1h"),
    delta_add("AddDump1h"),
    delta_add("AddPriceBug"),
];

/// An `Add*` term of the Delta Modifiers section.
const fn delta_add(key: &'static str) -> TickParam {
    exit_num(key, ParamSection::DeltaModifiers)
}

/// A numeric field of the Exit group every kind understands.
const fn exit_num(key: &'static str, section: ParamSection) -> TickParam {
    TickParam {
        key,
        group: ParamGroup::Exit,
        section,
        kind: ParamKind::Num,
        kinds: ANY,
        not_kinds: &[],
    }
}

/// A boolean field of the Exit group every kind understands.
const fn exit_bool(key: &'static str, section: ParamSection) -> TickParam {
    TickParam {
        key,
        group: ParamGroup::Exit,
        section,
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
/// its value decides whether the take is known at all. The Delta Modifiers section is a knob
/// since 2026-09-25, all but `MaxModifier` on a MoonShot, which the grid draws fixed there
/// (see its [`TICK_PARAMS`] entry) — so it is listed here as well.
const MODEL_ONLY_KEYS: &[&str] = &[
    "HookSellFixed",
    "MaxModifier",
    // The stop's trigger (see `ExitParams::fast_stop_loss`): read with the strategy's value, no
    // knob.
    "FastStopLoss",
    // PumpsDetection's one sell move (see `exit::pump_move::PUMP_MOVE_LAG_MS`); `PumpMovePersent` is the
    // core's own spelling of the field.
    "PumpMoveTimer",
    "PumpMovePersent",
    // The corridor family's one modifier the grid does not offer (no live strategy sets it).
    "MShotAdd5sDelta",
];

/// The switches of the sell rules the model does NOT have — read only to tell that one is on,
/// which keeps the trade out of the verdict and the search ([`unmodelled_rule`]). Not
/// [`MODEL_ONLY_KEYS`]: the model acts on none of them, and the grid draws them as outside it.
const RULE_SWITCH_KEYS: &[&str] = &[
    // No sell order at all.
    "AutoSell",
    // SellShot is on only with a distance to keep.
    "IgnoreSellShot",
    "SellShotDistance",
    "IgnoreSellSpread",
];

/// Exit fields the entry model reads as well: a MoonShot's `MaxModifier` caps its `MShotAdd*`
/// corridor too ([`mshot_params`]).
const ENTRY_SHARED_KEYS: &[&str] = &["MaxModifier"];

/// Whether writing `key` can move a MoonShot's entry corridor — an Entry field, or an Exit field
/// the entry model reads too. What a write's corridor warning keys on: the group alone misses
/// `MaxModifier`, which a mixed-kind scope offers as a knob and Save writes to every strategy.
pub fn moves_entry(key: &str) -> bool {
    ENTRY_SHARED_KEYS.contains(&key)
        || TICK_PARAMS
            .iter()
            .any(|f| f.key == key && f.group == ParamGroup::Entry)
}

/// Whether the models read `key` from the strategy without the grid offering it as a knob —
/// the grid draws such a field as fixed rather than as outside the model.
pub fn is_model_only(key: &str) -> bool {
    MODEL_ONLY_KEYS.contains(&key)
}

/// Every field name the models read — [`TICK_PARAMS`], [`MODEL_ONLY_KEYS`] and the switches of
/// the rules they do not have ([`RULE_SWITCH_KEYS`]) — for a `strategy_current_values` read,
/// each once: a knob for some kinds is model-only for others (`MaxModifier`).
pub fn param_keys() -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for key in TICK_PARAMS
        .iter()
        .map(|p| p.key)
        .chain(MODEL_ONLY_KEYS.iter().copied())
        .chain(RULE_SWITCH_KEYS.iter().copied())
    {
        if !keys.iter().any(|k| k == key) {
            keys.push(key.to_string());
        }
    }
    keys
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

/// MoonShot entry parameters out of a strategy's values, under the model's own settings.
pub fn mshot_params(v: &StrategyValues<'_>, model: ModelSettings) -> MshotParams {
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
            add_5s: v.num("MShotAdd5sDelta", 0.0),
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
            pricebug_cap: MSHOT_PRICEBUG_CAP_PCT,
        },
        max_modifier: v.num("MaxModifier", 0.0),
        model,
    }
}

/// Sell-line parameters out of a strategy's values, under the model's own settings.
pub fn exit_params(v: &StrategyValues<'_>, model: ModelSettings) -> ExitParams {
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
            add_5s: 0.0,
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
            pricebug_cap: 0.0,
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
        // Every trailing field hangs on `UseTrailing` (param_deps.toml), and `TakeProfit` on
        // `UseTakeProfit` too; the values stay in the dump with the switches off.
        trailing_pct: if v.bool("UseTrailing", false) {
            v.num("TrailingPercent", base.trailing_pct)
        } else {
            0.0
        },
        trailing_ema: v.num("TrailingEMA", base.trailing_ema),
        trailing_take_profit_pct: (v.bool("UseTrailing", false) && v.bool("UseTakeProfit", false))
            .then(|| v.num("TakeProfit", 0.0)),
        // The ladder's fields hang on `UseStopLoss` and on their own switch (param_deps.toml).
        second_stop: (v.bool("UseStopLoss", true) && v.bool("UseSecondStop", false)).then(|| {
            StopStep {
                after_s: v.num("TimeToSwitch2Stop", 0.0),
                switch_pct: v.num("PriceToSwitch2Stop", 0.0),
                level_pct: v.num("SecondStopLoss", 0.0),
            }
        }),
        third_stop: (v.bool("UseStopLoss", true) && v.bool("UseStopLoss3", false)).then(|| {
            StopStep {
                after_s: v.num("TimeToSwitchStop3", 0.0),
                switch_pct: v.num("PriceToSwitchStop3", 0.0),
                level_pct: v.num("StopLoss3", 0.0),
            }
        }),
        unmodelled: unmodelled_rule(v),
        model,
        take_from_archive: base.take_from_archive,
    }
}

/// The first sell rule the strategy switched on that the model does not have, if any.
///
/// Read by the switch, never by its fields: the fields stay in a strategy's dump with the switch
/// off (`assets/param_deps.toml`). On this machine's reports (2026-09-23) the trailing stop was
/// on for the strategies of 44 trades of 2 042 and the stop ladder for 2; both are modelled since
/// 2026-09-24 (`exit::stops::trailing`, `exit::stops::ladder`). SellShot and SellSpread are not modelled
/// at all (the developer's call, 2026-09-24); each is on for 2 live strategies of 1 422 (24.09),
/// SellShot only where a distance is set (the walk kept it off at a zero one). `AutoSell` off
/// places no sell order at all (no live strategy, 24.09). The EMA exit and
/// the rest of the sell fields the model does not have are in `unmodelled`, which warns about
/// them; this is the set that takes a trade out of the verdict.
pub(super) fn unmodelled_rule(v: &StrategyValues<'_>) -> Option<UnmodelledRule> {
    if !v.bool("AutoSell", true) {
        Some(UnmodelledRule::NoAutoSell)
    } else if !v.bool("IgnoreSellShot", true) && v.num("SellShotDistance", 0.0) != 0.0 {
        Some(UnmodelledRule::SellShot)
    } else if !v.bool("IgnoreSellSpread", true) {
        Some(UnmodelledRule::SellSpread)
    } else {
        None
    }
}

pub mod range;

#[cfg(test)]
mod tests;
