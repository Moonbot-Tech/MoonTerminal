// Explicit imports, NOT `use super::*`: the panel's parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute.
use super::{FooterIdle, IdleInput, footer_idle, idle_report, trading_wallet};
use crate::panels::assets::cache::idle_inputs;
use moon_core::account::AccountIdentity;
use moon_core::config::{AppConfig, ServerConfig};
use moon_core::feed::{ConnStatus, TransferAssetRow, TransferAssetsSnapshot, WalletKind};
use moon_core::session::CoreStore;
use moon_core::venue::{Brand, CoreVenue, MarketKind, Venue, wallets_key};

/// A priced coin worth `value` USDT.
fn coin(currency: &str, value: f64) -> TransferAssetRow {
    TransferAssetRow {
        currency: currency.to_string(),
        amount: value,
        total: value,
        value_usdt: value,
    }
}

/// Wallets holding `spot` and `futures`, every list answered at one early moment.
fn wallets(spot: Vec<TransferAssetRow>, futures: Vec<TransferAssetRow>) -> TransferAssetsSnapshot {
    wallets_at(spot, futures, 1)
}

/// Wallets holding `spot` and `futures`, every list answered at `at_ms`.
fn wallets_at(
    spot: Vec<TransferAssetRow>,
    futures: Vec<TransferAssetRow>,
    at_ms: i64,
) -> TransferAssetsSnapshot {
    TransferAssetsSnapshot {
        spot,
        futures,
        quarterly: Vec::new(),
        arrived_ms: [at_ms; 3],
    }
}

/// A visible, ready core counted in the footer total, on `brand`/`kind`, logged into exchange
/// account number `account` (the user id on Binance, Bybit and Gate, the account id elsewhere),
/// holding `held`. `None` for the account leaves the core unidentified.
fn core<'a>(
    core: u64,
    brand: Brand,
    kind: MarketKind,
    account: Option<i64>,
    held: Option<&'a TransferAssetsSnapshot>,
) -> IdleInput<'a> {
    let venue = Venue { brand, kind };
    IdleInput {
        core,
        visible: true,
        counted: true,
        ready: true,
        exchange_known: true,
        venue: Some(venue),
        key: account.and_then(|number| {
            wallets_key(
                Some(venue),
                AccountIdentity::from_auth(&format!("acct-{number}"), "", number),
            )
        }),
        wallets: held,
        seen_ms: held.map_or([0; 3], |held| held.arrived_ms),
    }
}

/// A configured core and group, with all other connection settings at their defaults.
fn configured_core(id: u64, name: &str, group: &str) -> ServerConfig {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "uid": id,
        "name": name,
        "group": group,
    }))
    .expect("valid core fixture")
}

/// Run the wallet cache input path with two ready Binance Spot cores on different accounts.
/// Both report futures margin. The configured Futures core on acct-1 has no session.
fn report_with_binance_spot_cores(config: &AppConfig) -> super::IdleReport {
    let mut store = CoreStore::default();
    for id in [1, 3] {
        store.ensure(id);
        let data = store.core_mut(id).expect("registered spot core");
        data.status = ConnStatus::Ready;
        data.transfer_rev = 1;
        data.transfer_assets = wallets(Vec::new(), vec![coin("USDT", 100.0)]);
        data.transfer_seen_ms = data.transfer_assets.arrived_ms;
    }
    let venue = CoreVenue::identify(3, "", None);
    let mut inputs = idle_inputs(config, &store, |id| match id {
        1 => (Some(&venue), AccountIdentity::from_auth("acct-1", "", 100)),
        3 => (Some(&venue), AccountIdentity::from_auth("acct-3", "", 300)),
        _ => (None, None),
    });
    for input in &mut inputs {
        input.visible = matches!(input.core, 1 | 3);
        input.counted = input.visible;
    }
    idle_report(&inputs, 1.0)
}

/// The configured futures sibling belongs to a different connection group from the spot cores.
fn binance_roster() -> AppConfig {
    AppConfig::headless(vec![
        configured_core(1, "CORE_A", "UNIT_01"),
        configured_core(2, "CORE_B", "UNIT_02"),
        configured_core(3, "CORE_C", "UNIT_01"),
    ])
}

