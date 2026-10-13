//! Unit tests for the profit-unit formatting that follows the Analytics metric toggle.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute and make it expand recursively (CONTRIBUTING.md).
use super::fmt_signed_unit;
use crate::analytics::{pnl_unit_label, set_pnl_unit};
use moon_core::db::{ProfitUnit, QuoteCurrency};
use moon_ui::MoonPalette;

/// Synthetic large Summary shared by the baseline benches and independent cache oracles.
pub(super) fn synthetic_summary() -> moon_core::db::analytics::Summary {
    use moon_core::db::analytics::{CoreSeries, DayPoint, Summary};
    let core_days: Vec<_> = (0..200)
        .map(|ci| {
            let per_bucket: Vec<_> = (0..400)
                .map(|bi| ((ci * 17 + bi * 13) % 97) as f64 - 48.0)
                .collect();
            CoreSeries {
                uid: ((ci * 73) % 200) as u64,
                name: format!("core-{ci}"),
                total: per_bucket.iter().sum(),
                per_bucket,
                per_bucket_trades: vec![1; 400],
                trades: 400,
                ..Default::default()
            }
        })
        .collect();
    Summary {
        days: (0..400)
            .map(|bi| DayPoint {
                start: bi as i64 * 86_400,
                profit: core_days.iter().map(|c| c.per_bucket[bi]).sum(),
                trades: 200,
            })
            .collect(),
        core_days,
        ..Summary::default()
    }
}

/// Configured server ids and RGBs, with missing cores and deliberately repeated colours.
pub(super) fn synthetic_servers() -> Vec<(u64, [u8; 3])> {
    (0..200)
        .map(|i| (i + 10, [((i % 7) * 30) as u8, 80, 160]))
        .collect()
}

/// Original linear first-match colour lookup, retained as an independent equality oracle.
pub(super) fn old_colors(
    data: &moon_core::db::analytics::Summary,
    servers: &[(u64, [u8; 3])],
) -> Vec<gpui::Hsla> {
    let configured: Vec<_> = data
        .core_days
        .iter()
        .map(|c| (c.uid, servers.iter().find(|s| s.0 == c.uid).map(|s| s.1)))
        .collect();
    super::charts::distinct_core_colors(&configured, MoonPalette::LIGHT)
}

/// Original cumulative derivation, kept independent of the cache to pin float fold order.
///
/// Returns:
///     The f64 running total, its f32 copy, and the f32 running curve of every core in snapshot
///     order.
pub(super) fn old_cumulative(
    data: &moon_core::db::analytics::Summary,
) -> (Vec<f64>, Vec<f32>, Vec<Vec<f32>>) {
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    let mut acc = 0.0f64;
    let cum: Vec<_> = data
        .days
        .iter()
        .map(|d| {
            acc += fin(d.profit);
            acc
        })
        .collect();
    let pts: Vec<_> = cum.iter().map(|&v| v as f32).collect();
    let curves: Vec<Vec<f32>> = data
        .core_days
        .iter()
        .map(|core| {
            let mut c = 0.0f32;
            core.per_bucket
                .iter()
                .take(data.days.len())
                .map(|v| {
                    c += fin(*v) as f32;
                    c
                })
                .collect()
        })
        .collect();
    (cum, pts, curves)
}

/// Baseline first-match lookup and distinct colour allocation for 200 cores/configured servers.
#[test]
#[ignore]
fn bench_summary_colors_200_cores() {
    let data = synthetic_summary();
    let servers = synthetic_servers();
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(old_colors(std::hint::black_box(&data), &servers));
    }
    println!(
        "bench_summary_colors_200_cores before_us_per_iter={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// Baseline running totals, stable line ordering and shared float range for 400 buckets/200 cores.
#[test]
#[ignore]
fn bench_cumulative_derive_400x200() {
    let data = synthetic_summary();
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(old_cumulative(std::hint::black_box(&data)));
    }
    println!(
        "bench_cumulative_derive_400x200 before_us_per_iter={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// The unit word must track both the active metric and exact persisted quote currency.
///
/// Hard-coding the historical USDT default in `summary::fmt_signed_unit` makes the USDC assertion
/// fail and mislabels every non-USDT insight sentence.
#[test]
fn unit_word_follows_the_metric() {
    set_pnl_unit(Some(ProfitUnit::Percent));
    assert_eq!(pnl_unit_label(), "%");
    let s = fmt_signed_unit(15.34);
    assert!(s.ends_with('%') && !s.contains("USDT"), "percent mode: {s}");

    let usdc = QuoteCurrency::from_report_ordinal(8).expect("USDC report ordinal");
    set_pnl_unit(Some(ProfitUnit::Quote(usdc)));
    assert_eq!(pnl_unit_label(), "USDC");
    let s = fmt_signed_unit(15.34);
    assert!(s.ends_with(" USDC") && !s.contains('%'), "USDC mode: {s}");
}

/// A non-finite figure stays a bare em dash and is never given a unit, in either mode.
#[test]
fn non_finite_stays_a_bare_em_dash() {
    set_pnl_unit(Some(ProfitUnit::Percent));
    assert_eq!(fmt_signed_unit(f64::NAN), "—");
    set_pnl_unit(Some(ProfitUnit::Quote(
        QuoteCurrency::from_report_ordinal(1).expect("USDT report ordinal"),
    )));
    assert_eq!(fmt_signed_unit(f64::INFINITY), "—");
}
