//! Unit regressions for Mini App targeting, ordering and safe entry-notional display.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use moon_core::telegram::notify::{
    ChatNotify, CoreScope, NotifyFile, NotifyLedger, NotifySettings,
};
use moon_core::telegram::runtime::NotifyStore;
use moon_core::venue::CoreVenue;

use super::settings::{
    SaveFault, SaveResult, keep_stored_bot_fields, prepare_settings, save_fault_text,
    store_settings,
};

use std::time::{Duration, Instant};

use moon_core::telegram::web::dto::StrategyPendingDto;

use moon_core::feed::{
    AssetRow, AssetsSnapshot, OrderRow, TransferAssetRow, TransferAssetsSnapshot,
};
use moon_core::session::BalanceState;
use moon_core::session::balances::BalanceFigures;
use moon_core::telegram::web::dto::BalanceStateDto;

use super::dto::{
    balance_state_dto, distance_text, entry_volume_text, order_quote, order_to_entry_pct, pnl_sign,
    pnl_text, pnl_usd, strategy_pending, trade_strategy, trade_volume_text,
};
use super::reads::coin_rows;
use super::{by_section, natural_cmp, scope_targets};

/// `mini_app/mod.rs:scope_targets` keeps only visible cores, in visible order, once each.
///
/// Mutation: return the requested ids unfiltered or skip the dedupe. A mass
/// trading switch then commands a core the owner does not see, or the same core
/// twice. Oracle: requested [3, 99, 1, 3] against visible [1, 2, 3] is [1, 3].
#[test]
fn scope_targets_drops_unknown_and_duplicate_cores() {
    assert_eq!(scope_targets(&[3, 99, 1, 3], &[1, 2, 3]), vec![1, 3]);
    assert_eq!(scope_targets(&[99], &[1, 2, 3]), Vec::<u64>::new());
}

/// `mini_app/mod.rs:natural_cmp` reads digit runs as numbers and ignores letter case.
///
/// Mutation: compare the raw strings. "Account № 10" then sorts before "№ 9".
#[test]
fn natural_cmp_orders_numbers_by_value() {
    assert_eq!(natural_cmp("Account № 9", "Account № 10"), Ordering::Less);
    assert_eq!(natural_cmp("core 21", "core 3"), Ordering::Greater);
    assert_eq!(natural_cmp("alpha", "Beta"), Ordering::Less);
    assert_eq!(natural_cmp("core 007", "core 7"), Ordering::Less);
    assert_eq!(natural_cmp("core", "core 1"), Ordering::Less);
    assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
}

/// `mini_app/mod.rs:by_section` groups by the terminal's exchange sections and natural-sorts names.
///
/// Mutation: skip the in-section sort, or keep the input order across sections. Oracle: the
/// unidentified core leads (the terminal's unknown-first rule), each venue's cores stay together,
/// and "№ 9" precedes "№ 10" inside its section.
#[test]
fn by_section_groups_by_exchange_then_natural_name() {
    let venues = HashMap::from([
        (1, CoreVenue::identify(6, "", None)),
        (2, CoreVenue::identify(2, "", None)),
        (3, CoreVenue::identify(6, "", None)),
        (4, CoreVenue::identify(2, "", None)),
    ]);
    let rows = vec![
        (1u64, "№ 10".to_string()),
        (2, "b".to_string()),
        (3, "№ 9".to_string()),
        (5, "lost".to_string()),
        (4, "A".to_string()),
    ];
    let ordered = by_section(rows, &venues, |(id, _)| *id, |(_, name)| name);
    let ids: Vec<u64> = ordered.iter().map(|(_, (id, _))| *id).collect();
    assert_eq!(ids[0], 5, "the unidentified core leads, as in the terminal");
    // Section order oracle: the terminal's own partition of the two venues.
    let terminal = moon_core::session::core_order::exchange_sections([
        (2, venues.get(&2)),
        (6, venues.get(&1)),
    ]);
    let first_is_code_2 = terminal[0].1 == [2];
    let expected: [u64; 4] = if first_is_code_2 {
        [4, 2, 3, 1]
    } else {
        [3, 1, 4, 2]
    };
    assert_eq!(
        &ids[1..],
        &expected,
        "terminal section order, natural names inside"
    );
    assert_eq!(ordered[1].0, ordered[2].0);
    assert_ne!(ordered[2].0, ordered[3].0);
}

/// `mini_app/dto.rs:strategy_pending` keeps a toggle Pending inside the 45 s window, then TimedOut
/// only while the row disagrees AND no fresh strategy list arrived.
///
/// Mutation: `if checked == wanted || rev_now != rev_before {` -> `if checked == wanted {`.
/// A core that rebuilt its strategy list without flipping the row then keeps the timed-out chip
/// forever. Oracle: the documented window (45 s) and the settle rule, not the function's output.
#[test]
fn strategy_pending_times_out_until_row_agrees_or_list_moves() {
    let sent = Instant::now();
    let entry = (true, sent, 5, 7);
    let at = |secs| sent + Duration::from_secs(secs);
    assert_eq!(
        strategy_pending(entry, 5, 7, false, at(44)),
        Some(StrategyPendingDto::Pending)
    );
    assert_eq!(
        strategy_pending(entry, 5, 7, false, at(46)),
        Some(StrategyPendingDto::TimedOut)
    );
    assert_eq!(strategy_pending(entry, 5, 7, true, at(46)), None);
    assert_eq!(
        strategy_pending(entry, 5, 8, false, at(46)),
        None,
        "a fresh strategy list after the window is the truth"
    );
}

