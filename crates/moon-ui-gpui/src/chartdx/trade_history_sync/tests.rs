//! Regressions for trade-history drawing inputs and filtering.

use std::collections::HashMap;

use super::{trade_kind_visible, trade_mark};
use moon_core::config::ChartGraphicsCfg;
use moon_core::db::{ChartTradeRecord, OffsetSegment, ReportAxis};

/// Moving this filter back into the durable QUERY, or inverting either checkbox, must fail here.
///
/// The pair is the graphics popup's two trade-kind boxes, and the rule is one box per kind with no
/// interaction between them: unticking "real" must not touch emulator marks and vice versa, and
/// unticking both must leave nothing drawn rather than everything. The LOCATION matters as much as
/// the truth table - a display toggle must never decide which rows the history was read with,
/// because the 1000-row cap is applied after any SQL predicate and hiding one kind would then
/// surface older trades of the other that had been truncated away.
#[test]
fn each_trade_kind_checkbox_hides_only_its_own_kind() {
    let cfg = |real: bool, emulator: bool| ChartGraphicsCfg {
        show_real_trades: real,
        show_emulator_trades: emulator,
        ..ChartGraphicsCfg::default()
    };

    // Shipped default: everything visible.
    assert!(trade_kind_visible(&ChartGraphicsCfg::default(), false));
    assert!(trade_kind_visible(&ChartGraphicsCfg::default(), true));

    // One box each, no crosstalk.
    assert!(trade_kind_visible(&cfg(true, false), false));
    assert!(!trade_kind_visible(&cfg(true, false), true));
    assert!(!trade_kind_visible(&cfg(false, true), false));
    assert!(trade_kind_visible(&cfg(false, true), true));

    // Both off draws NOTHING - never everything, which is what a single tri-state predicate that
    // cannot express "neither" would have produced.
    assert!(!trade_kind_visible(&cfg(false, false), false));
    assert!(!trade_kind_visible(&cfg(false, false), true));
}

/// `chartdx/trade_history_sync.rs:trade_mark` must correct seconds before scaling milliseconds.
///
/// Replacing the two `axis.to_utc` calls with raw record dates would put entry and exit arrows on
/// the wrong candle whenever a core reports a non-zero clock offset.
#[test]
fn trade_mark_places_a_clock_skewed_trade_on_its_true_utc_candles() {
    let record = ChartTradeRecord {
        record_id: 9,
        core_uid: 42,
        coin: "BTCUSDT".into(),
        buy_date: 1_700_000_000,
        close_date: 1_700_000_900,
        buy_price: 63_000.0,
        sell_price: 63_100.0,
        quantity: 0.25,
        is_short: false,
        emulator: false,
        profit: Some(25.0),
        quote: None,
        profit_pct: Some(0.16),
    };
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            record.core_uid,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: 3_600,
            }],
        )]),
        chrono_tz::UTC,
    );

    let mark = trade_mark(&record, &axis);

    assert_eq!(mark.buy_ms, 1_699_996_400_000);
    assert_eq!(mark.close_ms, 1_699_997_300_000);
}
