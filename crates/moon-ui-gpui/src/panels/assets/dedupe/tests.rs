use super::{BalanceFigures, TotalMember, fold_accounts};
use moon_core::account::AccountIdentity;
use moon_core::config::TotalMode;
use moon_core::session::BalanceState;
use moon_core::venue::{AccountMergeKey, Brand, MarketKind, Venue, merge_key};

/// Merge key of a core on `brand`/`kind` logged into `account`.
fn key(brand: Brand, kind: MarketKind, account: &str) -> Option<AccountMergeKey> {
    merge_key(
        Some(Venue { brand, kind }),
        AccountIdentity::from_auth(account, "", 0),
    )
}

/// Merge key of a Binance core logged into `uid`.
fn binance(kind: MarketKind, uid: i64) -> Option<AccountMergeKey> {
    merge_key(
        Some(Venue {
            brand: Brand::Binance,
            kind,
        }),
        AccountIdentity::from_auth("", "", uid),
    )
}

/// A live core with `total` as both its free and total figure.
fn member(name: &str, total: f64, merge: Option<AccountMergeKey>, mode: TotalMode) -> TotalMember {
    TotalMember {
        name: name.to_string(),
        figures: BalanceFigures {
            state: BalanceState::Live,
            free: total,
            total,
        },
        merge,
        mode,
    }
}

/// Totals of the rows that reach the sum, in order.
fn totals(members: &[TotalMember]) -> Vec<f64> {
    fold_accounts(members)
        .rows
        .iter()
        .map(|r| r.total)
        .collect()
}

/// Breakage: `dedupe.rs::fold_accounts` never matching an existing group — two cores on one
/// account would both be summed and the footer would double that account's money. Also pins
/// which reading is counted: the larger total, the earlier one on a tie.
#[test]
fn two_auto_cores_on_one_account_count_once_at_the_larger_reading() {
    let k = key(Brand::Bybit, MarketKind::Futures, "acct-a");
    let out = fold_accounts(&[
        member("core-a", 100.0, k.clone(), TotalMode::Auto),
        member("core-b", 101.0, k.clone(), TotalMode::Auto),
    ]);
    assert_eq!(
        out.rows.iter().map(|r| r.total).collect::<Vec<_>>(),
        [101.0]
    );
    assert_eq!(out.folded.len(), 1);
    assert_eq!(out.folded[0].kept, "core-b");
    assert_eq!(out.folded[0].folded, ["core-a"]);
    assert_eq!(out.folded[0].brand, Some(Brand::Bybit));

    let tie = fold_accounts(&[
        member("core-a", 100.0, k.clone(), TotalMode::Auto),
        member("core-b", 100.0, k, TotalMode::Auto),
    ]);
    assert_eq!(
        tie.folded[0].kept, "core-a",
        "a tie keeps the earliest core"
    );
}

/// Breakage: `fold_accounts` grouping every `Auto` core regardless of key — two different
/// accounts would collapse into one and the total would drop an account.
#[test]
fn different_accounts_are_summed_apart() {
    assert_eq!(
        totals(&[
            member(
                "core-a",
                10.0,
                key(Brand::Bybit, MarketKind::Futures, "acct-a"),
                TotalMode::Auto
            ),
            member(
                "core-b",
                20.0,
                key(Brand::Bybit, MarketKind::Futures, "acct-b"),
                TotalMode::Auto
            ),
        ]),
        [10.0, 20.0]
    );
}

/// Breakage: `fold_accounts` treating two unknown accounts (`merge: None`) as equal — every core
/// without an identity would fold into one.
#[test]
fn cores_without_an_account_never_merge() {
    assert_eq!(
        totals(&[
            member("core-a", 10.0, None, TotalMode::Auto),
            member("core-b", 20.0, None, TotalMode::Auto),
        ]),
        [10.0, 20.0]
    );
}

/// Breakage: `fold_accounts` letting an `Exclude` core without identity fall into the solo arm —
/// the user's exclusion would be ignored for exactly the cores the setting exists for.
#[test]
fn an_excluded_core_is_left_out_even_without_an_account() {
    let out = fold_accounts(&[
        member("core-a", 10.0, None, TotalMode::Exclude),
        member("core-b", 20.0, None, TotalMode::Auto),
    ]);
    assert_eq!(out.rows.iter().map(|r| r.total).collect::<Vec<_>>(), [20.0]);
    assert_eq!(out.excluded_by_user, ["core-a"]);
}

/// Breakage: `fold_accounts` folding a `Separate` core into an `Auto` group on its key — the
/// user asked for that core to be counted on its own.
#[test]
fn a_separate_core_counts_beside_an_auto_core_on_its_account() {
    let k = key(Brand::Okx, MarketKind::Futures, "acct-a");
    let out = fold_accounts(&[
        member("core-a", 10.0, k.clone(), TotalMode::Auto),
        member("core-b", 20.0, k, TotalMode::Separate),
    ]);
    assert_eq!(
        out.rows.iter().map(|r| r.total).collect::<Vec<_>>(),
        [10.0, 20.0]
    );
    assert!(out.folded.is_empty());
}