/// Build a resting long entry at `entry` with the mark at `mark`, no fill yet.
fn resting_order(entry: f64, mark: f32) -> OrderRow {
    OrderRow {
        market: "LINKUSDT".into(),
        market_display: "LINKUSDT".into(),
        coin: "LINK".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 10.0,
        remaining_size: 0.0,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: entry,
        sell_price: 0.0,
        create_time_ms: 0.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price: mark,
        fill_pct: 0.0,
        strat: "test".into(),
        strat_name: String::new(),
        strat_id: 1,
        status: String::new(),
        uid: 1,
        emulator: false,
        job_is_done: false,
        pending: true,
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

/// `mini_app/dto.rs:order_to_entry_pct` states the distance to a resting entry, by side, and only
/// while the order holds no position.
///
/// Mutation: drop the position gate, or ignore the side. A filled order then shows a distance
/// beside its PnL, or a short's entry above the market reads as already passed.
/// Oracle: long entry 100 at mark 110 is +10 %, short entry 100 at mark 80 is +20 %.
#[test]
fn order_to_entry_pct_measures_resting_entries_only() {
    let long = resting_order(100.0, 110.0);
    assert_eq!(order_to_entry_pct(&long), Some(10.0));

    let mut short = resting_order(100.0, 80.0);
    short.is_short = true;
    assert_eq!(order_to_entry_pct(&short), Some(20.0));

    let mut filled = resting_order(100.0, 110.0);
    filled.filled = true;
    filled.fill_pct = 100.0;
    assert_eq!(order_to_entry_pct(&filled), None);

    assert_eq!(order_to_entry_pct(&resting_order(100.0, 0.0)), None);
}

#[test]
fn the_telegram_log_prefix_still_matches_this_module() {
    // moon-core raises this prefix to `info` by default but cannot verify it from its own side;
    // renaming the crate would mute the owner-command lines again.
    let prefix = moon_core::diagnostics::TELEGRAM_TARGET;
    assert!(
        module_path!().starts_with(prefix),
        "the default filter raises {prefix:?}, but this module logs as {:?}",
        module_path!()
    );
}

#[test]
fn distance_to_entry_text_carries_no_sign() {
    assert_eq!(distance_text(1.5).as_deref(), Some("1.50%"));
    assert_eq!(distance_text(-1.5).as_deref(), Some("1.50%"));
}

/// Replacing size-times-entry with coin quantity, hard-coding dollars, or rounding BTC to cents
/// misstates a position on the phone. The independent notionals are 2 * 617 = 1234 and 3 * .004.
#[test]
fn entry_volume_uses_entry_notional_and_native_quote() {
    assert_eq!(
        entry_volume_text(2.0, 617.0, 1.0, "USDT").as_deref(),
        Some("1 234.0$")
    );
    assert_eq!(
        entry_volume_text(3.0, 0.004, 1.0, "BTC").as_deref(),
        Some("0.012 BTC")
    );
    assert_eq!(
        entry_volume_text(2.0, 240.0, 1.0, "USDC").as_deref(),
        Some("480 USDC")
    );
    assert_eq!(
        entry_volume_text(2.0, 250.0, 1.0, "USD").as_deref(),
        Some("500.0$")
    );
    assert_eq!(
        entry_volume_text(3.0, 0.004, 50_000.0, "USDT").as_deref(),
        Some("600.0$")
    );
}

/// Defaulting a missing entry/rate to zero or an unknown quote to dollars fabricates a figure;
/// overflow must also stay absent. Known COIN-M USD quotes are covered by the positive case.
#[test]
fn entry_volume_withholds_unavailable_or_invalid_inputs() {
    for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(entry_volume_text(2.0, invalid, 1.0, "USDT"), None);
        assert_eq!(entry_volume_text(invalid, 100.0, 1.0, "USDT"), None);
        assert_eq!(entry_volume_text(2.0, 100.0, invalid, "USDT"), None);
    }
    assert_eq!(entry_volume_text(f64::MAX, 2.0, 1.0, "USDT"), None);
    assert_eq!(entry_volume_text(2.0, 250.0, 1.0, ""), None);
    assert_eq!(entry_volume_text(2.0, 250.0, 1.0, "   "), None);
}

/// Using exit quantity or omitting the historical rate would show 1125 or 500 dollars instead
/// of the independently calculated 2 * 250 * .5 = 250; unavailable conversion shows no figure.
#[test]
fn closed_volume_uses_bought_quantity_and_safe_rate() {
    let mut trade = crate::report::MiniTrade {
        core_uid: 2,
        rec_id: 1,
        coin: "DEMO".into(),
        core_name: "Demo".into(),
        is_short: true,
        profit_usdt: Some(10.0),
        pct: Some(2.0),
        buy_utc: Some(100),
        close_utc: 150,
        buy_price: 250.0,
        sell_price: 260.0,
        quantity: 9.0,
        bought_quantity: Some(2.0),
        entry_volume_rate: Some(0.5),
        strategy_id: Some(0),
        channel_name: String::new(),
    };
    assert_eq!(trade_volume_text(&trade).as_deref(), Some("250.0$"));
    trade.entry_volume_rate = None;
    assert_eq!(trade_volume_text(&trade), None);
    trade.entry_volume_rate = Some(0.5);
    trade.bought_quantity = None;
    assert_eq!(trade_volume_text(&trade), None);
    trade.bought_quantity = Some(2.0);
    trade.buy_price = 0.0;
    assert_eq!(trade_volume_text(&trade), None);
}

/// `mini_app/dto.rs:trade_strategy` names strategy trades, marks only `0` manual, and leaves a missing
/// id unknown.
///
/// Mutation: drop negative ids (`u64::try_from`), treat a missing id as manual, or skip the stored
/// name. A strategy whose Delphi-signed id is negative then shows as manual in the trade sheet, or
/// one the core no longer lists shows as a bare id. Oracle: id `-7` names the strategy listed
/// under `(-7i64) as u64`; an unlisted id with a stored name shows that name.
#[test]
fn trade_strategy_names_signed_ids_and_separates_manual_from_unknown() {
    let names = |sid: u64| match sid {
        5 => Some("Demo Alpha".to_string()),
        sid if sid == (-7i64) as u64 => Some("Demo Beta".to_string()),
        _ => None,
    };
    assert_eq!(
        trade_strategy(Some(5), "Demo Old", names),
        (Some("Demo Alpha".to_string()), false)
    );
    assert_eq!(
        trade_strategy(Some(-7), "", names),
        (Some("Demo Beta".to_string()), false)
    );
    assert_eq!(
        trade_strategy(Some(9), " Demo Gone ", names),
        (Some("Demo Gone".to_string()), false)
    );
    assert_eq!(
        trade_strategy(Some(9), "", names),
        (Some("#9".to_string()), false)
    );
    assert_eq!(trade_strategy(Some(0), "Demo Alpha", names), (None, true));
    assert_eq!(trade_strategy(None, "", names), (None, false));
}

/// One synthetic asset row. Unused position fields stay at zero.
fn asset_row(
    coin: &str,
    qty_full: f64,
    value_usdt: f64,
    min_lot_usd: f64,
    is_quote_asset: bool,
) -> AssetRow {
    AssetRow {
        market: format!("{coin}USDT"),
        coin: coin.to_string(),
        quote: "USDT".to_string(),
        listed: 1,
        qty: qty_full,
        qty_full,
        price: 1.0,
        value_usdt,
        min_lot_usd,
        is_quote_asset,
        mark_price: 0.0,
        pos_size: 0.0,
        pos_price: 0.0,
        liq_price: 0.0,
        leverage: 0,
        pnl_usdt: 0.0,
        pnl_live: false,
    }
}

fn reading(state: BalanceState, free: f64, total: f64) -> BalanceFigures {
    BalanceFigures { state, free, total }
}

/// `reads::coin_rows` prices a stale core's coins and withholds value when the reading is not usable.
///
/// Mutation: treat Stale as unusable, or attach a number while the core is Awaiting or Unpriced.
/// A stale finite figure is still a number the account total counts, and its state stays stale.
/// An awaiting or unpriced reading is not usable, so its coins contribute no value and the core
/// adds nothing to a total. Oracle: hardcoded `unpriced` and `12.50$`, plus [`BalanceFigures::usable`]
/// on the same inputs.
#[test]
fn coin_rows_prices_a_stale_core_and_withholds_an_unusable_one() {
    let _locale = crate::test_locale::force("en");
    let assets = AssetsSnapshot {
        rows: vec![asset_row("AAA", 1.5, 12.5, 1.0, false)],
        ..AssetsSnapshot::default()
    };
    let stale = reading(BalanceState::Stale, 10.0, 20.0);
    assert!(stale.usable());
    let rows = coin_rows(&assets, &TransferAssetsSnapshot::default(), "", stale);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].value, Some(12.5));
    assert_eq!(rows[0].value_text, "12.50$");
    assert_ne!(
        balance_state_dto(BalanceState::Stale),
        BalanceStateDto::Live
    );

    for state in [BalanceState::Unpriced, BalanceState::Awaiting] {
        let held = reading(state, 10.0, 20.0);
        assert!(!held.usable(), "{state:?} must not enter a total");
        let rows = coin_rows(&assets, &TransferAssetsSnapshot::default(), "", held);
        assert_eq!(rows[0].value, None);
        assert_eq!(rows[0].value_text, "unpriced");
    }

    let broken = reading(BalanceState::Stale, f64::NAN, 20.0);
    assert!(!broken.usable());
    assert_eq!(
        coin_rows(&assets, &TransferAssetsSnapshot::default(), "", broken)[0].value,
        None
    );
}