/// Omitting an Act-off core would mark its sibling's futures margin and the other account's.
#[test]
fn strict_idle_withholds_binance_marks_when_futures_core_is_off() {
    let mut config = binance_roster();
    config.servers[1].active = false;

    let report = report_with_binance_spot_cores(&config);

    assert!(report.marks.is_empty());
    assert_eq!(footer_idle(&report).waiting_for, 1);
}

/// Filtering out an inactive group would make the futures balance look unused on both accounts.
#[test]
fn strict_idle_withholds_binance_marks_when_futures_group_is_off() {
    let mut config = binance_roster();
    config.group_mut("UNIT_02").active = false;

    let report = report_with_binance_spot_cores(&config);

    assert!(report.marks.is_empty());
    assert_eq!(footer_idle(&report).waiting_for, 1);
}

/// Keeping a removed core in the blocker roster would prevent both idle marks from returning.
#[test]
fn strict_idle_restores_binance_marks_when_off_core_is_removed() {
    let mut config = binance_roster();
    config.servers[1].active = false;
    assert!(report_with_binance_spot_cores(&config).marks.is_empty());
    config.servers.remove(1);

    let report = report_with_binance_spot_cores(&config);

    assert_eq!(report.marks.len(), 2);
    for id in [1, 3] {
        assert_eq!(report.marks[&id].wallets[0].kind, WalletKind::Futures);
    }
    assert_eq!(footer_idle(&report).total, 200.0);
    assert_eq!(footer_idle(&report).waiting_for, 0);
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
    let report = idle_report(
        &[core(
            1,
            Brand::Bybit,
            MarketKind::Futures,
            Some(1),
            Some(&held),
        )],
        1.0,
    );
    let mark = &report.marks[&1];
    assert_eq!(mark.wallets.len(), 1);
    assert_eq!(mark.wallets[0].kind, WalletKind::Futures);
    assert_eq!(mark.wallets[0].value, 200.0);
    assert_eq!(report.waiting_for, 0);
}

/// Breakage: a spot and a futures core on one Gate account judged apart — each one's trading
/// wallet would be marked as the other's idle money.
#[test]
fn an_account_trades_from_every_wallet_its_cores_use() {
    let shared = wallets(vec![coin("USDT", 120.0)], vec![coin("usdt", 300.0)]);
    let report = idle_report(
        &[
            core(1, Brand::Gate, MarketKind::Futures, Some(7), Some(&shared)),
            core(2, Brand::Gate, MarketKind::Spot, Some(7), Some(&shared)),
        ],
        1.0,
    );
    assert!(report.marks.is_empty(), "{:?}", report.marks);
}

/// Breakage: Gate accounts told apart by the deposit address — one account states it as the
/// address, or as `User-<id>` when the exchange would not give it. Two connected cores of one
/// account then became two accounts, and the spot core's mark named the futures margin as money
/// to move out from under an open position. Both report one user id, and that is the account.
#[test]
fn gate_cores_stating_two_shapes_of_one_address_are_one_account() {
    let shared = wallets(vec![coin("USDT", 120.0)], vec![coin("USDT", 300.0)]);
    let gate = |core, kind, address| {
        let venue = Venue {
            brand: Brand::Gate,
            kind,
        };
        IdleInput {
            core,
            visible: true,
            counted: true,
            ready: true,
            exchange_known: true,
            venue: Some(venue),
            key: wallets_key(Some(venue), AccountIdentity::from_auth("", address, 4242)),
            wallets: Some(&shared),
            seen_ms: shared.arrived_ms,
        }
    };
    let report = idle_report(
        &[
            gate(1, MarketKind::Spot, "bc1qdeposit"),
            gate(2, MarketKind::Futures, "User-4242"),
        ],
        1.0,
    );
    assert!(report.marks.is_empty(), "{:?}", report.marks);
    assert_eq!(report.waiting_for, 0);
}

