//! What a synthetic core reports about its account: balances, open orders, license, health.
//!
//! Sent with the settings and strategies of [`super::recorded`], under the same switch and for the
//! same reason: a capture run photographs every screen, and a panel with nothing to show reads as a
//! catalogue of empty states. A live core delivers each of these as a [`FeedMsg`], so they go down
//! the same channel and the panels cannot tell the difference; nothing here is a display mode.
//!
//! **Every number is invented.** No balance, order or key comes from anybody's account, and the
//! address is from the documentation range. They are chosen to look ordinary and stay fixed, so two
//! runs photograph the same application. A render bench and `--fixture` never reach this module.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use super::super::{
    ApiKeyExpiry, AssetRow, AssetsSnapshot, ChartTextRows, CoreEndpoint, CoreProblem,
    CoreProblemCategory, CoreProblems, CoreSysStatus, CoreTelegramService, CoreTelegramState,
    DetectRow, FeedMsg, FeedTx, GlobalBalanceRow, LicenseState, OrderRow, TransferAssetRow,
    TransferAssetsSnapshot,
};
use crate::util::now_unix_ms as now_ms;

/// Milliseconds in a day, for the dates the account reports relative to now.
const DAY_MS: f64 = 86_400_000.0;

/// Coins held beside the charted market, as `(coin, quantity, price)`.
///
/// Their markets are in the snapshot's catalog, so each row offers what a real holding does.
const HOLDINGS: [(&str, f64, f64); 3] = [
    ("AKE", 412_000.0, 0.000_31),
    ("BANK", 1_850.0, 0.052),
    ("ESPORTS", 960.0, 0.118),
];

/// The filter lines a core paints on its chart for a market no strategy can enter yet.
const CHART_FILTERS: [&str; 3] = [
    "Daily vol. doesn't match: 1.2M < 5M",
    "Hourly delta is too low: 0.8% < 2%",
    "Strategy is waiting for the BTC filter",
];

/// The sample news: English, Russian and the one tag each item is about.
const SAMPLE_NEWS: [(&str, &str, &str); 4] = [
    (
        "The exchange will list the token on its spot market today and open a USDT pair.",
        "Биржа сегодня добавит токен на спотовый рынок и откроет пару к USDT.",
        "listing",
    ),
    (
        "The project announced a mainnet upgrade and a token swap for existing holders.",
        "Проект объявил об обновлении основной сети и обмене токенов для текущих держателей.",
        "upgrade",
    ),
    (
        "Deposits and withdrawals of the token are paused for scheduled network maintenance.",
        "Ввод и вывод токена приостановлены на время плановых работ в сети.",
        "maintenance",
    ),
    (
        "The exchange reduces maximum leverage on the perpetual contract from 50x to 25x.",
        "Биржа снижает максимальное плечо по бессрочному контракту с 50x до 25x.",
        "leverage",
    ),
];

/// Send what a core reports about its account, once, as a live core does when it connects. Its
/// detects go separately, through [`detects`]: the feed has to place them after its own.
///
/// A send fails only when the terminal is shutting the feed down, and the feed's own loop notices
/// that on its next tick.
///
/// Args:
///     core_uid: The core these belong to; varies the figures so two cores do not read as one.
///     market: The market the core is serving, which its position and orders sit on.
///     price: Its starting price, which order lines are placed around.
///     strategy: Id and name of a strategy of this core, for its orders; empty when it has none.
///     now: The moment everything here is stamped with, in Unix milliseconds.
///     tx: The feed's channel to the terminal.
pub(super) fn send_account_state(
    core_uid: u64,
    market: &str,
    price: f64,
    strategy: &(u64, String),
    now: f64,
    tx: &FeedTx,
) {
    let _ = tx.send(FeedMsg::Assets(assets(core_uid, market, price)));
    let _ = tx.send(FeedMsg::TransferAssets(transfer_assets(core_uid)));
    let _ = tx.send(FeedMsg::License(license(core_uid, now)));
    let _ = tx.send(FeedMsg::CoreVersion {
        version: 772,
        suffix: Some(String::new()),
    });
    let _ = tx.send(FeedMsg::Endpoint(CoreEndpoint {
        address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10 + (core_uid % 200) as u8)),
        port: 5017,
    }));
    send_health(core_uid, tx);
    let _ = tx.send(FeedMsg::ApiExpiry(api_expiry(core_uid, now)));
    let _ = tx.send(FeedMsg::Problems(problems(core_uid, now)));
    send_telegram(tx);
    let _ = tx.send(FeedMsg::ChartText(vec![ChartTextRows {
        market: market.to_string(),
        filter_lines: CHART_FILTERS.iter().map(|line| line.to_string()).collect(),
    }]));
    let rows = orders(core_uid, market, price, strategy, now);
    let _ = tx.send(FeedMsg::Orders(rows.clone().into()));
    let _ = tx.send(FeedMsg::OrderLines(rows.into()));
}