/// `reads::coin_rows` drops a row only when its USDT value is strictly below the min lot.
///
/// Mutation: drop the equal case, keep dust, hide quote-currency market rows, or treat a zero
/// value as dust. A holding exactly at the lot stays; a smaller priced one does not. A
/// quote-currency market row above the lot stays, because this list is the core's coins. A
/// zero value means the rate is unknown, so the row stays unpriced even when the lot is
/// positive. Oracle: input coins KEEP, DUST, USDT, ZERO; DUST is the only one absent.
#[test]
fn coin_rows_drops_dust_below_the_min_lot() {
    let _locale = crate::test_locale::force("en");
    let assets = AssetsSnapshot {
        rows: vec![
            asset_row("KEEP", 2.0, 2.0, 2.0, false),
            asset_row("DUST", 0.1, 0.5, 1.0, false),
            asset_row("USDT", 9.0, 9.0, 1.0, true),
            asset_row("ZERO", 3.0, 0.0, 5.0, false),
        ],
        ..AssetsSnapshot::default()
    };
    let rows = coin_rows(
        &assets,
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    let coins: Vec<&str> = rows.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(coins, vec!["USDT", "KEEP", "ZERO"]);
    assert_eq!(rows[0].value, Some(9.0));
    assert_eq!(rows[0].value_text, "9.00$");
    assert_eq!(rows[1].value, Some(2.0));
    assert_eq!(rows[2].value, None);
    assert_eq!(rows[2].value_text, "unpriced");
}

/// `reads::coin_rows` carries a usable core's coin values, largest first, unpriced last.
///
/// Mutation: sort ascending, drop a positive value, or price a zero. Equal priced values keep
/// input order. Oracle: 40, then 12.5, then the two 7s in input order, then the zero row.
/// Quantity text for 1.5 is `1.5` and for 35483 is `35483.0`.
#[test]
fn coin_rows_sorts_priced_coins_ahead_of_unpriced() {
    let _locale = crate::test_locale::force("en");
    let assets = AssetsSnapshot {
        rows: vec![
            asset_row("SMALL", 1.5, 12.5, 0.0, false),
            asset_row("NONE", 1.0, 0.0, 0.0, false),
            asset_row("BIG", 35483.0, 40.0, 1.0, false),
            asset_row("TIEA", 7.0, 7.0, 0.0, false),
            asset_row("TIEB", 7.0, 7.0, 0.0, false),
        ],
        ..AssetsSnapshot::default()
    };
    let rows = coin_rows(
        &assets,
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 100.0, 200.0),
    );
    let coins: Vec<&str> = rows.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(coins, vec!["BIG", "SMALL", "TIEA", "TIEB", "NONE"]);
    assert_eq!(rows[0].value, Some(40.0));
    assert_eq!(rows[0].value_text, "40.00$");
    assert_eq!(rows[0].qty_text, "35483.0");
    assert_eq!(rows[1].value, Some(12.5));
    assert_eq!(rows[1].value_text, "12.50$");
    assert_eq!(rows[1].qty_text, "1.5");
    assert_eq!(rows[4].value, None);
    assert_eq!(rows[4].value_text, "unpriced");
}

fn transfer_row(currency: &str, amount: f64, total: f64, value_usdt: f64) -> TransferAssetRow {
    TransferAssetRow {
        currency: currency.to_string(),
        amount,
        total,
        value_usdt,
    }
}

/// `reads::coin_rows` collapses duplicate coin-margined wallets of one coin into a single row.
///
/// Mutation: emit every market row, or add the duplicate values together. A COIN-M core lists
/// the same wallet once per contract, so the coin list would repeat the coin or triple its
/// value. Oracle: rows `BTC` and `btc`, each quantity 1 and value 10, become one `BTC` row
/// whose quantity text is `1.0` and whose value is 10, not `2.0` and 20.
#[test]
fn coin_rows_collapses_duplicate_coin_wallets() {
    let _locale = crate::test_locale::force("en");
    let assets = AssetsSnapshot {
        rows: vec![
            asset_row("BTC", 1.0, 10.0, 1.0, false),
            asset_row("btc", 1.0, 10.0, 1.0, false),
        ],
        ..AssetsSnapshot::default()
    };
    let rows = coin_rows(
        &assets,
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].coin, "BTC");
    assert_eq!(rows[0].qty_text, "1.0");
    assert_eq!(rows[0].value, Some(10.0));
    assert_eq!(rows[0].value_text, "10.00$");
}

/// `reads::coin_rows` shows the larger of the full and free quantities.
///
/// Mutation: format `qty_full` only. A wallet whose full field is missing would show no
/// quantity while 5 coins are free. Oracle: `qty_full` 0 and `qty` 5 render `5.0`. That text
/// is what [`moon_core::util::fmt::qty`] produces for 5, because it keeps one fractional digit.
#[test]
fn coin_rows_uses_free_quantity_when_full_is_missing() {
    let _locale = crate::test_locale::force("en");
    let mut row = asset_row("SOL", 0.0, 8.0, 1.0, false);
    row.qty = 5.0;
    row.qty_full = 0.0;
    let assets = AssetsSnapshot {
        rows: vec![row],
        ..AssetsSnapshot::default()
    };
    let rows = coin_rows(
        &assets,
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].qty_text, "5.0");
    assert_eq!(rows[0].value, Some(8.0));
    assert_eq!(rows[0].value_text, "8.00$");
}

