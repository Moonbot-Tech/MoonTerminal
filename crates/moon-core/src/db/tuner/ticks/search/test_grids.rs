//! The ladders the axis searched by until 2026-09-25, kept as fixed grids for the tests: the
//! search's own behaviour is what they pin, and a test that moved with the live strategies' spread
//! would pin nothing. The axis itself takes its grids from `params::range`.

use std::sync::Arc;

use crate::db::tuner::ticks::params::range::Grids;

const ADD: &[f64] = &[
    0.0, 0.001, 0.002, 0.005, 0.01, 0.02, 0.03, 0.04, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4,
    0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95, 1.0,
];
const ADJUST: &[f64] = &[
    -0.5, -0.4, -0.3, -0.2, -0.1, 0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.2, 1.4,
    1.6, 1.8, 2.0,
];
const DELTA_ADD: &[f64] = &[
    0.0, 0.001, 0.002, 0.005, 0.01, 0.02, 0.03, 0.05, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5, 0.75, 1.0,
    1.5, 2.0, 3.0,
];
const DISTANCE: &[f64] = &[
    0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 75.0, 100.0, 150.0, 200.0,
];
const DROP: &[f64] = &[
    -1.0, -0.5, -0.2, -0.1, 0.0, 0.01, 0.05, 0.1, 0.15, 0.2, 0.3, 0.5, 0.7, 1.0, 1.5, 2.0,
];
const HOOK_LEVEL: &[f64] = &[
    10.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 90.0, 100.0,
];
const MAX_MODIFIER: &[f64] = &[
    0.0, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 100.0, 130.0, 200.0, 1000.0,
];
const PD_DELAY_S: &[f64] = &[0.0, 0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 30.0, 60.0];
const PD_PCT: &[f64] = &[
    1.0, 2.0, 3.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 70.0,
    80.0, 90.0, 100.0,
];
const PD_TIMER_S: &[f64] = &[
    0.0, 0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 15.0, 20.0, 30.0, 60.0, 120.0,
];
const PRICE: &[f64] = &[
    0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85,
    0.9, 0.95, 1.0, 1.05, 1.1, 1.15, 1.2, 1.25, 1.3, 1.35, 1.4, 1.45, 1.5, 1.55, 1.6, 1.65, 1.7,
    1.75, 1.8, 1.85, 1.9, 1.95, 2.0, 2.05, 2.1, 2.15, 2.2, 2.25, 2.3, 2.35, 2.4, 2.45, 2.5, 2.55,
    2.6, 2.65, 2.7, 2.75, 2.8, 2.85, 2.9, 2.95, 3.0, 3.05, 3.1, 3.15, 3.2, 3.25, 3.3, 3.35, 3.4,
    3.45, 3.5, 3.55, 3.6, 3.65, 3.7, 3.75, 3.8, 3.85, 3.9, 3.95, 4.0, 4.05, 4.1, 4.15, 4.2, 4.25,
    4.3, 4.35, 4.4, 4.45, 4.5, 4.55, 4.6, 4.65, 4.7, 4.75, 4.8, 4.85, 4.9, 4.95, 5.0, 5.05, 5.1,
    5.15, 5.2, 5.25, 5.3, 5.35, 5.4, 5.45, 5.5, 5.55, 5.6, 5.65, 5.7, 5.75, 5.8, 5.85, 5.9, 5.95,
    6.0, 6.05, 6.1, 6.15, 6.2, 6.25, 6.3, 6.35, 6.4, 6.45, 6.5, 6.55, 6.6, 6.65, 6.7, 6.75, 6.8,
    6.85, 6.9, 6.95, 7.0, 7.05, 7.1, 7.15, 7.2, 7.25, 7.3, 7.35, 7.4, 7.45, 7.5, 7.55, 7.6, 7.65,
    7.7, 7.75, 7.8, 7.85, 7.9, 7.95, 8.0,
];
const PRICE_MIN: &[f64] = &[
    0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85,
    0.9, 0.95, 1.0, 1.05, 1.1, 1.15, 1.2, 1.25, 1.3, 1.35, 1.4, 1.45, 1.5, 1.55, 1.6, 1.65, 1.7,
    1.75, 1.8, 1.85, 1.9, 1.95, 2.0, 2.05, 2.1, 2.15, 2.2, 2.25, 2.3, 2.35, 2.4, 2.45, 2.5, 2.55,
    2.6, 2.65, 2.7, 2.75, 2.8, 2.85, 2.9, 2.95, 3.0, 3.05, 3.1, 3.15, 3.2, 3.25, 3.3, 3.35, 3.4,
    3.45, 3.5, 3.55, 3.6, 3.65, 3.7, 3.75, 3.8, 3.85, 3.9, 3.95, 4.0, 4.05, 4.1, 4.15, 4.2, 4.25,
    4.3, 4.35, 4.4, 4.45, 4.5, 4.55, 4.6, 4.65, 4.7, 4.75, 4.8, 4.85, 4.9, 4.95, 5.0, 5.05, 5.1,
    5.15, 5.2, 5.25, 5.3, 5.35, 5.4, 5.45, 5.5, 5.55, 5.6, 5.65, 5.7, 5.75, 5.8, 5.85, 5.9, 5.95,
    6.0, 6.05, 6.1, 6.15, 6.2, 6.25, 6.3, 6.35, 6.4, 6.45, 6.5, 6.55, 6.6, 6.65, 6.7, 6.75, 6.8,
    6.85, 6.9, 6.95, 7.0, 7.05, 7.1, 7.15, 7.2, 7.25, 7.3, 7.35, 7.4, 7.45, 7.5, 7.55, 7.6, 7.65,
    7.7, 7.75, 7.8, 7.85, 7.9, 7.95, 8.0,
];
const SELL_DELAY_MS: &[f64] = &[0.0, 100.0, 250.0, 500.0, 1000.0];
const SELL_MODIFIER: &[f64] = &[
    -0.5, -0.3, -0.2, -0.1, -0.05, 0.0, 0.03, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.75, 1.0,
    1.5,
];
const SELL_PRICE: &[f64] = &[
    0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.2, 1.4, 1.6, 1.8, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5,
    5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0,
];
const SL_COUNT: &[f64] = &[0.0, 1.0, 2.0, 3.0, 5.0, 10.0];
const SL_DELAY_S: &[f64] = &[0.0, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];
const SL_TIME_S: &[f64] = &[0.0, 60.0, 300.0, 900.0, 1800.0, 3600.0, 7200.0];
const STEP_LEVEL: &[f64] = &[
    -3.0, -2.0, -1.0, -0.5, -0.2, 0.0, 0.1, 0.2, 0.25, 0.3, 0.4, 0.5, 0.8, 1.0, 1.5, 2.0,
];
const STOP: &[f64] = &[
    -15.0, -12.0, -10.0, -7.0, -5.0, -4.0, -3.0, -2.5, -2.0, -1.5, -1.0, -0.75, -0.5, -0.3, -0.2,
    -0.1,
];
const STOP_DELAY_S: &[f64] = &[0.0, 1.0, 2.0, 4.0, 6.0, 10.0, 20.0, 30.0];
const STOP_MODIFIER: &[f64] = &[
    -0.5, -0.3, -0.2, -0.1, -0.05, 0.0, 0.05, 0.1, 0.2, 0.3, 0.5, 1.0,
];
const SWITCH_PCT: &[f64] = &[0.1, 0.2, 0.3, 0.5, 0.8, 1.0, 1.3, 1.5, 2.0, 3.0, 5.0];
const SWITCH_S: &[f64] = &[
    0.0, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0,
];
const TAKE_PROFIT: &[f64] = &[0.2, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 5.0, 10.0];
const TRAILING: &[f64] = &[
    -10.0, -5.0, -4.0, -3.0, -2.0, -1.8, -1.5, -1.0, -0.8, -0.5, -0.3, -0.2, -0.1,
];
const TRAILING_EMA: &[f64] = &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 10.0];
const WAIT_S: &[f64] = &[0.0, 0.1, 0.3, 0.5, 1.0, 2.0, 5.0];