/// Breakage: `wallets_key` naming a Bybit account by its deposit address — the exchange gives one
/// BTC address per network and a core keeps whichever it was told last, so two cores of one
/// account can state two addresses. They then became two accounts: each carried the account's
/// Funding money as its own mark, and the footer summed it twice. Both report one user id, and
/// that is the account.
#[test]
fn bybit_cores_stating_two_addresses_of_one_account_are_marked_once() {
    let shared = wallets(vec![coin("USDT", 300.0)], vec![coin("USDT", 120.0)]);
    let bybit = |core, kind, address| {
        let venue = Venue {
            brand: Brand::Bybit,
            kind,
        };
        IdleInput {
            core,
            visible: true,
            counted: true,
            ready: true,
            exchange_known: true,
            venue: Some(venue),
            key: wallets_key(Some(venue), AccountIdentity::from_auth("", address, 5151)),
            wallets: Some(&shared),
            seen_ms: shared.arrived_ms,
        }
    };
    let report = idle_report(
        &[
            bybit(1, MarketKind::Spot, "bc1qfirstnetwork"),
            bybit(2, MarketKind::Futures, "bc1qothernetwork"),
        ],
        1.0,
    );
    assert_eq!(
        report.marks.keys().collect::<Vec<_>>(),
        [&1],
        "{:?}",
        report.marks
    );
    assert_eq!(report.marks[&1].wallets.len(), 1);
    assert_eq!(report.marks[&1].wallets[0].kind, WalletKind::Futures);
    assert_eq!(footer_idle(&report).total, 120.0);
    assert_eq!(report.waiting_for, 0);
}

/// Breakage: an account judged by its VISIBLE cores only — narrowed to the spot core, the panel
/// would mark the hidden futures core's working margin as idle money.
#[test]
fn a_hidden_sibling_still_counts_as_trading() {
    let shared = wallets(vec![coin("USDT", 120.0)], vec![coin("USDT", 300.0)]);
    let mut futures = core(1, Brand::Gate, MarketKind::Futures, Some(7), Some(&shared));
    futures.visible = false;
    futures.counted = false;
    let spot = core(2, Brand::Gate, MarketKind::Spot, Some(7), Some(&shared));
    let report = idle_report(&[futures, spot], 1.0);
    assert!(report.marks.is_empty(), "{:?}", report.marks);
}

/// Breakage: the mark repeated on every core of a shared account, or put on a hidden one — the
/// same Funding balance on two rows reads as twice the money, and on a hidden row it is lost.
#[test]
fn a_shared_account_is_marked_once_on_its_first_visible_core() {
    let shared = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let mut hidden = core(4, Brand::Bybit, MarketKind::Futures, Some(9), Some(&shared));
    hidden.visible = false;
    hidden.counted = false;
    let report = idle_report(
        &[
            hidden,
            core(5, Brand::Bybit, MarketKind::Spot, Some(9), Some(&shared)),
            core(6, Brand::Bybit, MarketKind::Futures, Some(9), Some(&shared)),
        ],
        1.0,
    );
    assert_eq!(report.marks.keys().copied().collect::<Vec<_>>(), [5]);
}

/// Breakage: an account judged by its first core's snapshot — after a transfer made from the
/// second core only that core is refreshed, and the stale first snapshot would keep showing
/// money that has already been moved.
#[test]
fn a_shared_account_is_judged_by_its_newest_snapshot() {
    let before = wallets_at(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)], 1_000);
    let after = wallets_at(vec![coin("USDT", 300.0)], vec![], 2_000);
    let report = idle_report(
        &[
            core(5, Brand::Bybit, MarketKind::Futures, Some(9), Some(&before)),
            core(6, Brand::Bybit, MarketKind::Spot, Some(9), Some(&after)),
        ],
        1.0,
    );
    assert!(report.marks.is_empty(), "{:?}", report.marks);
}

/// Breakage: dust, or an exchange whose layout is not verified, producing a mark — dust is what
/// the panel's slider hides everywhere else, and without a known trading wallet any wallet might
/// be the one the core trades from. Neither is "withheld": nothing is waiting on a core here.
#[test]
fn dust_and_unverified_venues_are_not_marked() {
    let dusty = wallets(vec![coin("DOGE", 0.4)], vec![coin("USDT", 900.0)]);
    let spot_money = wallets(vec![coin("USDT", 50.0)], vec![]);
    let report = idle_report(
        &[
            core(
                1,
                Brand::Binance,
                MarketKind::Futures,
                Some(1),
                Some(&dusty),
            ),
            core(
                3,
                Brand::Okx,
                MarketKind::Futures,
                Some(3),
                Some(&spot_money),
            ),
        ],
        1.0,
    );
    assert!(report.marks.is_empty(), "{:?}", report.marks);
    assert_eq!(report.waiting_for, 0);
}

