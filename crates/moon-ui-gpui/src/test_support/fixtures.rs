//! Deterministic, in-memory scale fixtures for UI benchmarks.

use moon_core::config::{AppConfig, Secret, ServerConfig};
use moon_core::db::tuner::ticks::Deal;
use moon_core::db::{ChartTradeRecord, QuoteCurrency};
use moon_core::feed::{AssetRow, AssetsSnapshot, ConnStatus, DetectRow, FeedMsg, OrderRow, StrategyRow};
use moon_core::market::ChartCandle;
use moon_core::session::CoreStore;

/// Fixed minute-aligned origin for bars and the comparison clock of time-sensitive probes.
pub(crate) const EPOCH_MS: i64 = 1_700_000_040_000;

/// The feed's strategy-kind ordinal for MoonShot.
const MOONSHOT_KIND: u8 = 6;

/// Use letter names for the first 26 cores and numeric names for the rest.
fn core_name(index: usize) -> String {
    if index < 26 {
        format!("CORE_{}", char::from(b'A' + index as u8))
    } else {
        format!("CORE_{}", index + 1)
    }
}

/// Assign unique synthetic coins within each generated sequence.
fn coin_name(index: usize) -> String {
    match index {
        0 => "XALPHA".into(),
        1 => "XBETA".into(),
        _ => format!("XA{}", index + 1),
    }
}

/// Name a strategy or group with a neutral numbered unit.
fn unit_name(index: usize) -> String {
    format!("UNIT_{:02}", index + 1)
}

/// Build one feed-backed store. Orders, assets, and detects are fleet totals distributed evenly.
/// Detects use the real per-core ring, so entries above its retention limit are evicted.
pub(crate) fn core_store(
    cores: usize,
    strategies_per_core: usize,
    orders: usize,
    assets: usize,
    detects: usize,
) -> CoreStore {
    let mut store = CoreStore::default();
    for index in 0..cores {
        let id = (index + 1) as u64;
        store.ensure(id);
        let core = store.core_mut(id).expect("the fixture core was inserted");
        let rows_here = |total| total / cores + usize::from(index < total % cores);

        core.apply(FeedMsg::Status(ConnStatus::Ready));
        let strategies: Vec<_> = (0..strategies_per_core).map(strategy_row).collect();
        let order_rows: Vec<_> = (0..rows_here(orders))
            .map(|row| order_row(row, &strategies))
            .collect();
        core.apply(FeedMsg::Strategies(strategies));
        core.apply(FeedMsg::Orders(order_rows.into()));
        let rows: Vec<_> = (0..rows_here(assets)).map(asset_row).collect();
        let markets = rows.iter().map(|row| row.market.clone()).collect();
        core.apply(FeedMsg::Assets(AssetsSnapshot {
            rows,
            markets: std::sync::Arc::new(markets),
            base_currency: "USDT".into(),
            ..AssetsSnapshot::default()
        }));
        let detects = (0..rows_here(detects)).map(detect_row).collect();
        core.apply(FeedMsg::Detects(detects));
    }
    store
}

/// Give the Strategies tree folder entries with stable ids.
fn strategy_row(index: usize) -> StrategyRow {
    StrategyRow {
        id: (index + 1) as u64,
        name: unit_name(index),
        kind: "MoonShot".into(),
        kind_ordinal: MOONSHOT_KIND,
        folder_path: unit_name(index / 10),
        checked: index.is_multiple_of(2),
        is_short: false,
        fields: Vec::new(),
    }
}

/// Create a filled order and link it to a strategy when the core has one.
fn order_row(index: usize, strategies: &[StrategyRow]) -> OrderRow {
    let coin = coin_name(index);
    let market = format!("{coin}USDT");
    let price = 10.0 + (index.wrapping_mul(37) % 251) as f64 / 100.0;
    let strategy = (!strategies.is_empty()).then(|| &strategies[index % strategies.len()]);
    OrderRow {
        market_display: market.clone(),
        market,
        coin,
        quote: "USDT".into(),
        is_short: false,
        size: 1.0,
        remaining_size: 1.0,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: price,
        sell_price: price + 2.0,
        create_time_ms: (EPOCH_MS + index as i64) as f64,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price: (price + 1.0) as f32,
        fill_pct: 100.0,
        strat: strategy.map_or_else(String::new, |row| row.kind.clone()),
        strat_name: strategy.map_or_else(String::new, |row| row.name.clone()),
        strat_id: strategy.map_or(0, |row| row.id),
        status: "BuyDone".into(),
        uid: (index + 1) as u64,
        emulator: index.is_multiple_of(2),
        job_is_done: false,
        pending: false,
        filled: true,
        stop_loss: None,
        trailing: None,
        take_profit: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: None,
        sell_trace: None,
    }
}

