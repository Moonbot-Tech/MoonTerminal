//! Unit tests for the profit-unit formatting that follows the Analytics metric toggle.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute and make it expand recursively (CONTRIBUTING.md).
use super::fmt_signed_unit;
use crate::analytics::{pnl_unit_label, set_pnl_pct};
use rust_i18n::t;

/// The unit word must track the active metric: a bare "%" in percent mode (the number carries it),
/// the "USDT" word in money mode. This is the exact split the "+15.34% USDT" bug got wrong.
#[test]
fn unit_word_follows_the_metric() {
    set_pnl_pct(true);
    assert_eq!(pnl_unit_label(), "%");
    let s = fmt_signed_unit(15.34);
    assert!(s.ends_with('%') && !s.contains("USDT"), "percent mode: {s}");

    set_pnl_pct(false);
    assert_eq!(pnl_unit_label(), "USDT");
    let s = fmt_signed_unit(15.34);
    assert!(s.ends_with(" USDT") && !s.contains('%'), "usdt mode: {s}");
}

/// A non-finite figure stays a bare em dash and is never given a unit, in either mode.
#[test]
fn non_finite_stays_a_bare_em_dash() {
    set_pnl_pct(true);
    assert_eq!(fmt_signed_unit(f64::NAN), "—");
    set_pnl_pct(false);
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
    set_pnl_pct(true);
    let pct = render();
    assert!(
        !pct.contains("USDT"),
        "percent mode still shows USDT: {pct}"
    );
    set_pnl_pct(false);
    let usdt = render();
    assert!(usdt.contains("USDT"), "usdt mode lost the unit: {usdt}");
}
