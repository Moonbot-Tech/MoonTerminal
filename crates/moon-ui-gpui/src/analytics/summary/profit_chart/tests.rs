//! Model rules of the profit card: which cores it draws, its y-range, its popup rows, the
//! shared hover mapping and the axis ticks.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::{ProfitChartModel, hover_bucket_at, popup_model, tick_indices};
use crate::analytics::summary::derived::derive_for_test;
use crate::analytics::summary::state::{CoreLines, SummaryUi};
use moon_core::db::analytics::{CoreSeries, DayPoint, Summary};
use moon_ui::MoonPalette;

/// Independent old layout oracle: last-painted half-open column owns the pointer.
fn old_hover_bucket(frac: f32, n: usize) -> Option<usize> {
    let step = 1.0 / n.saturating_sub(1).max(1) as f32;
    (0..n).rev().find(|&bi| {
        let centre = bi as f32 * step;
        let left = (centre - step / 2.0).max(0.0);
        let right = (centre + step / 2.0).min(1.0);
        left <= frac && frac < right
    })
}

/// Ideal-grid rounding loses f32 edges and the one-bucket gap, shifting popup identity.
#[test]
fn single_hover_matches_all_old_columns_and_shared_edges() {
    for n in 1usize..=500 {
        let step = 1.0 / n.saturating_sub(1).max(1) as f32;
        for bi in 0..n {
            let centre = bi as f32 * step;
            let left = (centre - step / 2.0).max(0.0);
            let right = (centre + step / 2.0).min(1.0);
            for sample in 0..=8 {
                let frac = left + (right - left) * sample as f32 / 8.0;
                assert_eq!(
                    hover_bucket_at(frac, n),
                    old_hover_bucket(frac, n),
                    "n={n} bi={bi} frac={frac:?}"
                );
            }
            assert_eq!(hover_bucket_at(left, n), old_hover_bucket(left, n));
            assert_eq!(hover_bucket_at(right, n), old_hover_bucket(right, n));
            if bi + 1 < n {
                let next_left = ((bi + 1) as f32 * step - step / 2.0).max(0.0);
                if next_left == right {
                    assert_eq!(hover_bucket_at(right, n), Some(bi + 1));
                }
            }
        }
    }
    assert_eq!(hover_bucket_at(0.5, 1), None);
    assert_eq!(hover_bucket_at(0.5001, 1), None);
    assert_eq!(hover_bucket_at(1.0, 1), None);
    assert_eq!(hover_bucket_at(0.0, 0), None);
}

/// Twenty synthetic cores over ten days; core `i` earns `i - 8` per day, so core 19 leads and
/// core 0 loses the most.
fn summary() -> Arc<Summary> {
    let days = 10;
    let core_days: Vec<CoreSeries> = (0..20u64)
        .map(|i| {
            let v = i as f64 - 8.0;
            CoreSeries {
                uid: 100 + i,
                name: format!("XALPHA-{i:02}"),
                per_bucket: vec![v; days],
                per_bucket_trades: vec![1; days],
                total: v * days as f64,
                trades: days as i64,
                ..Default::default()
            }
        })
        .collect();
    let per_day: f64 = core_days.iter().map(|c| c.per_bucket[0]).sum();
    Arc::new(Summary {
        bucket_secs: 86_400,
        days: (0..days)
            .map(|d| DayPoint {
                start: 1_800_000_000 + d as i64 * 86_400,
                profit: per_day,
                trades: 20,
            })
            .collect(),
        core_days,
        ..Default::default()
    })
}

/// The model draws exactly `shown_cores` for each mode and takes its range from `chart_range`
/// over those cores only (J1, F3).
#[test]
fn model_draws_the_shown_cores_with_their_own_range() {
    let _locale = crate::test_locale::force("en");
    let data = summary();
    let derived = derive_for_test(data.clone(), MoonPalette::LIGHT);
    let picked: BTreeSet<u64> = [100, 101].into();
    // Hand-computed from the fixture: the total earns 30 a day (sum of i - 8 over 20 cores), so
    // its curve tops out at 300; core 19 earns 11 a day (110 at most); core 0 loses 8 a day
    // (-80 at the end). Zero is always inside the range.
    for (lines, want, lo, hi) in [
        (CoreLines::None, vec![], 0.0f32, 300.0f32),
        (CoreLines::Top5, vec![19, 18, 17, 16, 15], 0.0, 300.0),
        (CoreLines::Selected, vec![0, 1], -80.0, 300.0),
    ] {
        let ui = SummaryUi {
            lines,
            picked: picked.clone(),
            ..Default::default()
        };
        let m = ProfitChartModel::new(&derived, &ui, chrono_tz::UTC, None, MoonPalette::LIGHT);
        let got: Vec<usize> = m.shown.iter().map(|c| c.ix).collect();
        assert_eq!(got, want, "{lines:?}");
        assert!(
            m.vmin <= lo && m.vmax >= hi,
            "{lines:?}: {} {}",
            m.vmin,
            m.vmax
        );
        assert!(
            m.vmax - m.vmin <= (hi - lo) * 1.2,
            "{lines:?}: padded range only"
        );
    }
    // Picking the biggest loser widens the range below what the top five alone need.
    let top = ProfitChartModel::new(
        &derived,
        &SummaryUi::default(),
        chrono_tz::UTC,
        None,
        MoonPalette::LIGHT,
    );
    let loser = ProfitChartModel::new(
        &derived,
        &SummaryUi {
            lines: CoreLines::Selected,
            picked: [100].into(),
            ..Default::default()
        },
        chrono_tz::UTC,
        None,
        MoonPalette::LIGHT,
    );
    assert!(loser.vmin < top.vmin);
}

/// The popup lists every shown core, with no row cap (J2): six picked, six rows.
#[test]
fn popup_rows_list_exactly_the_shown_cores() {
    let _locale = crate::test_locale::force("en");
    let data = summary();
    let derived = derive_for_test(data.clone(), MoonPalette::LIGHT);
    let picked: BTreeSet<u64> = [100, 103, 105, 110, 115, 119].into();
    let ui = SummaryUi {
        lines: CoreLines::Selected,
        picked,
        ..Default::default()
    };
    let m = ProfitChartModel::new(&derived, &ui, chrono_tz::UTC, None, MoonPalette::LIGHT);
    let pm = popup_model(&m, 3).expect("bucket 3");
    assert_eq!(pm.cores.len(), 6);
    let names: Vec<&str> = pm.cores.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "XALPHA-00",
            "XALPHA-03",
            "XALPHA-05",
            "XALPHA-10",
            "XALPHA-15",
            "XALPHA-19"
        ]
    );
    assert_eq!(pm.cores[0].value, -8.0);
    assert_eq!(pm.cum, data.days[0].profit * 4.0);
    assert!(popup_model(&m, 10).is_none());
}

/// Ticks thin to every k-th bucket, always end on the last, and never crowd it.
#[test]
fn axis_ticks_thin_and_keep_the_last() {
    assert!(tick_indices(0).is_empty());
    assert_eq!(tick_indices(1), [0]);
    assert_eq!(tick_indices(5), [0, 1, 2, 3, 4]);
    assert_eq!(tick_indices(30), [0, 5, 10, 15, 20, 25, 29]);
    let t = tick_indices(31);
    assert_eq!(t.last(), Some(&30));
    assert!(t.windows(2).all(|w| w[1] - w[0] >= 3), "{t:?}");
}