/// Build a spot holding above the minimum lot so the Assets filter keeps it.
fn asset_row(index: usize) -> AssetRow {
    let coin = coin_name(index);
    let qty = 1.0 + (index % 100) as f64;
    AssetRow {
        market: format!("{coin}USDT"),
        coin,
        quote: "USDT".into(),
        listed: 1,
        qty,
        qty_full: qty,
        price: 10.0,
        value_usdt: qty * 10.0,
        min_lot_usd: 5.0,
        is_quote_asset: false,
        mark_price: 0.0,
        pos_size: 0.0,
        pos_price: 0.0,
        liq_price: 0.0,
        leverage: 0,
        pnl_usdt: 0.0,
        pnl_live: false,
    }
}

/// Build a distinct market signal admitted by the Detects filter at the fixture epoch.
fn detect_row(index: usize) -> DetectRow {
    DetectRow {
        seq: 0,
        market: format!("{}USDT", coin_name(index)),
        time_ms: (EPOCH_MS + index as i64) as f64,
        sound_alert: true,
        keep_alert_secs: 60,
        add_to_chart: 0,
        keep_in_chart_secs: 60,
        sound_name: None,
        is_alert: false,
        kind: MOONSHOT_KIND,
        is_short: false,
        msg: String::new(),
        strat_name: unit_name(index),
    }
}

/// Build a file-free config with ids matching the store fixture and blank credentials.
pub(crate) fn app_config(cores: usize) -> AppConfig {
    let servers = (0..cores)
        .map(|index| {
            let uid = (index + 1) as u64;
            ServerConfig {
                id: uid,
                uid,
                name: core_name(index),
                active: true,
                feed: Default::default(),
                key: Secret::new(""),
                endpoint_override: String::new(),
                endpoint_to_station: false,
                group: unit_name(index / 10),
                market: "XALPHAUSDT".into(),
                color: [1, 2, 3],
                synthetic: false,
                chart_bundle: String::new(),
                default_alert_strategy: 0,
                own_trade_config: false,
                strat_slots: None,
                manual_strategy: None,
                trade: None,
                transport: None,
                workspace_membership: Default::default(),
                total_mode: Default::default(),
            }
        })
        .collect();
    AppConfig::headless(servers)
}

/// Build the tuner model's closed deals in ascending close order.
pub(crate) fn deals(n: usize) -> Vec<Deal> {
    (0..n)
        .map(|index| {
            let buy_ms = EPOCH_MS + index as i64 * 60_000;
            Deal {
                report_uid: index as i64 + 1,
                core_uid: 1,
                core_name: core_name(0),
                strategy_id: 1,
                kind: "MoonShot".into(),
                coin: coin_name(index % 2),
                buy_ms,
                buy_set_ms: Some(buy_ms - 500),
                corridor: None,
                close_ms: buy_ms + 15_000,
                buy_price: 10.0,
                sell_price: 12.0,
                spent: 10.0,
                sizing: None,
                pnl_pct: false,
                is_short: false,
                sell_reason: "Sell Price".into(),
                fact_pnl: 2.0,
                profit: Some(2.0),
                deltas: Default::default(),
                delta_track: None,
                quotes: None,
                bars: None,
                tick: None,
                pre_spike_ask: None,
                archived_take: None,
                fact_modifier: None,
                entry_placed: None,
                hook_depth_pct: None,
                drops_detect: None,
                hook_stated_take_pct: None,
                strike_detect: None,
                step_lag_ms: 0.0,
                round_trip_ms: None,
                leverage: None,
                stop_anchor: None,
                own_entry: None,
                gap: None,
            }
        })
        .collect()
}

/// Build recent-first report records on one market for chart and trade-window history.
pub(crate) fn history(n: usize) -> Vec<ChartTradeRecord> {
    (0..n)
        .rev()
        .map(|index| {
            let id = index as i64 + 1;
            let buy_ms = EPOCH_MS + index as i64 * 60_000;
            let close_ms = buy_ms + 15_000;
            ChartTradeRecord {
                record_id: id,
                core_uid: 1,
                coin: "XALPHA".into(),
                buy_date: buy_ms / 1000,
                close_date: close_ms / 1000,
                buy_ms: Some(buy_ms),
                close_ms: Some(close_ms),
                sell_set_date: 0,
                sell_set_ms: None,
                buy_set_ms: None,
                corridor: None,
                buy_price: 10.0,
                sell_price: 12.0,
                quantity: 1.0,
                is_short: false,
                emulator: false,
                profit: Some(2.0),
                profit_pct: Some(20.0),
                report_uid: Some(id),
                quote: Some(QuoteCurrency::usdt()),
            }
        })
        .collect()
}

/// Build ascending one-minute bars with varying price ranges, ready for the chart source.
pub(crate) fn candles(n: usize) -> Vec<ChartCandle> {
    (0..n)
        .map(|index| {
            let open = 10.0 + (index % 100) as f32 / 100.0;
            ChartCandle {
                t_open_ms: (EPOCH_MS + index as i64 * 60_000) as f64,
                open,
                high: open + 0.25,
                low: open - 0.25,
                close: open + 0.1,
                volume: 1.0,
                quote_volume: open,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