/// Breakage: an unidentified core judged as an account of its own, or ignored — it carries no
/// mark itself, and it may be the sibling that trades the wallet a mark on its exchange would
/// name. Accounts of ITS exchange wait for it; another exchange's accounts do not.
#[test]
fn an_unidentified_core_keeps_its_exchange_unjudged() {
    let funding = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let spot_money = wallets(vec![coin("USDT", 50.0)], vec![coin("USDT", 10.0)]);
    let report = idle_report(
        &[
            core(
                1,
                Brand::Bybit,
                MarketKind::Futures,
                Some(9),
                Some(&funding),
            ),
            // Connected to Bybit, account not stated yet: it may well be on account 9.
            core(2, Brand::Bybit, MarketKind::Spot, None, Some(&funding)),
            core(
                3,
                Brand::Binance,
                MarketKind::Futures,
                Some(5),
                Some(&spot_money),
            ),
        ],
        1.0,
    );
    assert_eq!(
        report.marks.keys().copied().collect::<Vec<_>>(),
        [3],
        "only the Binance account is established"
    );
    assert_eq!(report.waiting_for, 1);
    assert_eq!(footer_idle(&report).waiting_for, 1);
}

/// Breakage: a core that has not said which exchange it is on — offline, or still connecting —
/// treated as nobody's sibling. It could be on any account, so nothing is marked until it
/// identifies itself. A core on an exchange this terminal does not model is another matter: it
/// is on none of the marked exchanges, and holding every mark back for it would be for nothing.
#[test]
fn a_core_of_unknown_exchange_keeps_every_account_unjudged() {
    let funding = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let bybit = || {
        core(
            1,
            Brand::Bybit,
            MarketKind::Futures,
            Some(9),
            Some(&funding),
        )
    };
    let mut offline = core(2, Brand::Gate, MarketKind::Futures, None, None);
    offline.ready = false;
    offline.exchange_known = false;
    offline.venue = None;
    offline.visible = false;
    offline.counted = false;
    let report = idle_report(&[bybit(), offline], 1.0);
    assert!(report.marks.is_empty(), "{:?}", report.marks);
    assert_eq!(report.waiting_for, 1);

    let mut not_modelled = core(2, Brand::Gate, MarketKind::Futures, None, None);
    not_modelled.venue = None;
    let report = idle_report(&[bybit(), not_modelled], 1.0);
    assert_eq!(report.marks.keys().copied().collect::<Vec<_>>(), [1]);
    assert_eq!(report.waiting_for, 0);
}

/// Breakage: the footer saying it waits for a core when there is nothing to wait for — with no
/// idle money on any account, an unidentified core hides nothing and must add no caveat.
#[test]
fn an_unidentified_core_withholds_nothing_when_there_is_no_idle_money() {
    let clean = wallets(vec![coin("USDT", 100.0)], vec![]);
    let report = idle_report(
        &[
            core(1, Brand::Bybit, MarketKind::Futures, Some(9), Some(&clean)),
            core(2, Brand::Bybit, MarketKind::Spot, None, None),
        ],
        1.0,
    );
    assert_eq!(report.waiting_for, 0);
    assert_eq!(footer_idle(&report), FooterIdle::default());
}

/// Breakage: the footer adding the idle money of an account the total leaves out — with every
/// shown core of a Bybit account set to `TotalMode::Exclude`, «Not traded» would add Funding
/// money of an account the total itself does not count. The mark stays on the core.
#[test]
fn an_account_left_out_of_the_total_keeps_its_mark_but_not_the_footer_sum() {
    let excluded_held = wallets(vec![coin("USDT", 10.0)], vec![coin("USDT", 500.0)]);
    let counted_held = wallets(vec![coin("USDT", 10.0)], vec![coin("USDT", 40.0)]);
    let mut left_out = core(
        1,
        Brand::Bybit,
        MarketKind::Futures,
        Some(1),
        Some(&excluded_held),
    );
    left_out.counted = false;
    let report = idle_report(
        &[
            left_out,
            core(
                2,
                Brand::Bybit,
                MarketKind::Futures,
                Some(2),
                Some(&counted_held),
            ),
        ],
        1.0,
    );
    assert_eq!(report.marks[&1].wallets[0].value, 500.0, "the mark stays");
    assert!(!report.marks[&1].in_total);
    assert!(report.marks[&2].in_total);
    assert_eq!(footer_idle(&report).total, 40.0);
}

