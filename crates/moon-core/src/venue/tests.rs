use moonproto::ExchangeCode;

use super::{venue, Brand, CoreVenue, MarketKind, Venue};
use crate::symbol::Exchange;

/// The ordinals are wire values, so the oracle is moonproto's own constants rather than the
/// literals this table was written from: a renumbering upstream must fail here.
///
/// Breakage: swapping two arms, or giving `QBinance` the plain Binance venue again, changes the
/// logo, the caption, the market-name rules and the order-book kind all at once.
#[test]
fn platform_ordinals_resolve_to_their_venue() {
    let cases = [
        (ExchangeCode::FBybit, Brand::Bybit, MarketKind::Futures),
        (ExchangeCode::Binance, Brand::Binance, MarketKind::Spot),
        (ExchangeCode::FBinance, Brand::Binance, MarketKind::Futures),
        (ExchangeCode::Huobi, Brand::Htx, MarketKind::Spot),
        (
            ExchangeCode::QBinance,
            Brand::Binance,
            MarketKind::Quarterly,
        ),
        (ExchangeCode::ByBit, Brand::Bybit, MarketKind::Spot),
        (ExchangeCode::Gate, Brand::Gate, MarketKind::Spot),
        (ExchangeCode::FGate, Brand::Gate, MarketKind::Futures),
        (ExchangeCode::BitGet, Brand::BitGet, MarketKind::Spot),
        (ExchangeCode::FBitGet, Brand::BitGet, MarketKind::Futures),
        (ExchangeCode::Hyper, Brand::Hyperliquid, MarketKind::Spot),
        (
            ExchangeCode::FHyper,
            Brand::Hyperliquid,
            MarketKind::Futures,
        ),
        (ExchangeCode::OKX, Brand::Okx, MarketKind::Spot),
        (ExchangeCode::FOKX, Brand::Okx, MarketKind::Futures),
    ];
    for (code, brand, kind) in cases {
        assert_eq!(
            venue(code.stable_id()),
            Some(Venue { brand, kind }),
            "{code:?} (ordinal {})",
            code.stable_id()
        );
    }
}

/// Every ordinal a core can report is either a venue this build knows or explicitly nothing.
///
/// Breakage: a `_ => Some(...)` fallback arm would hand a future exchange somebody else's brand,
/// which is worse than showing it unbranded.
#[test]
fn only_known_platforms_have_a_venue() {
    for code in 0..=u8::MAX {
        let known = (2..=ExchangeCode::FOKX.stable_id()).contains(&code);
        assert_eq!(
            venue(code).is_some(),
            known,
            "ordinal {code} must {}resolve",
            if known { "" } else { "not " }
        );
    }
    // The two live ordinals that are deliberately not venues: no platform, and a delisted one.
    assert!(venue(ExchangeCode::None.stable_id()).is_none());
    assert!(venue(ExchangeCode::WasBittrex.stable_id()).is_none());
}

/// The naming family a venue reports must be the one `symbol::parse` is dispatched by, and COIN-M
/// must keep its own rules: `BTCUSD_PERP` splits differently from `BTCUSDT`.
///
/// Breakage: folding Quarterly into the plain Binance family silently changes which coin every
/// COIN-M market name resolves to.
#[test]
fn naming_family_follows_brand_and_kind() {
    let coin_m = venue(ExchangeCode::QBinance.stable_id()).expect("QBinance is a known venue");
    assert_eq!(coin_m.naming(), Exchange::BinanceCoinM);
    let usd_m = venue(ExchangeCode::FBinance.stable_id()).expect("FBinance is a known venue");
    assert_eq!(usd_m.naming(), Exchange::Binance);
    let spot = venue(ExchangeCode::Binance.stable_id()).expect("Binance is a known venue");
    assert_eq!(spot.naming(), Exchange::Binance);
}

