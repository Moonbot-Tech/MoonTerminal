//! Synthetic equivalence tests and micro-benchmarks for asset row collection.

use super::super::tests::synthetic_servers;
use super::{TotalModeIndex, market_row_visible, market_rows};
use moon_core::config::TotalMode;
use moon_core::feed::AssetRow;
use std::hint::black_box;
use std::time::Instant;

/// Builds a complete made-up market row without a session or production data.
fn row(ix: usize) -> AssetRow {
    AssetRow {
        market: format!("COIN{ix}USDT"),
        coin: format!("COIN{ix}"),
        quote: "USDT".into(),
        listed: 1,
        qty: 1.0,
        qty_full: 1.0,
        price: 1.0,
        value_usdt: if ix.is_multiple_of(10) { 5.0 } else { 0.1 },
        min_lot_usd: 0.0,
        is_quote_asset: false,
        mark_price: 0.0,
        pos_size: 0.0,
        pos_price: 0.0,
        liq_price: 0.0,
        leverage: 0,
        pnl_usdt: 0.0,
        pnl_live: false,
    }
}

/// Frozen clone-first visibility oracle copied from the pre-extraction collector.
fn reference_visible(row: &AssetRow, futures: bool, threshold: f64) -> bool {
    let row = row.clone();
    let value = row.value_usdt;
    let min_lot = if row.min_lot_usd > 0.0 {
        row.min_lot_usd
    } else {
        1.0
    };
    let is_position = row.pos_size != 0.0 && row.pos_size.abs() * row.price >= min_lot;
    let spot_coin_visible = !futures && !row.is_quote_asset && value >= threshold;
    threshold <= 0.0 || is_position || spot_coin_visible
}

/// Changing inclusive lot/dust bounds or NaN handling would hide or add holdings and positions.
#[test]
fn asset_visibility_predicate_matches_clone_first_path() {
    for futures in [false, true] {
        for quote in [false, true] {
            for threshold in [-1.0, 0.0, 1.0, 5.0, f64::NAN] {
                for minimum in [-1.0, 0.0, 1.0, 5.0, f64::NAN] {
                    for position in [-5.0, 0.0, 0.5, 1.0, f64::NAN] {
                        for price in [-1.0, 0.0, 1.0, f64::INFINITY, f64::NAN] {
                            for value in [0.0, 0.5, 1.0, 5.0, f64::NAN] {
                                let mut row = row(0);
                                row.is_quote_asset = quote;
                                row.min_lot_usd = minimum;
                                row.pos_size = position;
                                row.price = price;
                                row.value_usdt = value;
                                assert_eq!(
                                    market_row_visible(&row, futures, threshold),
                                    reference_visible(&row, futures, threshold),
                                    "futures={futures}, threshold={threshold}, row={row:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    let rows: Vec<_> = (0..50).map(row).collect();
    for futures in [false, true] {
        for threshold in [0.0, 1.0, 5.0] {
            let expected: Vec<_> = rows
                .iter()
                .filter(|row| reference_visible(row, futures, threshold))
                .cloned()
                .collect();
            let actual: Vec<_> = market_rows(&rows, futures, threshold).collect();
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        }
    }
}

/// Measures market-row ownership for 200 synthetic cores with 50 rows each, 90 percent dust.
#[test]
#[ignore]
fn bench_assets_collect_200_cores_dust_heavy() {
    let cores: Vec<Vec<AssetRow>> = (0..200)
        .map(|core| (0..50).map(|ix| row(core * 50 + ix)).collect())
        .collect();
    let iterations = 1_000;
    let started = Instant::now();
    for _ in 0..iterations {
        let kept: Vec<_> = black_box(&cores)
            .iter()
            .flat_map(|rows| market_rows(rows, false, 1.0))
            .collect();
        assert_eq!(kept.len(), 1_000);
        black_box(kept);
    }
    println!(
        "bench_assets_collect_200_cores_dust_heavy: {} ns/iter",
        started.elapsed().as_nanos() / iterations
    );
}

/// Last-id insertion or skipping absent ids would change the configured contribution to totals.
#[test]
fn total_mode_index_agrees_with_core_total_mode() {
    let mut servers = synthetic_servers();
    for (ix, server) in servers.iter_mut().enumerate() {
        server.total_mode = TotalMode::ALL[ix % TotalMode::ALL.len()];
    }
    // Keep 200 configured entries while introducing duplicate ids with conflicting modes.
    servers[150].id = 1;
    servers[150].total_mode = TotalMode::Exclude;
    servers[199].id = 2;
    servers[199].total_mode = TotalMode::Separate;
    let modes = TotalModeIndex::new(&servers);
    for core in 0..=201 {
        assert_eq!(
            modes.get(core),
            moon_core::session::balances::core_total_mode(&servers, core),
            "core {core}"
        );
    }
    assert_eq!(modes.get(1), TotalMode::Auto);
    assert_eq!(modes.get(2), TotalMode::Exclude);
    assert_eq!(modes.get(151), TotalMode::Auto);
    assert_eq!(TotalModeIndex::new(&[]).get(1), TotalMode::Auto);
}

/// Measures mode preparation plus 200 lookups, including the per-rebuild construction cost.
#[test]
#[ignore]
fn bench_assets_per_core_modes_200() {
    let mut servers = synthetic_servers();
    for (ix, server) in servers.iter_mut().enumerate() {
        server.total_mode = TotalMode::ALL[ix % TotalMode::ALL.len()];
    }
    let iterations = 100_000;
    let started = Instant::now();
    for _ in 0..iterations {
        let modes = TotalModeIndex::new(black_box(&servers));
        let mut signature = 0u64;
        for core in 1..=200 {
            signature = signature
                .wrapping_mul(31)
                .wrapping_add(modes.get(black_box(core)) as u64);
        }
        black_box(signature);
    }
    println!(
        "bench_assets_per_core_modes_200: {} ns/iter",
        started.elapsed().as_nanos() / iterations
    );
}