/// Breakage: one excluded core taking its whole account out of the footer sum — a sibling on the
/// same account that the total counts still counts that account, so its idle money stays.
#[test]
fn one_counted_core_keeps_a_shared_account_in_the_footer_sum() {
    let shared = wallets(vec![coin("USDT", 10.0)], vec![coin("USDT", 500.0)]);
    let mut excluded = core(1, Brand::Bybit, MarketKind::Futures, Some(9), Some(&shared));
    excluded.counted = false;
    let counted = core(2, Brand::Bybit, MarketKind::Spot, Some(9), Some(&shared));
    let report = idle_report(&[excluded, counted], 1.0);
    assert_eq!(footer_idle(&report).total, 500.0);
}

/// Breakage: the footer dating its sum by the newest snapshot — the sum adds accounts fetched at
/// different times and is only as fresh as the stalest of them; a newer date would vouch for
/// money that may have been moved since.
#[test]
fn the_footer_sum_is_as_old_as_its_oldest_snapshot() {
    let first = wallets_at(vec![coin("USDT", 10.0)], vec![coin("USDT", 40.0)], 2_000);
    let second = wallets_at(vec![coin("USDT", 10.0)], vec![coin("USDT", 60.0)], 1_000);
    let report = idle_report(
        &[
            core(1, Brand::Bybit, MarketKind::Futures, Some(1), Some(&first)),
            core(2, Brand::Bybit, MarketKind::Futures, Some(2), Some(&second)),
        ],
        1.0,
    );
    assert_eq!(
        footer_idle(&report),
        FooterIdle {
            total: 100.0,
            as_of_ms: Some(1_000),
            waiting_for: 0,
        }
    );
}

/// Breakage: a snapshot dated as a whole — the core answers each wallet request on its own, so a
/// refresh can bring a fresh unified-account list while the Funding request fails. Dated together,
/// that core's OLD Funding list took the new time and won over the sibling's newer one, and the
/// mark named money that had already been moved.
#[test]
fn a_list_that_was_not_refreshed_does_not_pass_for_the_newest() {
    // Core 5: unified answered at 3 000, Funding still the list from 1 000.
    let mut half_refreshed = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    half_refreshed.arrived_ms = [3_000, 1_000, 0];
    // Core 6: both answered at 2 000, after the Funding money was moved.
    let moved = wallets_at(vec![coin("USDT", 300.0)], vec![], 2_000);
    let report = idle_report(
        &[
            core(
                5,
                Brand::Bybit,
                MarketKind::Futures,
                Some(9),
                Some(&half_refreshed),
            ),
            core(6, Brand::Bybit, MarketKind::Spot, Some(9), Some(&moved)),
        ],
        1.0,
    );
    assert!(report.marks.is_empty(), "{:?}", report.marks);

    // The same core alone has nothing newer to go by: its Funding list stands, at its own age.
    let report = idle_report(
        &[core(
            5,
            Brand::Bybit,
            MarketKind::Futures,
            Some(9),
            Some(&half_refreshed),
        )],
        1.0,
    );
    assert_eq!(report.marks[&5].wallets[0].value, 200.0);
    assert_eq!(report.marks[&5].as_of_ms, 1_000);
}

/// Breakage: a wallet no core was ever answered for read as an empty one, or as a current one —
/// a list that never arrived says nothing about the money in it.
#[test]
fn a_wallet_never_answered_is_not_judged() {
    let mut unanswered = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    unanswered.arrived_ms = [1_000, 0, 0];
    let report = idle_report(
        &[core(
            1,
            Brand::Bybit,
            MarketKind::Futures,
            Some(1),
            Some(&unanswered),
        )],
        1.0,
    );
    assert!(report.marks.is_empty(), "{:?}", report.marks);
}

/// Breakage: a core that lost its connection keeping the account it stated before — the session
/// retains its exchange, account and wallets, and with them it kept its mark and vouched for its
/// siblings' accounts while nobody could see what it was doing, or on which account.
#[test]
fn a_core_that_is_not_ready_is_unidentified_whatever_it_stated() {
    let funding = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let ready = || {
        core(
            1,
            Brand::Bybit,
            MarketKind::Futures,
            Some(9),
            Some(&funding),
        )
    };
    let mut away = core(2, Brand::Bybit, MarketKind::Spot, Some(9), Some(&funding));
    away.ready = false;
    let report = idle_report(&[ready(), away], 1.0);
    assert!(report.marks.is_empty(), "{:?}", report.marks);
    assert_eq!(report.waiting_for, 1);

    // Alone, it establishes no account at all: no mark of its own, and nothing waits on it.
    let mut alone = ready();
    alone.ready = false;
    let report = idle_report(&[alone], 1.0);
    assert!(report.marks.is_empty(), "{:?}", report.marks);
    assert_eq!(report.waiting_for, 0);
}

