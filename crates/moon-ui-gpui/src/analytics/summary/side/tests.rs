//! Rules of the side column: the best/worst switch picks its rows, and an unresolved strategy
//! keeps its status label and readable id tail with the full id behind a tooltip.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute.
use super::{side_rows, trade_cells};
use crate::analytics::summary::state::TradeSide;
use crate::analytics::summary::{short_id, strat_display, strat_display_ex};
use moon_core::db::analytics::{Summary, TopTrade};

/// A synthetic trade on coin `coin` with the given profit.
fn trade(coin: &str, profit: f64) -> TopTrade {
    TopTrade {
        closedate: 1_800_000_000,
        coin: coin.to_string(),
        strategy: "XALPHA ~ F-01 / SUB 01".to_string(),
        strategy_id: "7".to_string(),
        core_name: "XCORE-01".to_string(),
        profit,
        strategy_is_id: false,
        strategy_alive: Some(2),
        is_short: false,
    }
}

/// The switch swaps the whole row set: best lists `best`, worst lists `worst`, never a mix.
#[test]
fn side_switch_swaps_rows() {
    let d = Summary {
        best: vec![trade("XCOINUSDT", 12.5), trade("XBETAUSDT", 8.0)],
        worst: vec![trade("XGAMMAUSDT", -9.0)],
        ..Default::default()
    };
    let coins = |side| {
        side_rows(&d, side)
            .iter()
            .map(|t| t.coin.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(coins(TradeSide::Best), ["XCOINUSDT", "XBETAUSDT"]);
    assert_eq!(coins(TradeSide::Worst), ["XGAMMAUSDT"]);
}

/// A trades row whose strategy the database could not name prints the muted status with the id
/// tail and carries the full id for its tooltip; the coin is never shortened.
#[test]
fn trade_row_keeps_the_unresolved_strategy_tooltip_and_the_whole_coin() {
    let _locale = crate::test_locale::force("en");
    let id = "-7653179346322682234";
    let mut tr = trade("XVERYLONGCOINNAMEUSDT", -3.0);
    tr.strategy = id.to_string();
    tr.strategy_id = id.to_string();
    tr.strategy_is_id = true;
    tr.strategy_alive = Some(0);
    tr.is_short = true;
    let c = trade_cells(&tr, chrono_tz::UTC);
    assert_eq!(c.coin, "XVERYLONGCOINNAMEUSDT");
    assert!(c.is_short);
    assert_eq!(c.strategy.text, "deleted strategy #…682234");
    assert!(c.strategy.muted);
    assert_eq!(c.strategy.full_id.as_deref(), Some(id));
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

/// The insights caption counts the selected period, not the days that traded: a 30-day period
/// with one traded day reads "30 days"; counting `days` would print "1 day".
#[test]
fn period_caption_counts_the_selected_period() {
    let _locale = crate::test_locale::force("en");
    let d = Summary {
        bucket_secs: 86_400,
        days: vec![moon_core::db::analytics::DayPoint {
            start: 0,
            profit: 1.0,
            trades: 1,
        }],
        from: 0,
        to: 30 * 86_400,
        ..Default::default()
    };
    let caption = super::period_caption(&d);
    assert!(caption.contains("30"), "{caption}");
}
