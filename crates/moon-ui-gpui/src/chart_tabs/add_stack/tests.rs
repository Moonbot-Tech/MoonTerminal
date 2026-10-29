//! Synthetic measurements of render-order ownership costs.

use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

/// Measures the cost of copying chart slots versus sharing them across render closures.
#[test]
#[ignore]
fn render_order_share_bench() {
    let started = Instant::now();
    for _ in 0..100_000 {
        let order: Vec<usize> = black_box((0..64).collect());
        black_box(order.clone());
        black_box(order.clone());
        black_box(order.clone());
        black_box(order);
    }
    let vec_elapsed = started.elapsed();

    let started = Instant::now();
    for _ in 0..100_000 {
        let render_order: Vec<usize> = black_box((0..64).collect());
        let order: Rc<[usize]> = render_order.into();
        black_box(order.clone());
        black_box(order.clone());
        black_box(order.clone());
        black_box(order);
    }
    let rc_elapsed = started.elapsed();
    println!(
        "render_order_share_bench default test profile: slots=64 iterations=100000 vec_us={} rc_us={}",
        vec_elapsed.as_micros(),
        rc_elapsed.as_micros()
    );
}

/// Measure the two linear lookup shapes at the synthetic 50-tab startup load.
#[test]
#[ignore = "synthetic startup chart restore scale probe"]
fn probe_startup_chart_restore_lookups() {
    const TABS: usize = 50;
    const CHARTS_PER_TAB: usize = 20;
    let charts: Vec<(u64, String)> = (0..CHARTS_PER_TAB)
        .map(|index| (index as u64 % 10 + 1, format!("COIN_A{index}USDT")))
        .collect();
    let specs: Vec<(u32, u64)> = (0..TABS).map(|index| (index as u32 + 1, 1)).collect();
    let mut custom_ms = Vec::new();
    let mut detached_ms = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        for _ in 0..TABS {
            for (core, market) in &charts {
                black_box(
                    charts
                        .iter()
                        .position(|entry| entry.0 == *core && entry.1 == *market),
                );
            }
        }
        custom_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        let started = Instant::now();
        for (num, bucket) in &specs {
            black_box(specs.iter().position(|spec| spec == &(*num, *bucket)));
        }
        detached_ms.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    custom_ms.sort_by(f64::total_cmp);
    detached_ms.sort_by(f64::total_cmp);
    println!(
        "probe_startup_chart_restore tabs=50 charts_per_tab=20 custom_linear_ms={custom_ms:?} detached_linear_ms={detached_ms:?} custom_median_ms={:.6} detached_median_ms={:.6}",
        custom_ms[2], detached_ms[2]
    );
}
