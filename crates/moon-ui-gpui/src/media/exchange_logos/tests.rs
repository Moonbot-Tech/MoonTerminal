use super::{BRANDS, EMBEDDED, RASTER_PX, load, logo_slug};

/// Core-reported names carry a market type, a leading futures letter, or an old brand spelling, and
/// all three must land on the one shipped logo.
///
/// Breakage: dropping the market-word strip splits `Binance Futures` off from `Binance`; reordering
/// `BRANDS` so `hyper` precedes `hyperliquid` sends Hyperliquid to the wrong stem.
#[test]
fn reported_exchange_names_resolve_to_one_brand() {
    for (name, slug) in [
        ("Binance", "binance"),
        ("Binance Futures", "binance"),
        ("FBinance", "binance"),
        ("Binance COIN-M", "binance"),
        ("ByBit Spot", "bybit"),
        ("Gate.io", "gate"),
        ("FGate", "gate"),
        ("OKEx", "okx"),
        ("OKX Futures", "okx"),
        ("Huobi", "htx"),
        ("HTX Spot", "htx"),
        ("Hyperliquid", "hyperliquid"),
        ("FHyper", "hyperliquid"),
        ("Bitget Futures", "bitget"),
    ] {
        assert_eq!(logo_slug(name), Some(slug), "{name} must resolve to {slug}");
    }
}

/// An unknown or empty exchange must resolve to nothing rather than to the nearest brand.
///
/// Breakage: matching on a prefix of the needle instead of the whole one would let `Kraken` or a
/// bare market type claim a logo it has no right to.
#[test]
fn unknown_exchanges_get_no_logo() {
    for name in ["", "   ", "Kraken", "Spot", "Futures", "MEXC"] {
        assert_eq!(logo_slug(name), None, "{name:?} must have no logo");
    }
}

/// Every brand the resolver can return must exist in the embedded set and rasterize.
///
/// Breakage: renaming or deleting an SVG leaves the resolver pointing at a stem that silently
/// resolves to no icon at runtime, which reads as "this exchange has no logo" instead of a bug.
#[test]
fn every_resolvable_brand_ships_a_rasterizable_file() {
    for (_, slug) in BRANDS {
        assert!(
            EMBEDDED.get_file(format!("{slug}.svg")).is_some(),
            "assets/exchanges/{slug}.svg is missing from the embedded set"
        );
        let texture = load(slug).unwrap_or_else(|| panic!("{slug}.svg must rasterize"));
        assert_eq!(
            texture.size(0).width.0 as u32,
            RASTER_PX,
            "{slug}.svg must rasterize to the shared square size"
        );
    }
}