/// Only spot venues pull the spot order book; both derivative kinds pull the futures one.
///
/// Breakage: `is_spot` following the brand instead of the kind would ask a futures provider for a
/// spot book, which returns nothing and empties the panel.
#[test]
fn spot_venues_are_exactly_the_spot_kind() {
    let spot: Vec<u8> = (0..=u8::MAX)
        .filter(|code| venue(*code).is_some_and(Venue::is_spot))
        .collect();
    assert_eq!(
        spot,
        vec![
            ExchangeCode::Binance.stable_id(),
            ExchangeCode::Huobi.stable_id(),
            ExchangeCode::ByBit.stable_id(),
            ExchangeCode::Gate.stable_id(),
            ExchangeCode::BitGet.stable_id(),
            ExchangeCode::Hyper.stable_id(),
            ExchangeCode::OKX.stable_id(),
        ]
    );
    let quarterly = venue(ExchangeCode::QBinance.stable_id()).expect("QBinance is a known venue");
    assert!(!quarterly.is_spot(), "COIN-M delivery is a derivative");
}

/// Each brand ships exactly one logo stem and one display spelling, so two cores of the same brand
/// cannot render as two rows.
///
/// Breakage: a duplicated slug would point two brands at one file; a duplicated display name would
/// merge two brands in every caption.
#[test]
fn brands_have_distinct_slugs_and_names() {
    let brands = Brand::ALL;
    let mut slugs: Vec<&str> = brands.iter().map(|brand| brand.slug()).collect();
    slugs.sort_unstable();
    let unique = slugs.len();
    slugs.dedup();
    assert_eq!(slugs.len(), unique, "brand slugs must be distinct");

    let mut names: Vec<&str> = brands.iter().map(|brand| brand.display()).collect();
    names.sort_unstable();
    let unique = names.len();
    names.dedup();
    assert_eq!(names.len(), unique, "brand names must be distinct");

    // A brand reachable through an ordinal but missing from ALL would skip the logo prewarm.
    for code in 0..=u8::MAX {
        if let Some(venue) = venue(code) {
            assert!(
                brands.contains(&venue.brand),
                "ordinal {code} resolves to a brand missing from Brand::ALL"
            );
        }
    }
}

/// Identification must agree with what a caption can actually draw.
///
/// Breakage: testing the raw string for emptiness lets a caption of nothing but bidi marks count as
/// a name; the venue then gets a section of its own headed with the not-identified wording — the
/// duplicate heading `is_nameable` exists to prevent. Keeping `reported` for a KNOWN ordinal would
/// put a core build's spelling into this value's equality and make a reconnect regroup the list.
#[test]
fn only_a_printable_caption_makes_an_unknown_ordinal_nameable() {
    let blank = CoreVenue::identify(200, "", Some(" \u{202e}\u{7} "));
    assert!(!blank.is_nameable(), "nothing printable is not a name");
    assert_eq!(blank.reported, "");

    let odd = CoreVenue::identify(200, "", Some("Kra\u{200b}ken "));
    assert!(odd.is_nameable());
    assert_eq!(
        odd.reported, "Kraken",
        "unprintables are stripped, not kept"
    );

    let known = CoreVenue::identify(
        ExchangeCode::QBinance.stable_id(),
        "",
        Some("Binance Quarterly"),
    );
    assert!(known.is_nameable(), "the directory names it");
    assert_eq!(
        known.reported, "",
        "a known ordinal must not carry the core's own spelling"
    );
}

/// The three market kinds must caption from three distinct dictionary keys.
///
/// Breakage: pointing two kinds at one key makes a COIN-M core read as plain futures.
#[test]
fn market_kinds_have_distinct_label_keys() {
    let keys = [
        MarketKind::Spot.label_key(),
        MarketKind::Futures.label_key(),
        MarketKind::Quarterly.label_key(),
    ];
    let mut sorted = keys;
    sorted.sort_unstable();
    let unique = sorted.len();
    let mut deduped = sorted.to_vec();
    deduped.dedup();
    assert_eq!(deduped.len(), unique, "market-kind keys must be distinct");
}
