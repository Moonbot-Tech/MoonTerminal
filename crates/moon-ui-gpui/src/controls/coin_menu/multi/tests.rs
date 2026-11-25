//! Multi-row menu order, localized counts, copy, and blacklist target regressions.

use super::{
    MultiFacts, MultiSlot, capped_trade_rows, distinct_targets, distinct_tickers, plan_multi,
    tickers_text,
};

/// Five trades on three tickers preserve the approved captions and compact grouping.
#[test]
fn plan_multi_five_rows_three_coins() {
    use MultiSlot::*;

    let _locale = crate::test_locale::force("ru");
    let facts = MultiFacts {
        rows: 5,
        coins: ["ABC", "DEF", "GHI"].map(str::to_owned).to_vec(),
    };
    let slots = plan_multi(&facts);
    assert_eq!(
        slots,
        [
            TradeWindows(5),
            Sep,
            Filter(3),
            NewReport,
            Copy,
            Sep,
            Permanent(3),
            Temporary(3)
        ],
    );
    assert_eq!(slots[0].label(), "Окна сделок (5)");
    assert_eq!(slots[2].label(), "Только эти 3 монеты в отчёте");
    assert_eq!(slots[6].label(), "Добавить 3 монеты в ЧС");
}

/// Repeated tickers keep their first spelling and never insert commas or a trailing line.
#[test]
fn copy_tickers_newline_joined_distinct_in_order() {
    let coins = distinct_tickers(["DEF", "ABC", "def", "", "龙虾", "ABC"].map(str::to_owned));
    assert_eq!(tickers_text(&coins), "DEF\nABC\n龙虾");
}

/// A repeated token on one core collapses while another core still receives its own spelling.
#[test]
fn multi_add_targets_dedupe_core_token() {
    let targets = [
        (7, "10ABC"),
        (9, "ABC"),
        (7, "10abc"),
        (7, "DEF"),
        (9, "ABC"),
    ]
    .map(|(core, token)| (core, token.to_owned()));
    assert_eq!(
        distinct_targets(targets.to_vec()),
        [
            (7, "10ABC".to_owned()),
            (9, "ABC".to_owned()),
            (7, "DEF".to_owned())
        ],
    );
}

/// The menu and Enter must preserve order and cap a larger selection before resolving any rows.
#[test]
fn trade_windows_capped_at_20() {
    let rows: Vec<_> = (1..=25).rev().collect();

    let capped = capped_trade_rows(&rows);
    assert_eq!(capped.len(), 20);
    assert_eq!(capped.first(), Some(&25));
    assert_eq!(capped.last(), Some(&6));
    assert_eq!(capped_trade_rows(&[8, 3]), &[8, 3]);
}