/// Breakage: `fold_accounts` reporting a fold (or dropping the row) when no member of a group
/// has a usable figure — the total would stop saying the account is still missing.
#[test]
fn a_group_with_nothing_usable_keeps_its_first_row_and_reports_no_fold() {
    let k = key(Brand::Bybit, MarketKind::Futures, "acct-a");
    let mut a = member("core-a", 0.0, k.clone(), TotalMode::Auto);
    a.figures.state = BalanceState::Awaiting;
    let mut b = member("core-b", 0.0, k, TotalMode::Auto);
    b.figures.state = BalanceState::Awaiting;
    let out = fold_accounts(&[a, b]);
    assert_eq!(out.rows.len(), 1);
    assert_eq!(out.rows[0].state, BalanceState::Awaiting);
    assert!(out.folded.is_empty());
}

/// Breakage: Binance becoming a unified-wallet brand, or the futures kind dropping out of its
/// key — spot and futures are separate money on one uid, while two futures cores on it are one
/// wallet; Bitget spot and futures on one account share one wallet.
#[test]
fn binance_wallets_split_by_market_and_bitget_folds() {
    assert_eq!(
        totals(&[
            member(
                "core-a",
                10.0,
                binance(MarketKind::Spot, 777),
                TotalMode::Auto
            ),
            member(
                "core-b",
                20.0,
                binance(MarketKind::Futures, 777),
                TotalMode::Auto
            ),
        ]),
        [10.0, 20.0],
        "Binance spot and futures on one uid are both counted"
    );
    assert_eq!(
        totals(&[
            member(
                "core-a",
                20.0,
                binance(MarketKind::Futures, 777),
                TotalMode::Auto
            ),
            member(
                "core-b",
                20.0,
                binance(MarketKind::Futures, 777),
                TotalMode::Auto
            ),
        ]),
        [20.0],
        "two Binance futures cores on one uid count once"
    );
    assert_eq!(
        totals(&[
            member(
                "core-a",
                30.0,
                key(Brand::BitGet, MarketKind::Spot, "acct-a"),
                TotalMode::Auto
            ),
            member(
                "core-b",
                30.0,
                key(Brand::BitGet, MarketKind::Futures, "acct-a"),
                TotalMode::Auto
            ),
        ]),
        [30.0],
        "Bitget spot and futures on one account count once"
    );
}

/// Merge key of a core on `brand` stating `address` and a Binance uid field.
fn addressed(brand: Brand, address: &str, uid: i64) -> Option<AccountMergeKey> {
    merge_key(
        Some(Venue {
            brand,
            kind: MarketKind::Futures,
        }),
        AccountIdentity::from_auth("", address, uid),
    )
}

/// Breakage: `venue.rs::merge_key` keying Hyperliquid on the Binance uid field (a checksum of the
/// address there) — one wallet would be summed twice, or two wallets folded into one.
#[test]
fn hyperliquid_cores_fold_by_address_whatever_the_uid_field() {
    assert_eq!(
        totals(&[
            member(
                "core-a",
                10.0,
                addressed(Brand::Hyperliquid, "0xabc", 1),
                TotalMode::Auto
            ),
            member(
                "core-b",
                10.0,
                addressed(Brand::Hyperliquid, "0xabc", 2),
                TotalMode::Auto
            ),
        ]),
        [10.0],
        "one address counts once"
    );
    assert_eq!(
        totals(&[
            member(
                "core-a",
                10.0,
                addressed(Brand::Hyperliquid, "0xabc", 1),
                TotalMode::Auto
            ),
            member(
                "core-b",
                20.0,
                addressed(Brand::Hyperliquid, "0xdef", 1),
                TotalMode::Auto
            ),
        ]),
        [10.0, 20.0],
        "two addresses are two wallets"
    );
}

/// Breakage: Gate or HTX folding on their stated identity — it is not proof of one wallet, so
/// both cores must be counted.
#[test]
fn gate_and_htx_cores_on_one_identity_are_both_counted() {
    for brand in [Brand::Gate, Brand::Htx] {
        assert_eq!(
            totals(&[
                member(
                    "core-a",
                    10.0,
                    addressed(brand, "0xabc", 7),
                    TotalMode::Auto
                ),
                member(
                    "core-b",
                    20.0,
                    addressed(brand, "0xabc", 7),
                    TotalMode::Auto
                ),
            ]),
            [10.0, 20.0],
            "{brand:?}"
        );
    }
}

/// Breakage: `fold_accounts` naming every non-kept member as folded — a core with no data would
/// be reported as counted elsewhere though it contributed nothing.
#[test]
fn an_unusable_member_is_not_named_as_folded() {
    let k = key(Brand::Bybit, MarketKind::Futures, "acct-a");
    let mut waiting = member("core-c", 0.0, k.clone(), TotalMode::Auto);
    waiting.figures.state = BalanceState::Awaiting;
    let out = fold_accounts(&[
        member("core-a", 10.0, k.clone(), TotalMode::Auto),
        waiting,
        member("core-b", 5.0, k.clone(), TotalMode::Auto),
    ]);
    assert_eq!(out.rows.iter().map(|r| r.total).collect::<Vec<_>>(), [10.0]);
    assert_eq!(out.folded.len(), 1);
    assert_eq!(out.folded[0].folded, ["core-b"]);

    let mut alone = member("core-c", 0.0, k.clone(), TotalMode::Auto);
    alone.figures.state = BalanceState::Awaiting;
    let solo = fold_accounts(&[member("core-a", 10.0, k, TotalMode::Auto), alone]);
    assert!(solo.folded.is_empty(), "nothing usable was folded");
}
