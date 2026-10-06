//! Equality and invalidation proofs for the snapshot-owned Summary cache.

use super::{DailyLabels, SummaryDerived, configured_colors, ensure_derived as refresh_derived};
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
            cached.order.clone(),
            cached.curves.clone(),
            cached.vmin,
            cached.vmax,
            cached.swings.clone(),
        ));
    }
    println!(
        "bench_cumulative_derive_400x200 after_us_per_iter={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// Reuse daily strings and width-keyed thinning; GUI font measurement remains outside timing.
#[test]
#[ignore]
fn bench_daily_labels_400() {
    let data = synthetic_summary();
    let mut daily = DailyLabels::new(&data.days);
    daily.ensure_texts(crate::analytics::pnl_suffix());
    daily.ensure_labelled(48.0);
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(daily.ensure_texts(crate::analytics::pnl_suffix()));
        std::hint::black_box((
            daily.texts.clone(),
            daily.ensure_labelled(std::hint::black_box(48.0)),
        ));
    }
    println!(
        "bench_daily_labels_400 after_us_per_iter={:.3}",
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
fn cumulative_cache_is_bit_equal_to_original_with_nan_and_absolute_ties() {
    let mut data = synthetic_summary();
    data.days[3].profit = f64::NAN;
    data.days[19].profit = f64::INFINITY;
    data.core_days[0].per_bucket[8] = f64::NAN;
    data.core_days[1].per_bucket[9] = f64::NEG_INFINITY;
    data.core_days[0].total = 1_000_000.0;
    data.core_days[1].total = -1_000_000.0;
    data.core_days[2].total = f64::NAN;
    let (cum, pts, order, curves, vmin, vmax) = old_cumulative(&data);
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
    assert_eq!(cached.order.as_ref(), order);
    assert_eq!(cached.curves.len(), curves.len());
    for (actual, expected) in cached.curves.iter().zip(&curves) {
        assert_eq!(
            actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }
    assert_eq!(cached.vmin.to_bits(), vmin.to_bits());
    assert_eq!(cached.vmax.to_bits(), vmax.to_bits());
    assert_eq!(
        cached.swings.as_ref(),
        crate::analytics::summary::cumulative::swing_labels(&pts)
    );
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
    let curves = cached.as_ref().unwrap().curves.clone();
    let order = cached.as_ref().unwrap().order.clone();
    servers[0].1 = [250, 0, 0];
    assert!(ensure_derived(
        &mut cached,
        &data,
        servers.iter().copied(),
        MoonPalette::LIGHT
    ));
    assert!(Rc::ptr_eq(&curves, &cached.as_ref().unwrap().curves));
    assert!(Rc::ptr_eq(&order, &cached.as_ref().unwrap().order));
    let replacement = Arc::new((*data).clone());
    assert!(ensure_derived(
        &mut cached,
        &replacement,
        servers.iter().copied(),
        MoonPalette::LIGHT
    ));
    assert!(!Rc::ptr_eq(&curves, &cached.as_ref().unwrap().curves));
}

/// Omitting suffix invalidation mislabels percent bars; every render must still measure once.
#[test]
fn daily_texts_match_original_formats_and_measure_cached_widest_each_render() {
    let data = synthetic_summary();
    let mut daily = DailyLabels::new(&data.days);
    let mut text_derivations = 0;
    for suffix in ["", "", "%", "%"] {
        text_derivations += usize::from(daily.ensure_texts(suffix));
        let expected: Vec<_> = data
            .days
            .iter()
            .map(|d| format!("{}{}", moon_core::util::fmt::compact(d.profit, 0), suffix))
            .collect();
        assert_eq!(daily.texts.as_ref(), expected);
        let expected_widest = expected
            .iter()
            .max_by_key(|text| text.chars().count())
            .unwrap();
        let calls = std::cell::Cell::new(0);
        assert_eq!(
            daily.label_w(|text| {
                calls.set(calls.get() + 1);
                assert_eq!(text, expected_widest);
                text.chars().count() as f32 * 8.0
            }),
            expected_widest.chars().count() as f32 * 8.0
        );
        assert_eq!(calls.get(), 1);
    }
    assert_eq!(text_derivations, 2);
    let empty = DailyLabels::new(&[]);
    assert_eq!(
        empty.label_w(|_| panic!("empty data must measure no label")),
        0.0
    );
}

/// Reusing thinning across a width change lets labels collide after a font or theme change.
#[test]
fn daily_thinning_matches_old_algorithm_and_recomputes_only_for_width_or_snapshot() {
    let data = Arc::new(synthetic_summary());
    let mut cached = None;
    ensure_derived(
        &mut cached,
        &data,
        synthetic_servers().into_iter(),
        MoonPalette::LIGHT,
    );
    let daily = &cached.as_ref().unwrap().daily;
    let profits: Vec<_> = data.days.iter().map(|d| d.profit).collect();
    let mut count = 0;
    let mut first = None;
    for width in [48.0, 48.0, 72.0, 72.0] {
        let (actual, changed) = daily.ensure_labelled(width);
        count += usize::from(changed);
        let expected: std::collections::BTreeSet<_> =
            crate::analytics::summary::charts::thinned_labels(
                &profits,
                crate::analytics::summary::charts::PLOT_W_NOMINAL,
                width,
            )
            .into_iter()
            .collect();
        assert_eq!(*actual, expected);
        if width == 48.0 {
            if let Some(previous) = &first {
                assert!(Rc::ptr_eq(previous, &actual));
            }
            first = Some(actual);
        }
    }
    assert_eq!(count, 2);
    let replacement = Arc::new((*data).clone());
    ensure_derived(
        &mut cached,
        &replacement,
        synthetic_servers().into_iter(),
        MoonPalette::LIGHT,
    );
    assert!(
        cached.as_ref().unwrap().daily.ensure_labelled(72.0).1,
        "new Arc must derive even at the same width"
    );
}
