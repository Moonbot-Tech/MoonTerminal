//! Synthetic regression and micro-benchmark coverage for asset cache signatures.

use super::super::tests::synthetic_servers;
use super::{
    AssetsRefresh, assets_observe_step, assets_refresh, missing_transfer_cores, sale_revisions_sig,
    total_modes_sig,
};
use moon_core::config::TotalMode;
use std::hint::black_box;
use std::time::Instant;

/// Restoring a wall-second retry bypasses the 250 ms floor and adds commands and repaints.
#[test]
fn assets_observer_step_preserves_admitted_gate_cadence() {
    let key = (17, 1.0f64.to_bits());
    let (action, stored, sale) = assets_observe_step(None, key, None, 1, true);
    assert_eq!(action, AssetsRefresh::Full); // Admitted key change at 2900 ms.
    assert_eq!(
        assets_observe_step(stored, key, sale, 1, false),
        (AssetsRefresh::None, stored, sale)
    ); // Rejected unchanged observation at 3000 ms.
    let (action, stored, sale) = assets_observe_step(stored, key, sale, 2, false);
    assert_eq!(action, AssetsRefresh::SaleOnly);
    assert_eq!(
        assets_observe_step(stored, key, sale, 3, true).0,
        AssetsRefresh::Full
    );
}

/// Disconnecting the tested step or restoring the wall-second due path must fail this edge check.
#[test]
fn assets_observer_uses_gate_and_applies_step() {
    let source = super::super::tests::strip_rust_comments(include_str!("../mod.rs"));
    let source: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(
        source
            .contains("cache::assets_observe_step(this.cache_sig,key,this.sale_sig,sale,gate_due)")
    );
    assert!(source.contains("this.cache_sig=cache_sig;this.sale_sig=sale_sig;matchrefresh{"));
    assert!(!source.contains("last_full_sec"));
}

/// Dropping a configured mode from invalidation leaves account totals stale after Settings edits.
#[test]
fn total_modes_sig_changes_when_any_mode_changes() {
    let mut servers = synthetic_servers();
    let baseline = total_modes_sig(&servers);
    for ix in 0..servers.len() {
        for mode in [TotalMode::Exclude, TotalMode::Separate] {
            servers[ix].total_mode = mode;
            assert_ne!(total_modes_sig(&servers), baseline, "configured core {ix}");
        }
        servers[ix].total_mode = TotalMode::Auto;
    }
    // Both duplicate entries participate, even though only the first can supply a balance mode.
    servers[199].id = servers[0].id;
    servers[199].total_mode = TotalMode::Exclude;
    let duplicate_baseline = total_modes_sig(&servers);
    servers[0].total_mode = TotalMode::Separate;
    assert_ne!(total_modes_sig(&servers), duplicate_baseline);
    servers[0].total_mode = TotalMode::Auto;
    servers[199].total_mode = TotalMode::Separate;
    assert_ne!(total_modes_sig(&servers), duplicate_baseline);
}

/// Measures configured-mode signature preparation on a synthetic 200-core cache-hit path.
#[test]
#[ignore]
fn bench_assets_modes_sig_200() {
    let servers = synthetic_servers();
    let iterations = 100_000;
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(total_modes_sig(black_box(&servers)));
    }
    println!(
        "bench_assets_modes_sig_200: {} ns/iter",
        started.elapsed().as_nanos() / iterations
    );
}

/// Full rebuilding on order-only changes wastes rows and totals work; missing SaleOnly loses badges.
#[test]
fn order_only_change_refreshes_sale_marks_without_full_rebuild() {
    let key = (17, 1.0f64.to_bits());
    let mut prev_sale = Some(0);
    let mut full = 0;
    let mut sale_only = 0;
    for sale in 1..=4 {
        match assets_refresh(Some(key), key, prev_sale, sale, false) {
            AssetsRefresh::Full => full += 1,
            AssetsRefresh::SaleOnly => sale_only += 1,
            AssetsRefresh::None => panic!("an order revision must refresh its sale markers"),
        }
        prev_sale = Some(sale);
    }
    assert_eq!((full, sale_only), (0, 4));
    assert_eq!(
        assets_refresh(Some(key), key, Some(4), 4, false),
        AssetsRefresh::None
    );
    println!(
        "order_only_change_refreshes_sale_marks_without_full_rebuild: {full} Full, {sale_only} SaleOnly"
    );
}

/// Checking sale changes first would starve missing-transfer retries during a continuous order feed.
#[test]
fn continuous_orders_keep_periodic_full_refresh_and_transfer_retry() {
    let key = (17, 1.0f64.to_bits());
    assert_eq!(
        assets_refresh(Some(key), key, Some(1), 2, true),
        AssetsRefresh::Full,
        "sale_changed && due must still retry transfers"
    );
    assert_eq!(
        assets_refresh(Some(key), (18, key.1), Some(1), 2, false),
        AssetsRefresh::Full,
        "full-key changes must take priority over sale changes"
    );
    assert_eq!(
        assets_refresh(Some(key), (key.0, 2.0f64.to_bits()), Some(1), 1, false),
        AssetsRefresh::Full,
        "the dust threshold must still invalidate rows"
    );
    let mut prev_key = None;
    let mut prev_sale = None;
    let mut last_admitted_tick = None;
    let mut full_per_second = [0u32; 10];
    let mut requests = [0; 200];
    // Keep the full key fixed while varying order revisions twenty times per second.
    for tick in 0..200 {
        let now_sec = tick / 20;
        let sale = tick + 1;
        // Each revision changes the gate signature, but the monotonic floor admits only 4 Hz.
        let due = last_admitted_tick.is_none_or(|last| tick - last >= 5);
        if due {
            last_admitted_tick = Some(tick);
        }
        let (refresh, next_key, next_sale) =
            assets_observe_step(prev_key, key, prev_sale, sale, due);
        match refresh {
            AssetsRefresh::Full => {
                full_per_second[now_sec as usize] += 1;
                // Synthetic revisions model core 1 loading at second 5 and core 2 already loaded.
                let revisions = (1..=200).map(|id| {
                    let loaded = id == 2 || (id == 1 && now_sec >= 5);
                    (id, u64::from(loaded))
                });
                for id in missing_transfer_cores(revisions) {
                    requests[id as usize - 1] += 1;
                }
            }
            AssetsRefresh::SaleOnly => {}
            AssetsRefresh::None => panic!("each tick changes the order revision"),
        }
        prev_key = next_key;
        prev_sale = next_sale;
    }
    assert_eq!(full_per_second, [4; 10]);
    assert_eq!(requests[0], 20, "stop retrying after the first snapshot");
    assert_eq!(requests[1], 0, "a loaded empty wallet must not be retried");
    assert!(requests[2..].iter().all(|count| *count == 40));
    println!(
        "continuous_orders_keep_periodic_full_refresh_and_transfer_retry: {} Full in 10 s",
        full_per_second.iter().sum::<u32>()
    );
}

/// Ignoring a second core or an order revision would strand its sale badge until a full rebuild.
#[test]
fn sale_signature_tracks_scoped_order_revisions_and_core_order() {
    let baseline = sale_revisions_sig([(1, 5), (2, 9)].into_iter());
    for changed in [[(1, 6), (2, 9)], [(1, 5), (2, 10)], [(2, 9), (1, 5)]] {
        assert_ne!(sale_revisions_sig(changed.into_iter()), baseline);
    }
}