/// `reads::coin_rows` adds a spot holding that exists only on the transfer wallet.
///
/// Mutation: ignore `transfer_assets.spot`, add the second wallet of the same coin, keep the
/// account-quote wallet, or add spot wallets on a futures core. A spot coin that the exchange
/// reports only as a transfer balance would be missing, a duplicated wallet would double, the
/// quote balance would show as a purchased coin, and a futures margin wallet would show too.
/// Oracle: DOGE is only on the spot wallet (total 4, free 1, value 6) and renders `4.0` and
/// `6.00$`; the second DOGE wallet is ignored; USDT matching the quote is absent; ADA already
/// on a market row stays one row; the same DOGE is absent when `futures_account` is set. A
/// priced dust market row stays hidden when that coin is also on a spot transfer wallet.
#[test]
fn coin_rows_adds_a_spot_transfer_holding_once() {
    let _locale = crate::test_locale::force("en");
    let assets = AssetsSnapshot {
        rows: vec![asset_row("ADA", 1.0, 4.0, 1.0, false)],
        base_currency: "USDT".to_string(),
        ..AssetsSnapshot::default()
    };
    let wallets = TransferAssetsSnapshot {
        spot: vec![
            transfer_row("DOGE", 1.0, 4.0, 6.0),
            transfer_row("DOGE", 9.0, 9.0, 20.0),
            transfer_row("USDT", 50.0, 50.0, 50.0),
            transfer_row("ADA", 3.0, 3.0, 12.0),
        ],
        ..TransferAssetsSnapshot::default()
    };
    let rows = coin_rows(
        &assets,
        &wallets,
        "USDC",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    let coins: Vec<&str> = rows.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(coins, vec!["DOGE", "ADA"]);
    assert_eq!(rows[0].qty_text, "4.0");
    assert_eq!(rows[0].value, Some(6.0));
    assert_eq!(rows[0].value_text, "6.00$");
    assert_eq!(rows[1].value, Some(4.0));

    let mut futures = assets.clone();
    futures.futures_account = true;
    let futures_rows = coin_rows(
        &futures,
        &wallets,
        "USDC",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    let futures_coins: Vec<&str> = futures_rows.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(futures_coins, vec!["ADA"]);

    let dust_market = AssetsSnapshot {
        rows: vec![asset_row("DOGE", 0.1, 0.2, 1.0, false)],
        base_currency: "USDT".to_string(),
        ..AssetsSnapshot::default()
    };
    let rescued = coin_rows(
        &dust_market,
        &wallets,
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    let rescued_coins: Vec<&str> = rescued.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(rescued_coins, vec!["ADA"]);
    assert_eq!(rescued[0].qty_text, "3.0");
    assert_eq!(rescued[0].value, Some(12.0));
}

/// `reads::coin_rows` keeps an unpriced coin whose lot is positive.
///
/// Mutation: drop every row with `value_usdt < min_lot_usd`, including a zero. A coin with no
/// rate would disappear instead of showing the unpriced word. Oracle: quantity 3, value 0,
/// lot 5 stays one row with no number and the text `unpriced`.
#[test]
fn coin_rows_keeps_an_unpriced_coin_when_the_lot_is_positive() {
    let _locale = crate::test_locale::force("en");
    let assets = AssetsSnapshot {
        rows: vec![asset_row("ZERO", 3.0, 0.0, 5.0, false)],
        ..AssetsSnapshot::default()
    };
    let rows = coin_rows(
        &assets,
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].coin, "ZERO");
    assert_eq!(rows[0].value, None);
    assert_eq!(rows[0].value_text, "unpriced");
    assert_eq!(rows[0].qty_text, "3.0");
}

/// `reads::coin_rows` keeps a priced dust market coin hidden when that coin is also on spot.
///
/// Mutation: record the seen set after dust is dropped. The transfer loop would add the
/// dust coin back and show a holding the market list already rejected. Oracle: DOGE at
/// value 0.2 under lot 1, plus a DOGE spot wallet of value 6, produces no DOGE row. ADA,
/// present only on the spot wallet (quantity 3, value 12), is the only row.
#[test]
fn coin_rows_hides_priced_dust_also_held_on_spot() {
    let _locale = crate::test_locale::force("en");
    let assets = AssetsSnapshot {
        rows: vec![asset_row("DOGE", 0.1, 0.2, 1.0, false)],
        base_currency: "USDT".to_string(),
        ..AssetsSnapshot::default()
    };
    let wallets = TransferAssetsSnapshot {
        spot: vec![
            transfer_row("DOGE", 1.0, 4.0, 6.0),
            transfer_row("ADA", 3.0, 3.0, 12.0),
            transfer_row("USDT", 50.0, 50.0, 50.0),
        ],
        ..TransferAssetsSnapshot::default()
    };
    let rows = coin_rows(&assets, &wallets, "", reading(BalanceState::Live, 1.0, 1.0));
    let coins: Vec<&str> = rows.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(coins, vec!["ADA"]);
    assert_eq!(rows[0].qty_text, "3.0");
    assert_eq!(rows[0].value, Some(12.0));
    assert_eq!(rows[0].value_text, "12.00$");
}

/// `reads::coin_rows` treats a non-finite USDT value as unpriced.
///
/// Mutation: accept infinity because `value > 0` is true, or let infinity beat a finite
/// value of the same quantity. The row would show an impossible figure, and input order
/// would decide which wallet wins. Oracle: one infinity row stays, with no number and the
/// text `unpriced`. Two `MIX` rows of quantity 2, values infinity and 15, become value 15
/// and `15.00$` in both orders. A NaN value stays unpriced the same way.
#[test]
fn coin_rows_treats_a_non_finite_value_as_unpriced() {
    let _locale = crate::test_locale::force("en");
    let infinite = AssetsSnapshot {
        rows: vec![asset_row("INF", 1.0, f64::INFINITY, 1.0, false)],
        ..AssetsSnapshot::default()
    };
    let rows = coin_rows(
        &infinite,
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].coin, "INF");
    assert_eq!(rows[0].value, None);
    assert_eq!(rows[0].value_text, "unpriced");

    let nan_value = AssetsSnapshot {
        rows: vec![asset_row("NAN", 1.0, f64::NAN, 1.0, false)],
        ..AssetsSnapshot::default()
    };
    let nan_rows = coin_rows(
        &nan_value,
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    assert_eq!(nan_rows.len(), 1);
    assert_eq!(nan_rows[0].value, None);
    assert_eq!(nan_rows[0].value_text, "unpriced");

    for (first, second) in [(f64::INFINITY, 15.0), (15.0, f64::INFINITY)] {
        let mixed = AssetsSnapshot {
            rows: vec![
                asset_row("MIX", 2.0, first, 1.0, false),
                asset_row("MIX", 2.0, second, 1.0, false),
            ],
            ..AssetsSnapshot::default()
        };
        let rows = coin_rows(
            &mixed,
            &TransferAssetsSnapshot::default(),
            "",
            reading(BalanceState::Live, 1.0, 1.0),
        );
        assert_eq!(rows.len(), 1, "order {first} then {second}");
        assert_eq!(rows[0].coin, "MIX");
        assert_eq!(rows[0].value, Some(15.0));
        assert_eq!(rows[0].value_text, "15.00$");
    }
}

/// `reads::coin_rows` turns a non-finite quantity into zero before it compares.
///
/// Mutation: pass NaN into `max` or `>`. Which row wins then depends on input order, and
/// a NaN full quantity would hide a real free quantity. Oracle: one row with `qty_full`
/// NaN and `qty` 5 renders `5.0` and value `8.00$`. Two SOL rows, one with both
/// quantities NaN and value 100 and one with quantity 2 and value 5, render `2.0` and
/// `5.00$` in both orders. A spot wallet whose total is NaN and whose amount is 3 renders
/// `3.0`.
#[test]
fn coin_rows_treats_a_non_finite_quantity_as_zero() {
    let _locale = crate::test_locale::force("en");
    let mut partial = asset_row("SOL", 0.0, 8.0, 1.0, false);
    partial.qty = 5.0;
    partial.qty_full = f64::NAN;
    let one = coin_rows(
        &AssetsSnapshot {
            rows: vec![partial],
            ..AssetsSnapshot::default()
        },
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].qty_text, "5.0");
    assert_eq!(one[0].value, Some(8.0));
    assert_eq!(one[0].value_text, "8.00$");

    let mut blank = asset_row("SOL", 0.0, 100.0, 1.0, false);
    blank.qty = f64::NAN;
    blank.qty_full = f64::NAN;
    let finite = asset_row("SOL", 2.0, 5.0, 1.0, false);
    for (label, rows_in) in [
        ("nan then finite", vec![blank.clone(), finite.clone()]),
        ("finite then nan", vec![finite, blank]),
    ] {
        let rows = coin_rows(
            &AssetsSnapshot {
                rows: rows_in,
                ..AssetsSnapshot::default()
            },
            &TransferAssetsSnapshot::default(),
            "",
            reading(BalanceState::Live, 1.0, 1.0),
        );
        assert_eq!(rows.len(), 1, "{label}");
        assert_eq!(rows[0].coin, "SOL");
        assert_eq!(rows[0].qty_text, "2.0", "{label}");
        assert_eq!(rows[0].value, Some(5.0), "{label}");
        assert_eq!(rows[0].value_text, "5.00$", "{label}");
    }

    let wallets = TransferAssetsSnapshot {
        spot: vec![transfer_row("XRP", 3.0, f64::NAN, 9.0)],
        ..TransferAssetsSnapshot::default()
    };
    let transferred = coin_rows(
        &AssetsSnapshot::default(),
        &wallets,
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    assert_eq!(transferred.len(), 1);
    assert_eq!(transferred[0].coin, "XRP");
    assert_eq!(transferred[0].qty_text, "3.0");
    assert_eq!(transferred[0].value, Some(9.0));
    assert_eq!(transferred[0].value_text, "9.00$");
}

/// `reads::coin_rows` skips a market row that holds no coins.
///
/// Mutation: keep a futures market whose quantity is zero because it has a position, or
/// record that coin as seen so its spot wallet is dropped. A position is not a held coin,
/// and a transfer of the same coin is. Oracle: POS with qty 0, qty_full 0, value 0 and
/// pos_size 3 is absent beside KEEP. The same POS on a spot wallet of quantity 2 and
/// value 7 appears, and KEEP stays.
#[test]
fn coin_rows_skips_a_position_only_market_row() {
    let _locale = crate::test_locale::force("en");
    let mut position = asset_row("POS", 0.0, 0.0, 1.0, false);
    position.pos_size = 3.0;
    let assets = AssetsSnapshot {
        rows: vec![position, asset_row("KEEP", 2.0, 4.0, 1.0, false)],
        ..AssetsSnapshot::default()
    };
    let held = coin_rows(
        &assets,
        &TransferAssetsSnapshot::default(),
        "",
        reading(BalanceState::Live, 1.0, 1.0),
    );
    let held_coins: Vec<&str> = held.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(held_coins, vec!["KEEP"]);

    let wallets = TransferAssetsSnapshot {
        spot: vec![transfer_row("POS", 2.0, 2.0, 7.0)],
        ..TransferAssetsSnapshot::default()
    };
    let rows = coin_rows(&assets, &wallets, "", reading(BalanceState::Live, 1.0, 1.0));
    let coins: Vec<&str> = rows.iter().map(|row| row.coin.as_str()).collect();
    assert_eq!(coins, vec!["POS", "KEEP"]);
    assert_eq!(rows[0].qty_text, "2.0");
    assert_eq!(rows[0].value, Some(7.0));
    assert_eq!(rows[0].value_text, "7.00$");
    assert_eq!(rows[1].coin, "KEEP");
    assert_eq!(rows[1].value, Some(4.0));
}

/// Isolated temp root for a notifications file. Removed on drop, including on panic.
struct NotifyTemp(PathBuf);

impl NotifyTemp {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "moon-tg-mini-notify-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for NotifyTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn empty_store(path: PathBuf) -> NotifyStore {
    NotifyStore {
        path,
        file: NotifyFile::default(),
        allowed: None,
    }
}

/// Initial rules must record an enable boundary once and preserve even an empty stored row.
#[test]
fn create_chat_settings_preserves_old_rows_and_enable_time() {
    let root = NotifyTemp::new("initial-rules");
    let path = root.path("notifications.json");
    let mut store = empty_store(path.clone());
    let legacy = root.path("legacy.json");
    std::fs::write(&legacy, r#"{"chats":{"7":{"revision":3}}}"#).unwrap();
    store.file = NotifyFile::load(&legacy).unwrap();
    super::settings::create_chat_settings(&mut store, 8, 1_000, chrono_tz::UTC).unwrap();
    assert_eq!(store.file.chats[&8].settings, NotifySettings::new_chat());
    assert_eq!(store.file.chats[&8].ledger.trades_enabled_utc, Some(1_000));
    let bytes = std::fs::read(&path).unwrap();
    super::settings::create_chat_settings(&mut store, 8, 2_000, chrono_tz::UTC).unwrap();
    super::settings::create_chat_settings(&mut store, 7, 2_000, chrono_tz::UTC).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(store.file.chats[&7].settings, NotifySettings::default());
    assert_eq!(store.file.chats[&7].revision, 3);
    assert_eq!(NotifyFile::load(&path).unwrap(), store.file);

    store.path = root.path("directory");
    std::fs::create_dir(&store.path).unwrap();
    let before = store.file.clone();
    assert!(super::settings::create_chat_settings(&mut store, 9, 3_000, chrono_tz::UTC).is_err());
    assert_eq!(
        store.file, before,
        "a failed initial save must not publish enabled rules"
    );
}

/// First settings drafts share pairing defaults, but viewing a sparse stored row must not opt in.
#[test]
fn initial_settings_draft_saves_without_changing_sparse_chats() {
    let root = NotifyTemp::new("initial-draft");
    let mut store = empty_store(root.path("notifications.json"));
    let legacy = root.path("legacy.json");
    std::fs::write(&legacy, r#"{"chats":{"7":{"revision":3}}}"#).unwrap();
    store.file = NotifyFile::load(&legacy).unwrap();
    let old = super::settings::settings_row(&store.file, 7);
    assert_eq!(old.settings, NotifySettings::default());
    assert_eq!(old.revision, 3);
    let draft = super::settings::settings_row(&store.file, 8);
    assert!(draft.settings.trades.on);
    assert!(draft.settings.down.on);
    assert_eq!(draft.settings.trades.profit_at_least_usd, Some(100.0));
    assert_eq!(draft.settings.trades.loss_at_least_usd, Some(100.0));
    assert_eq!(draft.revision, 0);
    assert!(
        !store.path.exists(),
        "opening a draft must not create settings"
    );
    assert_eq!(
        store_settings(
            &mut store,
            8,
            draft.settings,
            &[],
            1_000,
            chrono_tz::UTC,
            draft.revision
        ),
        SaveResult::Saved
    );
    assert_eq!(store.file.chats[&8].ledger.trades_enabled_utc, Some(1_000));
    assert_eq!(store.file.chats[&7].settings, NotifySettings::default());
}

/// Save with the revision currently stored for `chat`, in UTC.
///
/// An absent chat is revision 0. Tests that must send a different revision call
/// [`store_settings`] themselves.
fn save_known(
    store: &mut NotifyStore,
    chat: i64,
    settings: NotifySettings,
    visible: &[u64],
    now_utc: i64,
) -> SaveResult {
    let revision = store
        .file
        .chats
        .get(&chat)
        .map(|row| row.revision)
        .unwrap_or(0);
    store_settings(
        store,
        chat,
        settings,
        visible,
        now_utc,
        chrono_tz::UTC,
        revision,
    )
}

/// `settings::prepare_settings` keeps visible `Only` ids in the submitted order, duplicates included.
///
/// Mutation: sort the ids, drop duplicates, or intersect `All` with the visible list. A viewer
/// then loses a core they named twice, or an "all cores" rule becomes empty when the grant is
/// empty and the save is refused. Oracle: `[9, 1, 9, 2]` against visible `[1, 9]` is `[9, 1, 9]`,
/// and `All` against an empty grant stays `All`.
#[test]
fn prepare_settings_keeps_visible_only_ids_and_leaves_all() {
    let mut only = NotifySettings::default();
    only.trades.cores = CoreScope::Only(vec![9, 1, 9, 2]);
    let prepared = prepare_settings(only, &[1, 9]).expect("visible ids stay");
    assert_eq!(prepared.trades.cores, CoreScope::Only(vec![9, 1, 9]));

    let mut all = NotifySettings::default();
    all.trades.on = true;
    let prepared = prepare_settings(all, &[]).expect("All is not intersected");
    assert_eq!(prepared.trades.cores, CoreScope::All);
    assert!(prepared.trades.on);

    let mut empty = NotifySettings::default();
    empty.trades.cores = CoreScope::Only(vec![]);
    assert_eq!(
        prepare_settings(empty, &[1]),
        Err(SaveFault::Cores),
        "an empty Only list is a core fault even when the chat can see a core"
    );

    let mut invalid = NotifySettings::default();
    invalid.down.after_minutes = 0;
    assert_eq!(prepare_settings(invalid, &[]), Err(SaveFault::Invalid));
}

/// A viewer `Only` list stores the cores that chat can see, duplicates included, and leaves `All`.
///
/// Mutation: store the forbidden id, or drop the second copy of the allowed id. The file then
/// names a core the chat cannot see, or a repeated id disappears. Oracle: `[99, 1, 1, 2]` against
/// visible `[1]` is `[1, 1]` on disk. `All` with an empty visible list is stored unchanged.
#[test]
fn store_settings_keeps_visible_cores_and_does_not_filter_all() {
    let root = NotifyTemp::new("visible");
    let path = root.path("notifications.json");
    let mut store = empty_store(path.clone());

    let mut only = NotifySettings::default();
    only.trades.on = true;
    only.trades.cores = CoreScope::Only(vec![99, 1, 1, 2]);
    assert_eq!(
        save_known(&mut store, 4, only, &[1], 1_700_000_000),
        SaveResult::Saved
    );
    let row = &store.file.chats[&4];
    assert_eq!(row.settings.trades.cores, CoreScope::Only(vec![1, 1]));
    assert!(row.settings.trades.on);
    assert_eq!(row.ledger.trades_enabled_utc, Some(1_700_000_000));
    assert!(row.ledger.seen.is_empty());
    assert_eq!(row.revision, 1);
    assert_eq!(NotifyFile::load(&path).expect("reload"), store.file);

    let mut all = NotifySettings::default();
    all.trades.on = true;
    assert_eq!(
        save_known(&mut store, 5, all, &[], 1_700_000_100),
        SaveResult::Saved
    );
    let row = &store.file.chats[&5];
    assert_eq!(row.settings.trades.cores, CoreScope::All);
    assert!(row.settings.trades.on);
    assert_eq!(row.ledger.trades_enabled_utc, Some(1_700_000_100));
    assert_eq!(NotifyFile::load(&path).expect("reload all"), store.file);
}

/// `Only` of cores the chat cannot see is refused, and the file is not opened for write.
///
/// Mutation: call `update` before the intersection check. The destination here is a directory, so
/// that call would return [`SaveResult::Failed`] and could rewrite nothing only by accident.
/// Oracle: [`SaveResult::Refused`] with [`SaveFault::Cores`] or [`SaveFault::Invalid`], the seed
/// bytes unchanged, and chat 8 still absent.
#[test]
fn store_settings_refuses_before_it_writes() {
    let root = NotifyTemp::new("refuse");
    let path = root.path("notifications.json");
    let mut store = empty_store(path.clone());
    store
        .update(|file| {
            file.chats.insert(
                7,
                ChatNotify {
                    revision: 1,
                    ..ChatNotify::default()
                },
            );
        })
        .expect("seed");
    let seeded = store.file.clone();
    let bytes = std::fs::read(&path).expect("seed bytes");
    let dir = root.path("not-a-file");
    std::fs::create_dir_all(&dir).expect("directory");
    store.path = dir;

    let mut forbidden = NotifySettings::default();
    forbidden.trades.cores = CoreScope::Only(vec![99]);
    assert_eq!(
        save_known(&mut store, 8, forbidden, &[1], 50),
        SaveResult::Refused(SaveFault::Cores)
    );
    let mut empty = NotifySettings::default();
    empty.trades.cores = CoreScope::Only(vec![]);
    assert_eq!(
        save_known(&mut store, 8, empty, &[1], 50),
        SaveResult::Refused(SaveFault::Cores)
    );
    let mut invalid = NotifySettings::default();
    invalid.down.after_minutes = 0;
    assert_eq!(
        save_known(&mut store, 8, invalid, &[1], 50),
        SaveResult::Refused(SaveFault::Invalid)
    );

    assert_eq!(store.file, seeded);
    assert!(!store.file.chats.contains_key(&8));
    assert_eq!(std::fs::read(&path).expect("bytes after refuse"), bytes);
}

/// Enabling trades records the timestamp and clears `seen`. Saving again while they stay on
/// keeps both. Disabling clears the timestamp and leaves `seen`.
///
/// Mutation: refresh the timestamp on every save, or clear `seen` when trades turn off. A restart
/// then treats an unchanged rule as newly enabled, or forgets closes it already announced.
/// Oracle: the seeded timestamp 10 and seen row `(1, 2, 30)` survive a re-save at 99; off clears
/// only the timestamp; on at 200 replaces the timestamp and empties `seen`.
#[test]
fn store_settings_trades_ledger_follows_the_on_edge() {
    let root = NotifyTemp::new("trades");
    let path = root.path("notifications.json");
    let mut store = empty_store(path.clone());
    let mut on = NotifySettings::default();
    on.trades.on = true;
    let mut seen = BTreeMap::new();
    seen.insert(1_u64, BTreeMap::from([(2_i64, 30_i64)]));
    store
        .update(|file| {
            file.chats.insert(
                5,
                ChatNotify {
                    settings: on,
                    ledger: NotifyLedger {
                        trades_enabled_utc: Some(10),
                        seen,
                        down_announced: BTreeSet::from([7]),
                        ..NotifyLedger::default()
                    },
                    revision: 2,
                },
            );
        })
        .expect("seed");

    let mut still = NotifySettings::default();
    still.trades.on = true;
    assert_eq!(save_known(&mut store, 5, still, &[], 99), SaveResult::Saved);
    let row = &store.file.chats[&5];
    assert_eq!(row.ledger.trades_enabled_utc, Some(10));
    assert_eq!(
        row.ledger.seen.get(&1).and_then(|rows| rows.get(&2)),
        Some(&30)
    );
    assert!(row.ledger.down_announced.contains(&7));
    assert_eq!(row.revision, 3);

    let off = NotifySettings::default();
    assert_eq!(save_known(&mut store, 5, off, &[], 100), SaveResult::Saved);
    let row = &store.file.chats[&5];
    assert_eq!(row.ledger.trades_enabled_utc, None);
    assert_eq!(
        row.ledger.seen.get(&1).and_then(|rows| rows.get(&2)),
        Some(&30)
    );
    assert!(row.ledger.down_announced.contains(&7));
    assert!(!row.settings.trades.on);
    assert_eq!(row.revision, 4);

    let mut again = NotifySettings::default();
    again.trades.on = true;
    assert_eq!(
        save_known(&mut store, 5, again, &[], 200),
        SaveResult::Saved
    );
    let row = &store.file.chats[&5];
    assert_eq!(row.ledger.trades_enabled_utc, Some(200));
    assert!(row.ledger.seen.is_empty());
    assert!(row.ledger.down_announced.contains(&7));
    assert_eq!(row.revision, 5);
    assert_eq!(NotifyFile::load(&path).expect("reload"), store.file);
}

/// The deal chart rule starts at its own switch-on and forgets its ledger at its switch-off,
/// whatever the card rule does.
///
/// Mutation: leave `enabled_utc` unset, reuse the card rule's moment, or keep `seen` across a
/// switch-off. Charts then never come, come for trades closed before the switch, or a trade seen
/// before is skipped after the rule comes back.
#[test]
fn store_settings_chart_ledger_follows_its_own_on_edge() {
    let root = NotifyTemp::new("charts");
    let mut store = empty_store(root.path("notifications.json"));
    let mut on = NotifySettings::default();
    on.charts.on = true;
    assert_eq!(
        save_known(&mut store, 6, on.clone(), &[1], 100),
        SaveResult::Saved
    );
    let ledger = &store.file.chats[&6].ledger;
    assert_eq!(ledger.charts.enabled_utc, Some(100));
    assert_eq!(ledger.trades_enabled_utc, None, "the card rule stays off");
    store
        .file
        .chats
        .get_mut(&6)
        .unwrap()
        .ledger
        .charts
        .seen
        .insert(1, BTreeMap::from([(5, 90)]));
    assert_eq!(
        save_known(&mut store, 6, on.clone(), &[1], 200),
        SaveResult::Saved
    );
    assert_eq!(store.file.chats[&6].ledger.charts.enabled_utc, Some(100));
    let mut off = on.clone();
    off.charts.on = false;
    assert_eq!(save_known(&mut store, 6, off, &[1], 300), SaveResult::Saved);
    assert!(store.file.chats[&6].ledger.charts.is_empty());
    assert_eq!(save_known(&mut store, 6, on, &[1], 400), SaveResult::Saved);
    assert_eq!(store.file.chats[&6].ledger.charts.enabled_utc, Some(400));
}

/// `down_announced` is cleared only when down goes from on to off.
///
/// Mutation: clear the set on every save, or when down turns on. A core that is still down is
/// announced again, or a core marked down while the rule was off is forgotten when the rule
/// turns on. Oracle: the set `{4, 9}` survives a save that leaves down on, becomes empty when
/// down turns off, and a mark added while down is off survives the turn back on.
#[test]
fn store_settings_clears_down_announced_only_when_down_turns_off() {
    let root = NotifyTemp::new("down");
    let path = root.path("notifications.json");
    let mut store = empty_store(path);
    let mut down_on = NotifySettings::default();
    down_on.down.on = true;
    store
        .update(|file| {
            file.chats.insert(
                6,
                ChatNotify {
                    settings: down_on.clone(),
                    ledger: NotifyLedger {
                        down_announced: BTreeSet::from([4, 9]),
                        ..NotifyLedger::default()
                    },
                    revision: 3,
                },
            );
        })
        .expect("seed");

    assert_eq!(
        save_known(&mut store, 6, down_on, &[], 1),
        SaveResult::Saved
    );
    assert_eq!(
        store.file.chats[&6].ledger.down_announced,
        BTreeSet::from([4, 9])
    );
    assert_eq!(store.file.chats[&6].revision, 4);

    let off = NotifySettings::default();
    assert_eq!(save_known(&mut store, 6, off, &[], 2), SaveResult::Saved);
    assert!(store.file.chats[&6].ledger.down_announced.is_empty());
    assert!(!store.file.chats[&6].settings.down.on);
    assert_eq!(store.file.chats[&6].revision, 5);

    store
        .file
        .chats
        .get_mut(&6)
        .expect("chat")
        .ledger
        .down_announced
        .insert(4);
    let mut back = NotifySettings::default();
    back.down.on = true;
    assert_eq!(save_known(&mut store, 6, back, &[], 3), SaveResult::Saved);
    assert_eq!(
        store.file.chats[&6].ledger.down_announced,
        BTreeSet::from([4])
    );
    assert!(store.file.chats[&6].settings.down.on);
}

/// A failed atomic save keeps the previous settings in memory and on disk.
///
/// Mutation: assign the edited file before `save` returns. A rename onto a directory then
/// publishes settings the disk does not hold. Oracle: the seeded trades-on document, loaded
/// back from the original path, equals memory, and chat 8 was not inserted.
#[test]
fn store_settings_failed_save_keeps_the_previous_document() {
    let root = NotifyTemp::new("fail");
    let path = root.path("notifications.json");
    let mut store = empty_store(path.clone());
    let mut seeded_settings = NotifySettings::default();
    seeded_settings.trades.on = true;
    assert_eq!(
        save_known(&mut store, 7, seeded_settings, &[], 15),
        SaveResult::Saved
    );
    let seeded = store.file.clone();
    let dir = root.path("not-a-file");
    std::fs::create_dir_all(&dir).expect("directory");
    store.path = dir;

    let mut next = NotifySettings::default();
    next.trades.on = false;
    next.down.on = true;
    let failed = save_known(&mut store, 7, next, &[], 90);
    assert!(
        matches!(failed, SaveResult::Failed(ref text) if !text.is_empty()),
        "renaming onto a directory must fail the save, got {failed:?}"
    );
    assert_eq!(store.file, seeded);
    assert_eq!(store.file.chats[&7].ledger.trades_enabled_utc, Some(15));
    assert!(store.file.chats[&7].settings.trades.on);
    assert_eq!(NotifyFile::load(&path).expect("seed file"), seeded);
    assert!(!seeded.chats.contains_key(&8));
}

/// A revision the page did not load is refused before validation and before the file is written.
/// The matching revision is stored and comes back as one greater.
///
/// Mutation: compare the revision after `prepare_settings`, or skip the compare. A stale invalid
/// draft then says "check the values" and keeps the old form, or the second window's rules
/// replace the first. Oracle: chat 9 stays revision 2 with trades off while the path is a
/// directory; a following save at revision 2 turns trades on and stores revision 3. An absent
/// chat accepts 0 and refuses any other revision.
#[test]
fn store_settings_refuses_a_stale_revision_and_stores_a_match() {
    let root = NotifyTemp::new("revision");
    let path = root.path("notifications.json");
    let mut store = empty_store(path.clone());
    let mut seeded_settings = NotifySettings::default();
    seeded_settings.trades.on = false;
    store
        .update(|file| {
            file.chats.insert(
                9,
                ChatNotify {
                    settings: seeded_settings,
                    revision: 2,
                    ..ChatNotify::default()
                },
            );
        })
        .expect("seed");
    let seeded = store.file.clone();
    let bytes = std::fs::read(&path).expect("seed bytes");
    let dir = root.path("not-a-file");
    std::fs::create_dir_all(&dir).expect("directory");
    store.path = dir;

    let mut stale_invalid = NotifySettings::default();
    stale_invalid.down.after_minutes = 0;
    stale_invalid.trades.on = true;
    assert_eq!(
        store_settings(&mut store, 9, stale_invalid, &[1], 50, chrono_tz::UTC, 1,),
        SaveResult::Refused(SaveFault::Stale)
    );
    assert_eq!(
        store_settings(
            &mut store,
            11,
            NotifySettings::default(),
            &[],
            50,
            chrono_tz::UTC,
            4,
        ),
        SaveResult::Refused(SaveFault::Stale)
    );
    assert_eq!(store.file, seeded);
    assert!(!store.file.chats.contains_key(&11));
    assert_eq!(std::fs::read(&path).expect("bytes after stale"), bytes);

    store.path = path;
    let mut matched = NotifySettings::default();
    matched.trades.on = true;
    assert_eq!(
        store_settings(&mut store, 9, matched, &[], 80, chrono_tz::UTC, 2),
        SaveResult::Saved
    );
    assert!(store.file.chats[&9].settings.trades.on);
    assert_eq!(store.file.chats[&9].revision, 3);
    assert_eq!(store.file.chats[&9].ledger.trades_enabled_utc, Some(80));

    assert_eq!(
        store_settings(
            &mut store,
            10,
            NotifySettings::default(),
            &[],
            81,
            chrono_tz::UTC,
            0,
        ),
        SaveResult::Saved
    );
    assert_eq!(store.file.chats[&10].revision, 1);
    assert!(!store.file.chats.contains_key(&11));
}

/// Refusal text is the locale sentence, not the key. The guards are not nested: the locale lock
/// is not reentrant.
///
/// Mutation: return the key, or the English sentence in every locale. The page then shows
/// `telegram.mini_settings_err_cores` or English to a Russian chat. Oracle: the strings in
/// `locales/<lang>/telegram.<lang>.yml`, including the stale-revision sentence.
#[test]
fn save_fault_text_follows_the_chat_locale() {
    {
        let _locale = crate::test_locale::force("en");
        assert_eq!(
            save_fault_text(SaveFault::Cores),
            "Pick at least one core you can see"
        );
        assert_eq!(save_fault_text(SaveFault::Invalid), "Check the values");
        assert_eq!(
            save_fault_text(SaveFault::Stale),
            "Settings changed elsewhere — showing the latest"
        );
        assert_eq!(
            rust_i18n::t!("telegram.mini_settings_err_save").to_string(),
            "Could not save"
        );
    }
    {
        let _locale = crate::test_locale::force("ru");
        assert_eq!(
            save_fault_text(SaveFault::Cores),
            "Выберите хотя бы одно доступное ядро"
        );
        assert_eq!(save_fault_text(SaveFault::Invalid), "Проверьте значения");
        assert_eq!(
            save_fault_text(SaveFault::Stale),
            "Настройки изменены в другом окне — показаны актуальные"
        );
        assert_eq!(
            rust_i18n::t!("telegram.mini_settings_err_save").to_string(),
            "Не удалось сохранить"
        );
    }
    {
        let _locale = crate::test_locale::force("es");
        assert_eq!(
            save_fault_text(SaveFault::Cores),
            "Elige al menos un núcleo que puedas ver"
        );
        assert_eq!(save_fault_text(SaveFault::Invalid), "Revisa los valores");
        assert_eq!(
            save_fault_text(SaveFault::Stale),
            "Los ajustes cambiaron en otro lugar — se muestran los más recientes"
        );
        assert_eq!(
            rust_i18n::t!("telegram.mini_settings_err_save").to_string(),
            "No se pudo guardar"
        );
    }
}

/// An order's PnL text names its unit: dollars only for a USD-stable quote, nothing when unknown.
///
/// Mutation: always append "$". A BTC-quoted order's 0.00012345 BTC then reads as "+0.00$".
#[test]
fn pnl_text_carries_the_quote_unit() {
    assert_eq!(pnl_text(1.234, "USDT").as_deref(), Some("+1.23$"));
    assert_eq!(pnl_text(-2.5, "").as_deref(), Some("-2.50"));
    assert_eq!(
        pnl_text(0.00012345, "BTC").as_deref(),
        Some("+0.00012345 BTC")
    );
}

/// The unit and the conversion share one quote: the catalog's, else the name's, else USDC.
///
/// Mutation: leave an empty catalog quote unknown. Every Hyperliquid perp (`BTC`, `xyz:BIRD`, no
/// quote in the catalog) then prints a bare number and drops out of the dollar sums.
#[test]
fn order_quote_is_the_catalog_then_the_name_then_usdc() {
    assert_eq!(order_quote("BTC", "ETHBTC"), "BTC");
    assert_eq!(order_quote("", "BTCUSDT"), "USDT");
    assert_eq!(order_quote("", "BTC"), "USDC");
    assert_eq!(order_quote("", "xyz:BIRD"), "USDC");
}

/// Dollars: a stablecoin as is, a coin through its rate, nothing without a quote or a rate.
///
/// Mutation: return the quote-currency value when the rate is missing. A BTC PnL then joins the
/// dollar sum as if one BTC were one dollar.
#[test]
fn pnl_usd_converts_through_the_same_quote() {
    assert_eq!(pnl_usd(2.0, "USDT", |_| None), Some(2.0));
    assert_eq!(
        pnl_usd(0.001, "BTC", |q| (q == "BTC").then_some(60_000.0)),
        Some(60.0)
    );
    assert_eq!(pnl_usd(0.001, "BTC", |_| None), None);
    assert_eq!(pnl_usd(1.0, "", |_| Some(1.0)), None);
    assert_eq!(pnl_usd(1.0, "BTC", |_| Some(f64::INFINITY)), None);
}

/// The tone follows the printed text: a loss that rounds to zero is neutral.
#[test]
fn pnl_sign_follows_the_rounded_text() {
    assert_eq!(pnl_sign(-0.001, "USDT"), 0);
    assert_eq!(pnl_sign(-0.01, "USDT"), -1);
    assert_eq!(pnl_sign(0.00000002, "BTC"), 1);
}

/// A row from the settings window is refused on a stale revision or an unpaired chat, and taken
/// on the stored one — checked without writing.
#[test]
fn settings_window_rows_are_checked_before_any_write() {
    use moon_core::station_api::ChatNotifyRow;
    let mut telegram = moon_core::config::TelegramConfig::default();
    telegram.pair_chat(7);
    let mut file = moon_core::telegram::notify::NotifyFile::default();
    file.chats.insert(
        7,
        moon_core::telegram::notify::ChatNotify {
            revision: 3,
            ..Default::default()
        },
    );
    let row = |revision| {
        std::collections::BTreeMap::from([(
            7,
            ChatNotifyRow {
                settings: Default::default(),
                revision,
            },
        )])
    };
    assert!(super::settings::check_rows(&file, &telegram, &row(3)).is_ok());
    assert!(super::settings::check_rows(&file, &telegram, &row(2)).is_err());
    let unpaired = std::collections::BTreeMap::from([(9, ChatNotifyRow::default())]);
    assert!(super::settings::check_rows(&file, &telegram, &unpaired).is_err());
}

/// An automatic report that turns on records its current slot as done, so the first report
/// comes at the next slot; a re-save while it stays on, and another report turning on, leave it.
#[test]
fn store_settings_marks_the_current_slot_when_an_auto_report_turns_on() {
    use chrono::TimeZone;
    use moon_core::telegram::notify::AutoReport;
    let at = |hour, minute| {
        chrono_tz::UTC
            .with_ymd_and_hms(2026, 10, 3, hour, minute, 0)
            .single()
            .expect("utc instant")
            .timestamp()
    };
    let root = NotifyTemp::new("auto-slot");
    let mut store = empty_store(root.path("notifications.json"));
    let mut rule = NotifySettings::default();
    rule.reports.set(AutoReport::Hourly, true);
    assert_eq!(
        save_known(&mut store, 1, rule.clone(), &[], at(14, 37)),
        SaveResult::Saved
    );
    let slots = store.file.chats[&1].ledger.reports;
    assert_eq!(slots.hourly.slot_utc, Some(at(14, 0)));
    assert_eq!(slots.today.slot_utc, None);
    rule.reports.set(AutoReport::Month, true);
    assert_eq!(
        save_known(&mut store, 1, rule.clone(), &[], at(16, 5)),
        SaveResult::Saved
    );
    let slots = store.file.chats[&1].ledger.reports;
    assert_eq!(slots.hourly.slot_utc, Some(at(14, 0)), "hourly stayed on");
    assert_eq!(slots.month.slot_utc, Some(at(0, 0)));
    // Off forgets the last message: a later run must not delete a report of this one.
    store
        .file
        .chats
        .get_mut(&1)
        .unwrap()
        .ledger
        .reports
        .month
        .message = Some(77);
    rule.reports.set(AutoReport::Month, false);
    assert_eq!(
        save_known(&mut store, 1, rule.clone(), &[], at(17, 0)),
        SaveResult::Saved
    );
    assert_eq!(store.file.chats[&1].ledger.reports.month.message, None);
}

/// The Mini App's save keeps the chat's automatic reports: the page never sends them.
#[test]
fn a_mini_app_save_keeps_the_stored_auto_reports() {
    use moon_core::telegram::notify::{AutoReport, ChatNotify};
    let mut file = NotifyFile::default();
    let mut stored = ChatNotify::default();
    stored.settings.reports.set(AutoReport::Today, true);
    file.chats.insert(5, stored);
    let mut from_page = NotifySettings::default();
    from_page.down.on = true;
    keep_stored_bot_fields(&file, 5, &mut from_page);
    assert!(from_page.down.on);
    assert!(from_page.reports.on(AutoReport::Today));
    let mut fresh = NotifySettings::default();
    fresh.reports.set(AutoReport::Hourly, true);
    keep_stored_bot_fields(&file, 6, &mut fresh);
    assert!(!fresh.reports.any(), "a chat with no row has none");
}
