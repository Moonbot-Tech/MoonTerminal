use std::sync::{
    Arc, Barrier, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

use super::{BRANDS, EMBEDDED, RASTER_PX, load, logo_slug, prewarm_once};

/// Core-reported names carry a market type, a leading futures letter, or an old brand spelling, and
/// all three must land on the one shipped logo.
///
/// Breakage: dropping the trailing market-word strip splits `Binance Futures` off from `Binance`;
/// removing an explicit leading-F alias loses real futures exchange names.
#[test]
fn reported_exchange_names_resolve_to_one_brand() {
    for (name, slug) in [
        ("Binance", "binance"),
        ("Binance Futures", "binance"),
        ("FBinance", "binance"),
        ("Binance COIN-M", "binance"),
        ("FBinance USD-M Futures", "binance"),
        ("ByBit Spot", "bybit"),
        ("FByBit", "bybit"),
        ("Gate.io", "gate"),
        ("FGate", "gate"),
        ("OKEx", "okx"),
        ("OKX Futures", "okx"),
        ("FOKX", "okx"),
        ("Huobi", "htx"),
        ("HTX Spot", "htx"),
        ("Hyperliquid", "hyperliquid"),
        ("FHyper", "hyperliquid"),
        ("Bitget Futures", "bitget"),
        ("FBitget", "bitget"),
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
    for name in [
        "",
        "   ",
        "Kraken",
        "Spot",
        "Futures",
        "MEXC",
        "GateHub",
        "HyperTrade",
        "BinanceClone",
        "NotOKX",
        "MyByBit",
    ] {
        assert_eq!(logo_slug(name), None, "{name:?} must have no logo");
    }
}

/// Catches replacing `exchange_logos.rs:prewarm_once` with an ordinary cache check: concurrent
/// Shell entries could all begin filesystem reads and SVG decode before any one task fills it.
#[test]
fn concurrent_logo_prewarm_runs_one_blocking_initializer() {
    const CALLERS: usize = 8;
    let gate = Arc::new(OnceLock::new());
    let start = Arc::new(Barrier::new(CALLERS));
    let calls = Arc::new(AtomicUsize::new(0));
    let workers = (0..CALLERS)
        .map(|_| {
            let gate = gate.clone();
            let start = start.clone();
            let calls = calls.clone();
            std::thread::spawn(move || {
                start.wait();
                prewarm_once(&gate, || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::thread::yield_now();
                });
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().expect("prewarm worker must not panic");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "all concurrent callers must share one blocking initializer"
    );
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