/// Send the core's current health reading.
pub(super) fn send_health(core_uid: u64, tx: &FeedTx) {
    let _ = tx.send(FeedMsg::SysStatus(sys_status(core_uid, now_ms())));
}

/// Send the Telegram snapshot, which is also what answers a panel waiting on that state.
pub(super) fn send_telegram(tx: &FeedTx) {
    let _ = tx.send(FeedMsg::Telegram(Some(Arc::new(telegram()))));
}

/// `(coin, quote)` of a market named `<coin>USDT`, which is all a synthetic core serves.
fn split_market(market: &str) -> (&str, &str) {
    match market.strip_suffix("USDT") {
        Some(coin) => (coin, "USDT"),
        None => (market, ""),
    }
}

/// The Assets window: an open position on the served market and a few other holdings.
///
/// `usd_rate_known` is what lets the panel price any of it; without it every total reads as
/// unpriced, whatever the rows hold.
fn assets(core_uid: u64, market: &str, price: f64) -> AssetsSnapshot {
    let scale = 1.0 + (core_uid % 5) as f64 * 0.25;
    let (coin, quote) = split_market(market);
    // What `orders` nets to on this market: the long's 1500 less the short's 900.
    let position = 600.0;
    let entry = price * 0.985;
    let mut rows = vec![AssetRow {
        market: market.to_string(),
        coin: coin.to_string(),
        quote: quote.to_string(),
        listed: 3,
        qty: position,
        qty_full: position,
        price,
        value_usdt: position * price,
        min_lot_usd: 5.0,
        is_quote_asset: false,
        mark_price: price,
        pos_size: position,
        pos_price: entry,
        liq_price: price * 0.71,
        leverage: 10,
        pnl_usdt: (price - entry) * position,
        pnl_live: true,
    }];
    for (held, qty, held_price) in HOLDINGS {
        let qty = qty * scale;
        rows.push(AssetRow {
            market: format!("{held}USDT"),
            coin: held.to_string(),
            quote: "USDT".to_string(),
            listed: 1,
            qty,
            qty_full: qty,
            price: held_price,
            value_usdt: qty * held_price,
            min_lot_usd: 5.0,
            is_quote_asset: false,
            mark_price: 0.0,
            pos_size: 0.0,
            pos_price: 0.0,
            liq_price: 0.0,
            leverage: 0,
            pnl_usdt: 0.0,
            pnl_live: false,
        });
    }
    let markets: HashSet<String> = rows.iter().map(|row| row.market.clone()).collect();
    let leverage: HashMap<String, i32> = rows
        .iter()
        .filter(|row| row.leverage > 0)
        .map(|row| (row.market.clone(), row.leverage))
        .collect();
    let held: f64 = rows.iter().map(|row| row.value_usdt).sum();
    let free = 3_120.0 * scale;
    AssetsSnapshot {
        rows,
        global: GlobalBalanceRow {
            free_usdt: free,
            total_usdt: free + held,
            pnl_usdt: (price - entry) * position,
            usd_rate_known: true,
            ..Default::default()
        },
        futures_account: true,
        base_currency: "USDT".to_string(),
        markets: Arc::new(markets),
        leverage,
    }
}

