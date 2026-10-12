//! Equality and invalidation proofs for the snapshot-owned Summary cache.

use super::{SummaryDerived, configured_colors, ensure_derived as refresh_derived};
use crate::analytics::summary::tests::{
    old_colors, old_cumulative, synthetic_servers, synthetic_summary,
};
use moon_ui::MoonPalette;
use std::rc::Rc;
use std::sync::Arc;

/// Observe production invalidation while dropping its borrowed entry before the next update.
fn ensure_derived(
    slot: &mut Option<Rc<SummaryDerived>>,
    data: &Arc<moon_core::db::analytics::Summary>,
    servers: impl Iterator<Item = (u64, [u8; 3])> + Clone,
    p: MoonPalette,
) -> bool {
    refresh_derived(slot, data.clone(), servers, p).1
}

/// Cache-hit colour path includes current server lookup and exact configured-key validation.
#[test]
#[ignore]
fn bench_summary_colors_200_cores() {
    let data = Arc::new(synthetic_summary());
    let servers = synthetic_servers();
    let mut cached = None;
    ensure_derived(
        &mut cached,
        &data,
        servers.iter().copied(),
        MoonPalette::LIGHT,
    );
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(ensure_derived(
            &mut cached,
            std::hint::black_box(&data),
            servers.iter().copied(),
            MoonPalette::LIGHT,
        ));
        std::hint::black_box(cached.as_ref().unwrap().colors.clone());
    }
    println!(
        "bench_summary_colors_200_cores after_us_per_iter={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// Reuse colourless curve storage after snapshot validation, measured separately from colour keys.
#[test]
#[ignore]
fn bench_cumulative_derive_400x200() {
    let data = Arc::new(synthetic_summary());
    let mut cached = None;
    ensure_derived(
        &mut cached,
        &data,
        synthetic_servers().into_iter(),
        MoonPalette::LIGHT,
    );
    let cached = cached.as_ref().unwrap();
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box((
            cached.cum.clone(),
            cached.pts.clone(),
            cached.core_curves.clone(),
            cached.peaks.clone(),
        ));
    }
    println!(
        "bench_cumulative_derive_400x200 after_us_per_iter={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// Replacing first-wins map insertion with overwrite changes duplicate-id colours on every chart.
#[test]
fn colours_equal_linear_first_match_with_duplicate_server_ids() {
    let data = Arc::new(synthetic_summary());
    let mut servers = synthetic_servers();
    servers.insert(0, (73, [240, 10, 20]));
    servers.push((73, [10, 240, 20]));
    let mut cached = None;
    assert!(ensure_derived(
        &mut cached,
        &data,
        servers.iter().copied(),
        MoonPalette::LIGHT
    ));
    assert_eq!(
        cached.as_ref().unwrap().colors.as_ref(),
        old_colors(&data, &servers)
    );
    let configured = configured_colors(&data, servers.iter().copied());
    assert_eq!(
        configured.iter().find(|c| c.0 == 73).unwrap().1,
        Some([240, 10, 20])
    );
    assert!(configured.iter().any(|c| c.1.is_none()));
    servers.last_mut().unwrap().1 = [90, 80, 70];
    assert!(!ensure_derived(
        &mut cached,
        &data,
        servers.iter().copied(),
        MoonPalette::LIGHT
    ));
    servers.retain(|(uid, _)| *uid != 73);
    assert!(ensure_derived(
        &mut cached,
        &data,
        servers.iter().copied(),
        MoonPalette::LIGHT
    ));
    assert_eq!(
        cached.as_ref().unwrap().colors.as_ref(),
        old_colors(&data, &servers)
    );
    servers.insert(0, (73, [240, 10, 20]));
    assert!(ensure_derived(
        &mut cached,
        &data,
        servers.iter().copied(),
        MoonPalette::LIGHT
    ));
    assert_eq!(
        cached.as_ref().unwrap().colors.as_ref(),
        old_colors(&data, &servers)
    );
}

/// Missing Arc or RGB invalidation reuses another period's colours; hover churn must reuse storage.
#[test]
fn colours_recompute_only_for_new_snapshot_or_changed_rgb() {
    let data = Arc::new(synthetic_summary());
    let mut servers = synthetic_servers();
    let mut cached = None;
    let mut count = 0;
    for _ in 0..2 {
        count += usize::from(ensure_derived(
            &mut cached,
            &data,
            servers.iter().copied(),
            MoonPalette::LIGHT,
        ));
    }
    assert_eq!(count, 1);
    let rendered = cached.as_ref().unwrap().clone();
    assert_eq!(Rc::strong_count(cached.as_ref().unwrap()), 2);
    drop(rendered);
    assert_eq!(Rc::strong_count(cached.as_ref().unwrap()), 1);
    let entry_pointer = Rc::as_ptr(cached.as_ref().unwrap());
    let colors = cached.as_ref().unwrap().colors.clone();
    for _ in 0..100 {
        assert!(!ensure_derived(
            &mut cached,
            &data,
            servers.iter().copied(),
            MoonPalette::LIGHT
        ));
    }
    assert!(Rc::ptr_eq(&colors, &cached.as_ref().unwrap().colors));
    assert_eq!(entry_pointer, Rc::as_ptr(cached.as_ref().unwrap()));
    let replacement = Arc::new((*data).clone());
    count += usize::from(ensure_derived(
        &mut cached,
        &replacement,
        servers.iter().copied(),
        MoonPalette::LIGHT,
    ));
    assert_eq!(count, 2);
    servers[0].1 = [255, 0, 0];
    count += usize::from(ensure_derived(
        &mut cached,
        &replacement,
        servers.iter().copied(),
        MoonPalette::LIGHT,
    ));
    assert_eq!(count, 3);
    assert_eq!(
        Arc::strong_count(&replacement),
        2,
        "cache must retain its snapshot against address reuse"
    );
}

/// Moving f32 accumulation into an f64 sum or changing stable tie order distorts drawn curves.
#[test]
fn cumulative_cache_is_bit_equal_to_original_with_nan() {
    let mut data = synthetic_summary();
    data.days[3].profit = f64::NAN;
    data.days[19].profit = f64::INFINITY;
    data.core_days[0].per_bucket[8] = f64::NAN;
    data.core_days[1].per_bucket[9] = f64::NEG_INFINITY;
    data.core_days[0].total = 1_000_000.0;
    data.core_days[1].total = -1_000_000.0;
    data.core_days[2].total = f64::NAN;
    let (cum, pts, curves) = old_cumulative(&data);
    let data = Arc::new(data);
    let mut cached = None;
    ensure_derived(
        &mut cached,
        &data,
        synthetic_servers().into_iter(),
        MoonPalette::LIGHT,
    );
    let cached = cached.unwrap();
    assert_eq!(
        cached.cum.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        cum.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(
        cached.pts.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        pts.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(cached.core_curves.len(), curves.len());
    for (actual, expected) in cached.core_curves.iter().zip(&curves) {
        assert_eq!(
            actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }
}

/// Hover redraws and RGB edits must keep the expensive snapshot-only curves and legend order.
#[test]
fn cumulative_hover_churn_derives_once_and_rgb_edit_keeps_curves() {
    let data = Arc::new(synthetic_summary());
    let mut servers = synthetic_servers();
    let mut cached = None;
    let mut count = 0;
    for _ in 0..100 {
        count += usize::from(ensure_derived(
            &mut cached,
            &data,
            servers.iter().copied(),
            MoonPalette::LIGHT,
        ));
    }
    assert_eq!(count, 1);
    let curves = cached.as_ref().unwrap().core_curves.clone();
    servers[0].1 = [250, 0, 0];
    assert!(ensure_derived(
        &mut cached,
        &data,
        servers.iter().copied(),
        MoonPalette::LIGHT
    ));
    assert!(Rc::ptr_eq(&curves, &cached.as_ref().unwrap().core_curves));
    let replacement = Arc::new((*data).clone());
    assert!(ensure_derived(
        &mut cached,
        &replacement,
        servers.iter().copied(),
        MoonPalette::LIGHT
    ));
    assert!(!Rc::ptr_eq(&curves, &cached.as_ref().unwrap().core_curves));
}

/// The drawdown marks sit on the deepest fall of a saw: trough at its bottom, peak at the running
/// high just before it; a rising curve has no marks.
#[test]
fn drawdown_marks_find_the_deepest_fall_of_a_saw() {
    let saw = [0.0f32, 5.0, 2.0, 8.0, 1.0, 6.0, 4.0];
    let (peaks, peak_ix, trough_ix) = super::drawdown_marks(&saw);
    assert_eq!(&*peaks, &[0.0, 5.0, 5.0, 8.0, 8.0, 8.0, 8.0]);
    assert_eq!((peak_ix, trough_ix), (Some(3), Some(4)));
    assert_eq!(super::drawdown_marks(&[0.0, 1.0, 2.0]).1, None);
}

/// Every core gets a cached curve (J1) and the
/// day extremes point at the largest and smallest buckets; hover churn derives nothing again.
#[test]
fn every_core_curve_is_cached_once() {
    let data = Arc::new(synthetic_summary());
    let servers = synthetic_servers();
    let mut cached = None;
    let mut derives = 0;
    for _ in 0..50 {
        derives += usize::from(ensure_derived(
            &mut cached,
            &data,
            servers.iter().copied(),
            MoonPalette::LIGHT,
        ));
    }
    assert_eq!(derives, 1);
    let d = cached.as_ref().unwrap();
    assert_eq!(d.core_curves.len(), data.core_days.len());
    let profits: Vec<f64> = data.days.iter().map(|day| day.profit).collect();
    let max = profits.iter().copied().fold(f64::MIN, f64::max);
    let min = profits.iter().copied().fold(f64::MAX, f64::min);
    assert_eq!(profits[d.day_max_ix.unwrap()], max);
    assert_eq!(profits[d.day_min_ix.unwrap()], min);
    assert_eq!(d.peaks.len(), d.pts.len());
}
