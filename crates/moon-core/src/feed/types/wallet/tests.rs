//! Per-brand wallet naming and visibility (#1347).

use super::WalletKind;
use crate::venue::Brand;

/// English Assets dictionary, the fallback every language is checked against.
const EN_ASSETS: &str = include_str!("../../../../../../locales/en/assets.en.yml");

/// A Bybit core reports its Unified Trading account as `Spot` and its Funding wallet as
/// `Futures`; both must be named that way, on spot and futures Bybit cores alike.
///
/// Mutation: drop either Bybit arm of `label_key`, or match it only for one market kind. The
/// Bybit wallet then reads as "Spot"/"Futures" again — the mislabel the issue reported.
#[test]
fn bybit_names_its_unified_and_funding_wallets() {
    let bybit = Some(Brand::Bybit);
    assert_eq!(
        WalletKind::Spot.label_key(bybit),
        "assets.wallet.kind.bybit_unified"
    );
    assert_eq!(
        WalletKind::Futures.label_key(bybit),
        "assets.wallet.kind.bybit_funding"
    );
}

/// Every other brand, and a core whose venue is not known yet, keeps the generic names.
///
/// Mutation: apply the Bybit names to any brand (or to `None`). A Binance futures core, whose
/// margin really is in its Futures wallet, would then read "Funding".
#[test]
fn other_brands_keep_the_generic_names() {
    let mut brands: Vec<Option<Brand>> = Brand::ALL
        .into_iter()
        .filter(|brand| *brand != Brand::Bybit)
        .map(Some)
        .collect();
    brands.push(None);
    for brand in brands {
        assert_eq!(WalletKind::Spot.label_key(brand), "assets.wallet.kind.spot");
        assert_eq!(
            WalletKind::Futures.label_key(brand),
            "assets.wallet.kind.futures"
        );
        assert_eq!(
            WalletKind::Quarterly.label_key(brand),
            "assets.wallet.kind.quarterly"
        );
    }
}

/// Bybit drops its empty `Quarterly` column, but never one that reports a balance.
///
/// Mutation: hide Bybit's Quarterly unconditionally (funds there vanish from view), or not at
/// all (the always-empty column the issue reported comes back).
#[test]
fn bybit_hides_quarterly_only_while_it_is_empty() {
    let bybit = Some(Brand::Bybit);
    assert!(!WalletKind::Quarterly.is_shown(bybit, 0));
    assert!(WalletKind::Quarterly.is_shown(bybit, 1));
    assert!(WalletKind::Spot.is_shown(bybit, 0));
    assert!(WalletKind::Futures.is_shown(bybit, 0));
}

/// Every other brand, and an unidentified core, keeps all three columns even when empty: a
/// Binance COIN-M core's Quarterly wallet is a real account.
///
/// Mutation: hide an empty Quarterly for every brand.
#[test]
fn other_brands_show_every_wallet_even_empty() {
    let mut brands: Vec<Option<Brand>> = Brand::ALL
        .into_iter()
        .filter(|brand| *brand != Brand::Bybit)
        .map(Some)
        .collect();
    brands.push(None);
    for brand in brands {
        for kind in WalletKind::ALL {
            assert!(kind.is_shown(brand, 0), "{kind:?} hidden for {brand:?}");
        }
    }
}

/// A key the dictionary lacks draws the key itself, and the contract test that catches a missing
/// key only reads `t!("…")` literals — it cannot see one returned by `label_key`.
///
/// Mutation: rename a key in `label_key` or in `assets.en.yml` without the other.
#[test]
fn every_wallet_label_key_is_in_the_dictionary() {
    let mut brands: Vec<Option<Brand>> = Brand::ALL.into_iter().map(Some).collect();
    brands.push(None);
    for brand in brands {
        for kind in WalletKind::ALL {
            let key = kind.label_key(brand);
            assert!(
                EN_ASSETS
                    .lines()
                    .any(|line| line.starts_with(&format!("{key}:"))),
                "{key} is missing from locales/en/assets.en.yml"
            );
        }
    }
}
