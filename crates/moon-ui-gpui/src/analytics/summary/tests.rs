//! Unit tests for the profit-unit formatting that follows the Analytics metric toggle.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute and make it expand recursively (CONTRIBUTING.md).
use super::{
    DeltaGood, delta_parts, fmt_signed_unit, pct_delta, short_id, strat_display, strat_display_ex,
};
use crate::analytics::{pnl_unit_label, set_pnl_unit};
use moon_core::db::{ProfitUnit, QuoteCurrency};
use moon_ui::MoonPalette;
use rust_i18n::t;

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
pub(super) fn old_cumulative(
    data: &moon_core::db::analytics::Summary,
) -> (Vec<f64>, Vec<f32>, Vec<usize>, Vec<Vec<f32>>, f32, f32) {
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
    let mut order: Vec<_> = (0..data.core_days.len()).collect();
    order.sort_by(|&a, &b| {
        data.core_days[b]
            .total
            .abs()
            .total_cmp(&data.core_days[a].total.abs())
    });
    order.truncate(super::cumulative::MAX_CORE_LINES);
    let curves: Vec<Vec<f32>> = order
        .iter()
        .map(|&ci| {
            let mut c = 0.0f32;
            data.core_days[ci]
                .per_bucket
                .iter()
                .take(data.days.len())
                .map(|v| {
                    c += fin(*v) as f32;
                    c
                })
                .collect()
        })
        .collect();
    let mut vmax = pts.iter().copied().fold(0.0f32, f32::max);
    let mut vmin = pts.iter().copied().fold(0.0f32, f32::min);
    for c in &curves {
        for &v in c {
            vmax = vmax.max(v);
            vmin = vmin.min(v);
        }
    }
    (cum, pts, order, curves, vmin.min(0.0), vmax.max(1e-6))
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

/// End to end through the real locale template: the insight sentence must lose the stray "USDT" in
/// percent mode and keep it in money mode. Language-agnostic — "USDT" and "%" are neutral tokens in
/// every locale, so the assertions hold whatever language is active.
#[test]
fn insight_sentence_unit_follows_the_metric() {
    let _locale = crate::test_locale::force("en");
    let render = || {
        t!(
            "analytics.ins.best_strategy",
            name = "S",
            profit = fmt_signed_unit(15.34),
            wr = "76.1"
        )
        .to_string()
    };
    set_pnl_unit(Some(ProfitUnit::Percent));
    let pct = render();
    assert!(
        !pct.contains("USDT"),
        "percent mode still shows USDT: {pct}"
    );
    set_pnl_unit(Some(ProfitUnit::Quote(
        QuoteCurrency::from_report_ordinal(1).expect("USDT report ordinal"),
    )));
    let usdt = render();
    assert!(usdt.contains("USDT"), "usdt mode lost the unit: {usdt}");
}

/// Swapping the positive and negative palette arguments in `DeltaGood::tone`'s `Down` arm makes
/// a deeper maximum drawdown render green, telling the user that a worsening period improved.
#[test]
fn kpi_delta_tones_follow_each_metrics_good_direction() {
    let p = MoonPalette::LIGHT;
    let cases = [
        (
            "deeper max drawdown",
            DeltaGood::Down,
            2690.0,
            7228.21,
            p.orange,
        ),
        (
            "shallower max drawdown",
            DeltaGood::Down,
            7228.21,
            2690.0,
            p.green,
        ),
        (
            "longer duration",
            DeltaGood::Neither,
            30.0,
            60.0,
            p.text_soft,
        ),
        (
            "shorter duration",
            DeltaGood::Neither,
            60.0,
            30.0,
            p.text_soft,
        ),
        ("improving up metric", DeltaGood::Up, 100.0, 150.0, p.green),
        ("worsening up metric", DeltaGood::Up, 150.0, 100.0, p.orange),
    ];

    for (name, good, prev, cur, expected_tone) in cases {
        let expected_arrow = if cur > prev { "▲" } else { "▼" };
        let (text, tone) = delta_parts(pct_delta(cur, Some(prev)), good, p)
            .unwrap_or_else(|| panic!("{name} must have a visible delta"));
        assert!(
            text.starts_with(expected_arrow),
            "{name} must point {expected_arrow}: {text}"
        );
        assert_eq!(
            tone, expected_tone,
            "{name} must use its independently chosen palette token"
        );
    }

    assert_eq!(
        delta_parts(Some(0.05), DeltaGood::Up, p),
        Some(("▲ 0.1%".to_string(), p.green)),
        "a 0.05% increase rounds to the visible 0.1% boundary"
    );
    assert_eq!(
        pct_delta(100.0, None),
        None,
        "no previous period has no delta"
    );
    assert_eq!(
        pct_delta(100.0, Some(0.0)),
        None,
        "a zero previous period has no meaningful percentage"
    );
    assert_eq!(
        delta_parts(pct_delta(100.02, Some(100.0)), DeltaGood::Up, p),
        None,
        "a delta that rounds away leaves the tile's muted em dash"
    );
}

/// `summary/mod.rs:strat_display_ex` must resolve manual orders before the unresolved-id branch,
/// and `short_id` must expose exactly the final six decimal digits only when the full id exceeds
/// eight characters. Reordering those rules labels manual orders as deleted and makes a tooltip's
/// full identity disagree with the muted Summary label a user can see.
#[test]
fn unresolved_strategy_labels_keep_their_status_and_readable_id_tail() {
    // Held, not merely set: the locale is process-wide and other tests in this binary switch it
    // while this one runs. See `crate::test_locale`.
    let _locale = crate::test_locale::force("en");
    let id = "-7653179346322682234";

    let deleted = strat_display_ex(id, id, true, Some(0));
    assert_eq!(deleted.text, "deleted strategy #…682234");
    assert!(deleted.muted);
    assert_eq!(deleted.full_id.as_deref(), Some(id));

    for alive in [Some(2), None] {
        let unnamed = strat_display_ex(id, id, true, alive);
        assert_eq!(unnamed.text, "unnamed strategy #…682234");
        assert!(unnamed.muted);
        assert_eq!(unnamed.full_id.as_deref(), Some(id));
    }

    let manual = strat_display_ex("0", "0", true, Some(0));
    assert_eq!(manual.text, "Manual (no strategy)");
    assert!(!manual.muted);
    assert_eq!(manual.full_id, None);

    let named = strat_display_ex("Seven", "7", false, Some(0));
    assert_eq!(named.text, "Seven");
    assert!(!named.muted);
    assert_eq!(named.full_id, None);
    assert_eq!(strat_display("42"), "42");
    assert_eq!(short_id(id), "…682234");
    assert_eq!(short_id("odd-id"), "odd-id");
    assert_eq!(short_id("12345678"), "12345678");
    assert_eq!(short_id("123456789"), "…456789");
}