/// Every number knob at its former ladder.
pub(in crate::db::tuner::ticks) fn legacy() -> &'static Grids {
    static GRIDS: std::sync::OnceLock<Grids> = std::sync::OnceLock::new();
    GRIDS.get_or_init(|| {
        Grids::of([
            ("MShotPrice", Arc::from(PRICE)),
            ("MShotPriceMin", Arc::from(PRICE_MIN)),
            ("MShotRaiseWait", Arc::from(WAIT_S)),
            ("MShotReplaceDelay", Arc::from(WAIT_S)),
            ("MShotAddHourlyDelta", Arc::from(ADD)),
            ("MShotAdd3hDelta", Arc::from(ADD)),
            ("MShotAdd15minDelta", Arc::from(ADD)),
            ("MShotAdd5minDelta", Arc::from(ADD)),
            ("MShotAdd1minDelta", Arc::from(ADD)),
            ("MShotAdd24hDelta", Arc::from(ADD)),
            ("MShotAddMarkDelta", Arc::from(ADD)),
            ("MShotAddMarketDelta", Arc::from(ADD)),
            ("MShotAddBTCDelta", Arc::from(ADD)),
            ("MShotAddBTC5mDelta", Arc::from(ADD)),
            ("MShotAddPriceBug", Arc::from(ADD)),
            ("MShotAddDistance", Arc::from(DISTANCE)),
            ("SellPrice", Arc::from(SELL_PRICE)),
            ("MShotSellPriceAdjust", Arc::from(ADJUST)),
            ("HookSellLevel", Arc::from(HOOK_LEVEL)),
            ("SellDelay", Arc::from(SELL_DELAY_MS)),
            ("MaxModifier", Arc::from(MAX_MODIFIER)),
            ("PriceDownTimer", Arc::from(PD_TIMER_S)),
            ("PriceDownPercent", Arc::from(PD_PCT)),
            ("PriceDownDelay", Arc::from(PD_DELAY_S)),
            ("PriceDownAllowedDrop", Arc::from(DROP)),
            ("SellLevelDelay", Arc::from(SL_DELAY_S)),
            ("SellLevelDelayNext", Arc::from(SL_DELAY_S)),
            ("SellLevelTime", Arc::from(SL_TIME_S)),
            ("SellLevelCount", Arc::from(SL_COUNT)),
            ("SellLevelAdjust", Arc::from(DROP)),
            ("SellLevelAllowedDrop", Arc::from(DROP)),
            ("SellLevelWorkTime", Arc::from(SL_TIME_S)),
            ("StopLossDelay", Arc::from(STOP_DELAY_S)),
            ("StopLoss", Arc::from(STOP)),
            ("TimeToSwitch2Stop", Arc::from(SWITCH_S)),
            ("PriceToSwitch2Stop", Arc::from(SWITCH_PCT)),
            ("SecondStopLoss", Arc::from(STEP_LEVEL)),
            ("TimeToSwitchStop3", Arc::from(SWITCH_S)),
            ("PriceToSwitchStop3", Arc::from(SWITCH_PCT)),
            ("StopLoss3", Arc::from(STEP_LEVEL)),
            ("TrailingPercent", Arc::from(TRAILING)),
            ("TrailingEMA", Arc::from(TRAILING_EMA)),
            ("TakeProfit", Arc::from(TAKE_PROFIT)),
            ("SellModifier", Arc::from(SELL_MODIFIER)),
            ("StopLossModifier", Arc::from(STOP_MODIFIER)),
            ("Add1minDelta", Arc::from(DELTA_ADD)),
            ("Add5minDelta", Arc::from(DELTA_ADD)),
            ("Add15minDelta", Arc::from(DELTA_ADD)),
            ("AddHourlyDelta", Arc::from(DELTA_ADD)),
            ("Add3hDelta", Arc::from(DELTA_ADD)),
            ("Add24hDelta", Arc::from(DELTA_ADD)),
            ("AddMarketDelta", Arc::from(DELTA_ADD)),
            ("AddMarket24Delta", Arc::from(DELTA_ADD)),
            ("AddBTCDelta", Arc::from(DELTA_ADD)),
            ("AddBTC5mDelta", Arc::from(DELTA_ADD)),
            ("AddBTC1mDelta", Arc::from(DELTA_ADD)),
            ("AddMarkDelta", Arc::from(DELTA_ADD)),
            ("AddPump1h", Arc::from(DELTA_ADD)),
            ("AddDump1h", Arc::from(DELTA_ADD)),
            ("AddPriceBug", Arc::from(DELTA_ADD)),
        ])
    })
}