/// The wallets of the transfer tree: cash on spot and futures, nothing on quarterly.
fn transfer_assets(core_uid: u64) -> TransferAssetsSnapshot {
    let row = |amount: f64| TransferAssetRow {
        currency: "USDT".to_string(),
        amount,
        total: amount,
        value_usdt: amount,
    };
    let scale = 1.0 + (core_uid % 5) as f64 * 0.25;
    TransferAssetsSnapshot {
        spot: vec![row(640.0 * scale)],
        futures: vec![row(3_120.0 * scale)],
        quarterly: Vec::new(),
    }
}

/// A paid core with a running news subscription, so no panel opens on a "buy this" state.
fn license(core_uid: u64, now: f64) -> LicenseState {
    LicenseState {
        paid_version: true,
        reg_id: 100_000 + core_uid as i32,
        moon_credits: 1_500,
        moon_credits_hold: 40,
        moon_credits_auction: 0,
        can_use_watcher: true,
        news_valid_until: Some((now + 90.0 * DAY_MS) as i64),
        news_trial_used: true,
    }
}

/// Healthy telemetry: every Core Status column has a figure and none of them is alarming.
fn sys_status(core_uid: u64, now: f64) -> CoreSysStatus {
    let extra = (core_uid % 5) as u8;
    CoreSysStatus {
        process_cpu_percent: Some(6 + extra),
        system_cpu_percent: Some(18 + extra),
        used_memory_mb: Some(740 + u16::from(extra) * 30),
        free_physical_memory_mb: Some(5_200),
        logical_cpu_count: Some(8),
        round_trip_ms: Some(34 + u32::from(extra)),
        order_api_latency_ms: Some(112 + u16::from(extra) * 5),
        updated_ms: now as i64,
    }
}

/// An API key with months left: a known date and the count the core reports beside it.
fn api_expiry(core_uid: u64, now: f64) -> ApiKeyExpiry {
    let days = 120 + (core_uid % 5) as i32 * 7;
    ApiKeyExpiry {
        unlimited: false,
        known: true,
        days_left: Some(days),
        at_unix: Some((now / 1_000.0) as i64 + i64::from(days) * 86_400),
        checked_ms: now as i64,
    }
}

/// The core's own findings: one ordinary one on every fourth uid, so the recorded cores 1 and 3 show
/// the panel both of its states.
fn problems(core_uid: u64, now: f64) -> CoreProblems {
    let items = (core_uid % 4 == 3)
        .then(|| CoreProblem {
            kind: 1,
            kind_name: "ClockDrift".to_string(),
            category: CoreProblemCategory::Machine,
            title: "Clock drift".to_string(),
            message: "The machine's clock is 2 seconds off the exchange's.".to_string(),
            technical_details: String::new(),
            first_seen_ms: Some((now - DAY_MS) as i64),
            confirmed_ms: Some(now as i64),
            confirmations: 3,
        })
        .into_iter()
        .collect();
    CoreProblems {
        supported: true,
        items,
    }
}

/// A signed-in Telegram reader. No phone: the panel then says "signed in" without a number.
fn telegram() -> CoreTelegramState {
    CoreTelegramState {
        enabled: true,
        proxy_type: Some(0),
        client_state: Some("connected".to_string()),
        service_online: true,
        state_supported: true,
        service_version: Some("1.8.0".to_string()),
        service: Some(CoreTelegramService {
            auth_state: "ready".to_string(),
            connection: "ready".to_string(),
            ..Default::default()
        }),
        settings_supported: true,
        ..Default::default()
    }
}

