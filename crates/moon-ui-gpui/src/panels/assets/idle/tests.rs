// Explicit imports, NOT `use super::*`: the panel's parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute.
use super::{IdleInput, footer_idle_total, idle_marks, trading_wallet};
use moon_core::account::AccountIdentity;
use moon_core::feed::{TransferAssetRow, TransferAssetsSnapshot, WalletKind};
use moon_core::venue::{Brand, MarketKind, Venue, wallets_key};

/// A priced coin worth `value` USDT.
fn coin(currency: &str, value: f64) -> TransferAssetRow {
    TransferAssetRow {
        currency: currency.to_string(),
        amount: value,
        total: value,
        value_usdt: value,
    }
}

/// Wallets holding `spot` and `futures`.
fn wallets(spot: Vec<TransferAssetRow>, futures: Vec<TransferAssetRow>) -> TransferAssetsSnapshot {
    TransferAssetsSnapshot {
        spot,
        futures,
        quarterly: Vec::new(),
    }
}

/// A visible core counted in the footer total, on `brand`/`kind`, on the account stating
/// `address` when given, holding `held` as received at `at_ms`.
fn core<'a>(
    core: u64,
    brand: Brand,
    kind: MarketKind,
    address: Option<&str>,
    held: Option<(&'a TransferAssetsSnapshot, i64)>,
) -> IdleInput<'a> {
    let venue = Venue { brand, kind };
    IdleInput {
        core,
        visible: true,
        counted: true,
        venue: Some(venue),
        key: address.and_then(|address| {
            wallets_key(Some(venue), AccountIdentity::from_auth("", address, 0))
        }),
        wallets: held,
    }
}

/// Breakage: `trading_wallet` following the market on Bybit — the Unified Trading account a Bybit
/// futures core trades from (reported under `Spot`) would be marked as idle money.
#[test]
fn a_bybit_core_of_either_market_trades_from_the_unified_account() {
    let venue = |brand, kind| Venue { brand, kind };
    assert_eq!(
        trading_wallet(venue(Brand::Bybit, MarketKind::Futures)),
        Some(WalletKind::Spot)
    );
    assert_eq!(
        trading_wallet(venue(Brand::Binance, MarketKind::Futures)),
        Some(WalletKind::Futures)
    );
    assert_eq!(
        trading_wallet(venue(Brand::Gate, MarketKind::Spot)),
        Some(WalletKind::Spot)
    );
    assert_eq!(trading_wallet(venue(Brand::Okx, MarketKind::Futures)), None);
}

/// Breakage: the idle rule following the code NAME — a Bybit futures core trades from the Spot
/// code (the unified account), so its balance there is not idle; Funding is.
#[test]
fn bybit_funding_is_idle_and_the_unified_account_is_not() {
    let held = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let marks = idle_marks(
        &[core(
            1,
            Brand::Bybit,
            MarketKind::Futures,
            None,
            Some((&held, 0)),
        )],
        1.0,
    );
    let mark = &marks[&1];
    assert_eq!(mark.wallets.len(), 1);
    assert_eq!(mark.wallets[0].kind, WalletKind::Futures);
    assert_eq!(mark.wallets[0].value, 200.0);
}

/// Breakage: a spot and a futures core on one Gate account judged apart — each one's trading
/// wallet would be marked as the other's idle money.
#[test]
fn an_account_trades_from_every_wallet_its_cores_use() {
    let shared = wallets(vec![coin("USDT", 120.0)], vec![coin("usdt", 300.0)]);
    let marks = idle_marks(
        &[
            core(
                1,
                Brand::Gate,
                MarketKind::Futures,
                Some("1Btc"),
                Some((&shared, 0)),
            ),
            core(
                2,
                Brand::Gate,
                MarketKind::Spot,
                Some("1Btc"),
                Some((&shared, 0)),
            ),
        ],
        1.0,
    );
    assert!(marks.is_empty(), "{marks:?}");
}

/// Breakage: an account judged by its VISIBLE cores only — narrowed to the spot core, the panel
/// would mark the hidden futures core's working margin as idle money.
#[test]
fn a_hidden_sibling_still_counts_as_trading() {
    let shared = wallets(vec![coin("USDT", 120.0)], vec![coin("USDT", 300.0)]);
    let mut futures = core(
        1,
        Brand::Gate,
        MarketKind::Futures,
        Some("1Btc"),
        Some((&shared, 0)),
    );
    futures.visible = false;
    futures.counted = false;
    let spot = core(
        2,
        Brand::Gate,
        MarketKind::Spot,
        Some("1Btc"),
        Some((&shared, 0)),
    );
    let marks = idle_marks(&[futures, spot], 1.0);
    assert!(marks.is_empty(), "{marks:?}");
}

