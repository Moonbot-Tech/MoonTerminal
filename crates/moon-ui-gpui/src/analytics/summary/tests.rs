//! Unit tests for the profit-unit formatting that follows the Analytics metric toggle.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute and make it expand recursively (CONTRIBUTING.md).
use super::{DeltaGood, delta_parts, fmt_signed_unit, pct_delta};
use crate::analytics::{pnl_unit_label, set_pnl_unit};
use moon_core::db::{ProfitUnit, QuoteCurrency};
use moon_ui::MoonPalette;
use rust_i18n::t;

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
