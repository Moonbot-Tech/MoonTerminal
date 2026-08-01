//! Unit tests for the profit-unit formatting that follows the Analytics metric toggle.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute and make it expand recursively (CONTRIBUTING.md).
use super::fmt_signed_unit;
use crate::analytics::{pnl_unit_label, set_pnl_unit};
use moon_core::db::{ProfitUnit, QuoteCurrency};
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
