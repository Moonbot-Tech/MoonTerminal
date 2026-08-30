use moonproto::ExchangeCode;

use super::{ARB_VENUES, Brand, CoreVenue, MarketKind, Venue, arb_alias, arb_row_matches, venue};
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

/// An arbitrage spelling must agree with the venue the same code resolves to.
///
/// The arbitrage panel's word for a venue is its brand plus the market kind's letter — `BinanceQ`
/// is Binance COIN-M, `GateF` is Gate futures — so the two halves of this module cannot be allowed
/// to describe one code differently. The spellings are the PANEL's, which is why the brand's word
/// here is not always `Brand::display`: the panel writes `Bitget`, `Okx` and `Htx`, and abbreviates
/// Hyperliquid to `HL_`.
///
/// Breakage: mistyping a suffix (`BinanceF` on the spot code) or moving a code to another brand in
/// `venue` without moving its spelling. Either paints one exchange's spread under another's name,
/// which is the one mistake an arbitrage column must never make.
#[test]
fn arbitrage_spellings_agree_with_the_venue_each_code_resolves_to() {
    let panel_word = |brand: Brand| match brand {
        Brand::Binance => "Binance",
        Brand::Bybit => "Bybit",
        Brand::Htx => "Htx",
        Brand::Gate => "Gate",
        Brand::BitGet => "Bitget",
        Brand::Hyperliquid => "HL_",
        Brand::Okx => "Okx",
    };
    let letter = |kind: MarketKind| match kind {
        MarketKind::Spot => "S",
        MarketKind::Futures => "F",
        MarketKind::Quarterly => "Q",
    };
    let mut checked = 0;
    for (code, alias) in ARB_VENUES {
        // Arbitrage-only codes — Forex, UpBit, BinAlpha, the delisted Bittrex, the sideless OKX
        // constant — are price sources rather than venues a core connects to, so the directory
        // deliberately does not resolve them and there is nothing to agree with.
        let Some(venue) = venue(code) else { continue };
        assert_eq!(
            alias,
            format!("{}{}", panel_word(venue.brand), letter(venue.kind)),
            "ordinal {code} is spelled against the venue it resolves to"
        );
        checked += 1;
    }
    // Counted rather than pinned to a literal: a new exchange added correctly to BOTH halves must
    // pass, and only a directory entry the roster never names may fail. The count still has to be
    // asserted — without it a `venue` that resolved nothing would let this test compare nothing and
    // pass in silence.
    let directory_size = (0..=u8::MAX).filter(|code| venue(*code).is_some()).count();
    assert_eq!(
        checked, directory_size,
        "every ordinal the directory knows must be covered by the roster"
    );
}

/// Every venue a core can CONNECT to must also be nameable as an arbitrage source.
///
/// The two are one exchange asked about twice: a core on Bitget futures is also a venue somebody
/// else compares a price against. A code the directory learned about is therefore a code the
/// arbitrage roster owes a word for.
///
/// Breakage: an exchange added upstream and wired into `venue` while `ARB_VENUES` is forgotten. The
/// column then prints it as a bare `#16` and the settings window cannot offer it a colour — the
/// exact split this table was merged to end.
#[test]
fn every_directory_venue_has_an_arbitrage_spelling() {
    for code in 0..=u8::MAX {
        if venue(code).is_some() {
            assert!(
                arb_alias(code).is_some(),
                "ordinal {code} is a venue but has no arbitrage spelling"
            );
        }
    }
}

/// One code, one row; one row, one word.
///
/// Breakage: a duplicated code gives the settings window two rows for one venue, whose colour and
/// visibility then depend on which the lookup finds first. A duplicated word puts two venues under
/// one heading in the column, where a reader cannot tell whose spread they are looking at.
#[test]
fn arbitrage_venues_are_listed_once_each() {
    let mut codes: Vec<u8> = ARB_VENUES.iter().map(|(code, _)| *code).collect();
    codes.sort_unstable();
    let total = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), total, "a platform code appears twice");

    let mut names: Vec<&str> = ARB_VENUES.iter().map(|(_, name)| *name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "two venues share a spelling");

    // An empty spelling reads as a name and prints as a blank heading over somebody's spread. The
    // table used to guard itself — an empty string was how the old match said "no name" and fell
    // through to the number — so the guard moves here rather than disappearing.
    for (code, name) in ARB_VENUES {
        assert!(!name.is_empty(), "ordinal {code} has a blank spelling");
    }

    // The number fallback must stay reachable: it is what says "this build has never seen this
    // platform" instead of inventing a word for it.
    assert!(arb_alias(200).is_none());
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

fn core_venue(code: u8, dex: &str) -> CoreVenue {
    CoreVenue {
        id: crate::feed::ExchangeId::with_dex(code, dex),
        dex: dex.to_string(),
        reported: String::new(),
    }
}

/// An ordinary exchange is matched by its platform ordinal, which the arbitrage code copies.
#[test]
fn an_exchange_matches_the_core_on_the_same_platform() {
    assert!(core_venue(4, "").matches_arb(4, ""));
    assert!(!core_venue(3, "").matches_arb(4, ""));
}

/// A Hyperliquid deployer shares the futures ordinal with every other deployer, so only its DEX
/// name identifies it — and a plain Hyperliquid core is NOT one of them.
#[test]
fn a_deployer_matches_by_its_dex_name() {
    assert!(core_venue(13, "hyna").matches_arb(51, "hyna"));
    assert!(!core_venue(13, "para").matches_arb(51, "hyna"));
    assert!(
        !core_venue(13, "").matches_arb(51, "hyna"),
        "plain Hyperliquid futures is not the hyna deployer"
    );
}

/// The other direction of the same rule: a core WITH a DEX is not the exchange the ordinal names,
/// or every deployer would answer for Hyperliquid futures itself.
#[test]
fn an_exchange_does_not_match_a_core_with_a_dex() {
    assert!(!core_venue(13, "xyz").matches_arb(13, ""));
    assert!(core_venue(13, "").matches_arb(13, ""));
}

/// The free function and the method are one rule, so the caption pass — which holds bare pairs —
/// cannot dim a row the click would open.
#[test]
fn the_pair_form_answers_exactly_like_the_venue_form() {
    for (code, dex) in [(4u8, ""), (13, "hyna"), (13, "")] {
        for row in [(4u8, ""), (51, "hyna"), (13, "")] {
            assert_eq!(
                arb_row_matches((code, dex), row),
                core_venue(code, dex).matches_arb(row.0, row.1),
            );
        }
    }
}