/// Breakage: the mark counting a coin the wallet column hides — the column shows a coin only
/// ABOVE the threshold, so one worth exactly the threshold was in the mark and in the footer sum
/// and nowhere in the wallet the mark sends the user to.
#[test]
fn a_coin_worth_exactly_the_threshold_is_dust() {
    let at_threshold = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 5.0)]);
    let above = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 5.01)]);
    let report = |held| {
        idle_report(
            &[core(
                1,
                Brand::Bybit,
                MarketKind::Futures,
                Some(1),
                Some(held),
            )],
            5.0,
        )
    };
    assert!(report(&at_threshold).marks.is_empty());
    assert_eq!(report(&above).marks[&1].wallets[0].value, 5.01);
}

/// Breakage: the footer counting every unidentified core as one it waits for — a core on another
/// exchange, or on one this terminal does not model, cannot be the sibling that trades the money
/// held back, and naming it sends the user after a core that changes nothing.
#[test]
fn the_footer_waits_only_for_cores_that_could_hold_the_money_back() {
    let funding = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let mut not_modelled = core(4, Brand::Gate, MarketKind::Futures, None, None);
    not_modelled.venue = None;
    let report = idle_report(
        &[
            core(
                1,
                Brand::Bybit,
                MarketKind::Futures,
                Some(9),
                Some(&funding),
            ),
            core(2, Brand::Bybit, MarketKind::Spot, None, None),
            core(3, Brand::Binance, MarketKind::Futures, None, None),
            not_modelled,
        ],
        1.0,
    );
    assert!(report.marks.is_empty(), "{:?}", report.marks);
    assert_eq!(footer_idle(&report).waiting_for, 1, "only the Bybit core");
}

/// Breakage: the footer saying it waits for a core over money its sum would not include — with
/// every shown core of the account set to `TotalMode::Exclude` the mark is still withheld, but
/// «Not traded» never counted that account and has nothing to explain.
#[test]
fn money_the_total_leaves_out_is_not_waited_for() {
    let funding = wallets(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)]);
    let mut left_out = core(
        1,
        Brand::Bybit,
        MarketKind::Futures,
        Some(9),
        Some(&funding),
    );
    left_out.counted = false;
    let report = idle_report(
        &[
            left_out,
            core(2, Brand::Bybit, MarketKind::Spot, None, None),
        ],
        1.0,
    );
    assert!(report.marks.is_empty(), "{:?}", report.marks);
    assert_eq!(footer_idle(&report), FooterIdle::default());
}

/// Breakage: an older list standing in for a newer one that is gone — the core that heard of the
/// transfer has its feed replaced and starts from empty, unanswered lists, so the newest list
/// LEFT is the sibling's from before the transfer, and the mark for the moved money comes back.
/// The wallet is not judged until some core of the account is answered for it again.
#[test]
fn a_list_lost_with_its_feed_is_not_replaced_by_an_older_one() {
    let before = wallets_at(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)], 1_000);
    let wiped = wallets_at(vec![], vec![], 0);
    let sibling = || core(5, Brand::Bybit, MarketKind::Futures, Some(9), Some(&before));
    // Core 6 held lists from 2 000 — after the money was moved — and lost them.
    let mut restarted = core(6, Brand::Bybit, MarketKind::Spot, Some(9), Some(&wiped));
    restarted.seen_ms = [2_000, 2_000, 0];
    let report = idle_report(&[sibling(), restarted], 1.0);
    assert!(report.marks.is_empty(), "{:?}", report.marks);

    // Answered again, the wallet is judged again — here it still holds the money.
    let again = wallets_at(vec![coin("USDT", 100.0)], vec![coin("USDT", 200.0)], 3_000);
    let mut refreshed = core(6, Brand::Bybit, MarketKind::Spot, Some(9), Some(&again));
    refreshed.seen_ms = [3_000, 3_000, 0];
    let report = idle_report(&[sibling(), refreshed], 1.0);
    assert_eq!(report.marks[&5].wallets[0].value, 200.0);
    assert_eq!(report.marks[&5].as_of_ms, 3_000);
}