/// The open orders: a position with its exit and stops, a resting entry, and a short.
///
/// Prices are placed around the served price so each line lands on the chart, and the kinds are the
/// ones a reader needs to tell apart there.
fn orders(
    core_uid: u64,
    market: &str,
    price: f64,
    strategy: &(u64, String),
    now: f64,
) -> Vec<OrderRow> {
    let base = core_uid * 1_000;
    let long_entry = price * 0.985;
    let short_entry = price * 1.012;
    vec![
        OrderRow {
            size: 1_500.0,
            remaining_size: 1_500.0,
            buy_price: long_entry,
            sell_price: price * 1.018,
            create_time_ms: now - 40.0 * 60_000.0,
            sell_create_time_ms: now - 35.0 * 60_000.0,
            entry_fill_time_ms: now - 35.0 * 60_000.0,
            fill_pct: 100.0,
            status: "SellSet".to_string(),
            filled: true,
            sl_on: true,
            ts_on: true,
            stop_loss: Some(price * 0.955),
            trailing: Some(price * 0.972),
            take_profit: Some(price * 1.018),
            uid: base + 1,
            ..order_base(market, price, strategy)
        },
        OrderRow {
            size: 1_200.0,
            buy_price: price * 0.968,
            create_time_ms: now - 12.0 * 60_000.0,
            status: "BuySet".to_string(),
            pending: true,
            uid: base + 2,
            ..order_base(market, price, strategy)
        },
        OrderRow {
            is_short: true,
            size: 900.0,
            remaining_size: 900.0,
            buy_price: short_entry,
            sell_price: price * 0.984,
            create_time_ms: now - 25.0 * 60_000.0,
            sell_create_time_ms: now - 20.0 * 60_000.0,
            entry_fill_time_ms: now - 20.0 * 60_000.0,
            fill_pct: 100.0,
            status: "SellSet".to_string(),
            filled: true,
            sl_on: true,
            stop_loss: Some(price * 1.041),
            take_profit: Some(price * 0.984),
            uid: base + 3,
            ..order_base(market, price, strategy)
        },
    ]
}

/// What every order of this core has in common; the callers state what makes each one itself.
fn order_base(market: &str, price: f64, strategy: &(u64, String)) -> OrderRow {
    let (coin, quote) = split_market(market);
    OrderRow {
        market: market.to_string(),
        market_display: market.to_string(),
        coin: coin.to_string(),
        quote: quote.to_string(),
        is_short: false,
        size: 0.0,
        remaining_size: 0.0,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: 0.0,
        sell_price: 0.0,
        create_time_ms: 0.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price: price as f32,
        fill_pct: 0.0,
        strat: "Spread".to_string(),
        // The panel prints the name only beside an id: it is what the name's actions act on.
        strat_name: strategy.1.clone(),
        strat_id: strategy.0,
        status: String::new(),
        uid: 0,
        emulator: false,
        job_is_done: false,
        pending: false,
        filled: false,
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

/// Detect cards: rows the panel keeps, because each carries a sound alert and asks for no chart tab.
pub(super) fn detects(market: &str, strategy: &str, now: f64) -> Vec<DetectRow> {
    [
        ("Delta +4.2% in 5 min", 18, 4.0),
        ("Volume spike x3.1", 18, 2.0),
    ]
    .into_iter()
    .map(|(msg, kind, minutes)| DetectRow {
        seq: 0,
        market: market.to_string(),
        time_ms: now - minutes * 60_000.0,
        sound_alert: true,
        keep_alert_secs: 86_400,
        add_to_chart: 0,
        keep_in_chart_secs: 0,
        sound_name: None,
        is_alert: false,
        kind,
        is_short: false,
        msg: msg.to_string(),
        strat_name: strategy.to_string(),
    })
    .collect()
}

/// The text of one sample news item, in English and Russian, repeating through a short list.
///
/// The Ctrl card on the chart and the News panel both print it, so it reads as ordinary news rather
/// than as a test line. No item names a real exchange or a real event.
///
/// Args:
///     id: The item's number in the stream.
///
/// Returns:
///     The English text, the Russian text and the item's tag.
pub(super) fn sample_news(id: u64) -> (String, String, String) {
    let (en, ru, tag) = SAMPLE_NEWS[(id % SAMPLE_NEWS.len() as u64) as usize];
    (en.to_string(), ru.to_string(), tag.to_string())
}

/// Every tag the sample news carries, for the catalog the News panel's filter offers.
///
/// Returns:
///     The tags, in the order the items first use them.
pub(super) fn sample_news_tags() -> Vec<String> {
    SAMPLE_NEWS
        .iter()
        .map(|(_, _, tag)| tag.to_string())
        .collect()
}

#[cfg(test)]
mod tests;
