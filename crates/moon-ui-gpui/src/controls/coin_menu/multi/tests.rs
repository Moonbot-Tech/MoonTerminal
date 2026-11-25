//! Multi-row menu order, localized counts, copy, and blacklist target regressions.

use std::collections::HashMap;
use std::rc::Rc;

use super::{
    BlacklistTargets, MultiFacts, MultiSlot, TempTarget, TradeSeed, UndoStep, capped_trade_rows,
    distinct_targets, distinct_tickers, multi_added_caption, plan_multi, tickers_text,
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
    let targets = BlacklistTargets {
        own: facts.coins.iter().cloned().map(|coin| (1, coin)).collect(),
        temporary: facts
            .coins
            .iter()
            .map(|coin| TempTarget {
                core: 1,
                symbol: format!("{coin}USDT"),
                ticker: coin.clone(),
            })
            .collect(),
        ..BlacklistTargets::default()
    };
    let slots = plan_multi(&facts, &targets);
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
    assert_eq!(
        slots
            .into_iter()
            .filter(|slot| slot.emphasized())
            .collect::<Vec<_>>(),
        [TradeWindows(5)]
    );
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
fn trade_windows_cap_follows_max_windows() {
    let rows: Vec<_> = (1..=TradeSeed::MAX_WINDOWS + 1).rev().collect();

    let capped = capped_trade_rows(&rows);
    assert_eq!(capped, &rows[..TradeSeed::MAX_WINDOWS]);
    assert_eq!(capped_trade_rows(&[8]), &[8]);
}

/// Unresolved printed tickers stay in Report actions but never inflate blacklist captions.
#[test]
fn multi_bl_caption_counts_resolvable_coins_only() {
    let facts = MultiFacts {
        rows: 5,
        coins: ["XALPHA", "XBETA", "XGAMMA"].map(str::to_owned).to_vec(),
    };
    let targets = BlacklistTargets {
        own: vec![
            (1, "XALPHA".to_string()),
            (2, "10XBETA".to_string()),
            (3, "xbeta".to_string()),
        ],
        tickers: Rc::new(HashMap::from([(
            (2, "10XBETA".to_string()),
            "XBETA".to_string(),
        )])),
        temporary: vec![TempTarget {
            core: 1,
            symbol: "XALPHAUSDT".to_string(),
            ticker: "XALPHA".to_string(),
        }],
        ..BlacklistTargets::default()
    };
    let slots = plan_multi(&facts, &targets);
    assert!(slots.contains(&MultiSlot::Filter(3)));
    assert!(slots.contains(&MultiSlot::Permanent(2)));
    assert!(slots.contains(&MultiSlot::Temporary(1)));
}

/// Name the first three printed tickers and name or count only the affected destinations.
#[test]
fn multi_toast_names_coins_and_cores() {
    let _locale = crate::test_locale::force("ru");
    let coins = ["XALPHA", "XBETA", "XGAMMA", "XDELTA"].map(str::to_owned);
    let step = |core| UndoStep::inserted_many(core, Vec::new(), |_| Ok(true));
    assert_eq!(
        multi_added_caption(&[step(1)], coins.clone(), |_| "CORE_A".to_string()),
        "XALPHA, XBETA, XGAMMA и ещё 1 в ЧС · CORE_A"
    );
    assert_eq!(
        multi_added_caption(&[step(1), step(2)], coins, |_| panic!("multiple cores")),
        "XALPHA, XBETA, XGAMMA и ещё 1 в ЧС · Ядер: 2"
    );
}