/// Breakage: the mark repeated on every core of a shared account, or put on a hidden one — the
/// same Funding balance on two rows reads as twice the money, and on a hidden row it is lost.
#[test]
fn a_shared_account_is_marked_once_on_its_first_visible_core() {
    let shared = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let mut hidden = core(
        4,
        Brand::Bybit,
        MarketKind::Futures,
        Some("acct"),
        Some((&shared, 0)),
    );
    hidden.visible = false;
    hidden.counted = false;
    let marks = idle_marks(
        &[
            hidden,
            core(
                5,
                Brand::Bybit,
                MarketKind::Spot,
                Some("acct"),
                Some((&shared, 0)),
            ),
            core(
                6,
                Brand::Bybit,
                MarketKind::Futures,
                Some("acct"),
                Some((&shared, 0)),
            ),
        ],
        1.0,
    );
    assert_eq!(marks.keys().copied().collect::<Vec<_>>(), [5]);
}

/// Breakage: an account judged by its first core's snapshot — after a transfer made from the
/// second core only that core is refreshed, and the stale first snapshot would keep showing
/// money that has already been moved.
#[test]
fn a_shared_account_is_judged_by_its_newest_snapshot() {
    let before = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let after = wallets(vec![coin("USDT", 300.0)], vec![]);
    let marks = idle_marks(
        &[
            core(
                5,
                Brand::Bybit,
                MarketKind::Futures,
                Some("acct"),
                Some((&before, 1_000)),
            ),
            core(
                6,
                Brand::Bybit,
                MarketKind::Spot,
                Some("acct"),
                Some((&after, 2_000)),
            ),
        ],
        1.0,
    );
    assert!(marks.is_empty(), "{marks:?}");
}

/// Breakage: dust, a core of unknown venue, or an exchange whose layout is not verified producing
/// a mark — dust is what the panel's slider hides everywhere else, and without a known trading
/// wallet any wallet might be the one the core trades from.
#[test]
fn dust_and_unverified_venues_are_not_marked() {
    let dusty = wallets(vec![coin("DOGE", 0.4)], vec![coin("USDT", 900.0)]);
    let spot_money = wallets(vec![coin("USDT", 50.0)], vec![]);
    let mut unknown = core(
        2,
        Brand::Binance,
        MarketKind::Futures,
        None,
        Some((&spot_money, 0)),
    );
    unknown.venue = None;
    let marks = idle_marks(
        &[
            core(
                1,
                Brand::Binance,
                MarketKind::Futures,
                None,
                Some((&dusty, 0)),
            ),
            unknown,
            core(
                3,
                Brand::Okx,
                MarketKind::Futures,
                None,
                Some((&spot_money, 0)),
            ),
        ],
        1.0,
    );
    assert!(marks.is_empty(), "{marks:?}");
}

/// Breakage: the footer adding the idle money of an account the total leaves out — with every
/// shown core of a Bybit account set to `TotalMode::Exclude`, «Not traded» and «Combined» would
/// add Funding money of an account the total itself does not count. The mark stays on the core.
#[test]
fn an_account_left_out_of_the_total_keeps_its_mark_but_not_the_footer_sum() {
    let excluded_held = wallets(vec![coin("USDT", 10.0)], vec![coin("USDT", 500.0)]);
    let counted_held = wallets(vec![coin("USDT", 10.0)], vec![coin("USDT", 40.0)]);
    let mut left_out = core(
        1,
        Brand::Bybit,
        MarketKind::Futures,
        Some("acct-a"),
        Some((&excluded_held, 0)),
    );
    left_out.counted = false;
    let marks = idle_marks(
        &[
            left_out,
            core(
                2,
                Brand::Bybit,
                MarketKind::Futures,
                Some("acct-b"),
                Some((&counted_held, 0)),
            ),
        ],
        1.0,
    );
    assert_eq!(marks[&1].wallets[0].value, 500.0, "the mark stays");
    assert!(!marks[&1].in_total);
    assert!(marks[&2].in_total);
    assert_eq!(footer_idle_total(marks.values()), 40.0);
}

/// Breakage: one excluded core taking its whole account out of the footer sum — a sibling on the
/// same account that the total counts still counts that account, so its idle money stays.
#[test]
fn one_counted_core_keeps_a_shared_account_in_the_footer_sum() {
    let shared = wallets(vec![coin("USDT", 10.0)], vec![coin("USDT", 500.0)]);
    let mut excluded = core(
        1,
        Brand::Bybit,
        MarketKind::Futures,
        Some("acct"),
        Some((&shared, 0)),
    );
    excluded.counted = false;
    let counted = core(
        2,
        Brand::Bybit,
        MarketKind::Spot,
        Some("acct"),
        Some((&shared, 0)),
    );
    let marks = idle_marks(&[excluded, counted], 1.0);
    assert_eq!(footer_idle_total(marks.values()), 500.0);
}
