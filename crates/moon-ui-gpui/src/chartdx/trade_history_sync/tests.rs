//! Regressions for the drawing-time trade-kind filter.

use super::trade_kind_visible;
use moon_core::config::ChartGraphicsCfg;

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
