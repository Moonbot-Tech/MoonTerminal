//! Filter-line tooltip and GPU-free strategy identity routing regressions.

use super::{filter_line_tooltip_key, strategy_reveal_target};
use crate::chartdx::FilterLineTarget;
use crate::strategies::RevealTarget;

/// A global line and a strategy line name different sentences. Swapping the keys would open
/// the core tooltip on a strategy line.
#[test]
fn the_tooltip_key_follows_the_blacklist_kind() {
    assert_eq!(
        filter_line_tooltip_key(&FilterLineTarget::GlobalBlacklist {
            coin: "XALPHA".into(),
        }),
        "chart_labels.filter.open_core_bl"
    );
    assert_eq!(
        filter_line_tooltip_key(&FilterLineTarget::StrategyBlacklist {
            strategy: "UNIT_01".into(),
            coin: "WBTC".into(),
        }),
        "chart_labels.filter.open_strategy_bl"
    );
}

/// Stripping a display code before the exact match can select another strategy on the same core.
#[test]
fn strategy_resolution_prefers_exact_then_stripped_then_deferred_name() {
    let name = "UNIT_01 [CODE]";
    assert_eq!(
        strategy_reveal_target(name, [(1, "UNIT_01"), (2, name)]),
        RevealTarget::Id(2)
    );
    assert_eq!(
        strategy_reveal_target(name, [(1, "UNIT_01")]),
        RevealTarget::Id(1)
    );
    assert_eq!(
        strategy_reveal_target(name, [(1, "OTHER")]),
        RevealTarget::Name(name.into())
    );
    assert_eq!(
        strategy_reveal_target("UNIT_01 []", [(1, "UNIT_01")]),
        RevealTarget::Name("UNIT_01 []".into())
    );
    assert_eq!(
        strategy_reveal_target("UNIT_01 [FIRST] [LAST]", [(1, "UNIT_01 [FIRST]")]),
        RevealTarget::Id(1)
    );
}

/// A strategy-only refusal must not claim a blacklist or field reveal in its tooltip.
#[test]
fn strategy_tooltips_distinguish_optional_field_reveals() {
    assert_eq!(
        filter_line_tooltip_key(&FilterLineTarget::Strategy {
            strategy: "UNIT_01".into(),
            field: None
        }),
        "chart_labels.filter.open_strategy"
    );
    assert_eq!(
        filter_line_tooltip_key(&FilterLineTarget::Strategy {
            strategy: "UNIT_01".into(),
            field: Some("CustomEMA")
        }),
        "chart_labels.filter.open_strategy_field"
    );
}

/// A filter press must be consumed before either button can execute a trading gesture.
#[test]
fn both_buttons_route_filter_lines_before_trading() {
    let source = include_str!("../render_input.rs");
    let left = source
        .split_once("pub(super) fn mouse_down_left(")
        .unwrap()
        .1
        .split_once("pub(super) fn mouse_up_left(")
        .unwrap()
        .0;
    let right = source
        .split_once("pub(super) fn mouse_down_right(")
        .unwrap()
        .1
        .split_once("pub(super) fn mouse_up_right(")
        .unwrap()
        .0;
    for body in [left, right] {
        let filter = body
            .find("this.try_open_filter_line(pos, window, cx)")
            .unwrap();
        assert!(filter < body.find("this.try_action_click(").unwrap());
        assert!(filter < body.find("this.try_place_order_click(").unwrap());
    }
    let consumed = right
        .split_once("this.try_open_filter_line(pos, window, cx)")
        .unwrap()
        .1
        .split_once("if within")
        .unwrap()
        .0;
    assert!(consumed.contains("this.suppress_rmb_up = true;"));
    assert!(consumed.contains("cx.stop_propagation();"));
    assert!(consumed.contains("return;"));
}
