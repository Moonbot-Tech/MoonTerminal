//! Static action-authority regressions for group-owned chart panels.

use std::{collections::HashMap, rc::Rc};

use super::ChartSettingsSig;
use moon_core::{
    config::{ChartGraphicsCfg, ChartLabelsCfg, ChartTheme, OrdersStyleSet},
    db::{OffsetSegment, ReportAxis},
    market::CandleViewCfg,
};

/// Removing a dispatch-time `workspace_action_allows_core` guard from `trade.rs`, `render.rs`, or
/// `render_input.rs` must fail: a stale chart could trade or navigate a core other than the Auto
/// rail selection.
#[test]
fn every_chart_command_and_navigation_path_revalidates_auto_authority() {
    let trade = include_str!("trade.rs");
    for command in [
        "manual chart order blocked",
        "chart start-cross cancel",
        "hotkey cancel hovered order failed",
        "hotkey split hovered order failed",
        "manual chart move line:",
    ] {
        let command_at = trade
            .find(command)
            .unwrap_or_else(|| panic!("missing chart action marker: {command}"));
        let prefix = &trade[..command_at];
        assert!(
            prefix
                .rfind("workspace_action_allows_core")
                .is_some_and(|guard_at| command_at - guard_at < 2_500),
            "{command} must follow a nearby live workspace-authority guard"
        );
    }
    let menu = trade
        .split("pub(super) fn try_open_order_menu(")
        .nth(1)
        .and_then(|tail| tail.split("pub fn cancel_hovered_order(").next())
        .expect("chart order-menu producer must remain present");
    assert!(
        menu.find("workspace_action_allowed").unwrap()
            < menu
                .find("workspace_group: self.workspace_group.clone()")
                .unwrap(),
        "the chart menu must validate before carrying group authority into delayed callbacks"
    );

    // The market buttons are placed where the caption layout reserved room for them, so their
    // callback fires a frame after the rectangle was published — and the rail guard has to hold on
    // that path just the same. It is the LAST gate: the rail can have moved to another core between
    // the frame that laid the button out and the press.
    let actions = include_str!("market_actions.rs");
    let dispatch = actions
        .split("fn dispatch_market_action(")
        .nth(1)
        .expect("chart market-action dispatch must remain present");
    let guard = dispatch
        .find("workspace_action_allows_core(group, core)")
        .expect("the dispatch must revalidate the workspace rail");
    for command in ["cancel_market_buys", "toggle_panic_sell", "set_temp_ban"] {
        assert!(
            guard
                < dispatch
                    .find(command)
                    .unwrap_or_else(|| panic!("{command} must be dispatched from one place")),
            "{command} must revalidate the workspace rail before dispatch"
        );
    }

    let input = include_str!("render_input.rs");
    let navigation = input
        .split("if let Some((core, market)) = this.input.pending_to_main.take()")
        .nth(1)
        .and_then(|tail| tail.split("if input_changed || opened_to_main").next())
        .expect("AddToChart-to-Main navigation callback must remain present");
    assert!(
        navigation.find("workspace_action_allows_core").unwrap()
            < navigation.find("open_on_main").unwrap(),
        "old chart navigation must not bypass the Auto rail"
    );
}

/// Removing the group argument from `main_stack.rs` or `add_stack.rs` must fail: chart callbacks
/// would become explicitly unscoped and the live guards above would always allow stale cores.
#[test]
fn chart_stacks_pass_their_workspace_group_into_every_panel() {
    let main = include_str!("../../chart_tabs/main_stack.rs");
    let add = include_str!("../../chart_tabs/add_stack.rs");
    assert!(main.contains(
        "ChartPanel::new_main(\n                backend,\n                Some(workspace_group),"
    ));
    // Matched without the argument list's layout: the call has since grown a parameter and been
    // wrapped, and pinning its formatting would make this test fail on `cargo fmt` rather than on
    // the thing it guards — the group travelling into the panel.
    assert!(add.contains("ChartPanel::new_addto("));
    let call = add
        .split("ChartPanel::new_addto(")
        .nth(1)
        .expect("the AddToChart panel constructor must remain present");
    let args = call.split(')').next().unwrap_or_default();
    assert!(
        args.contains("workspace_group"),
        "AddToChart panels must be constructed with their workspace group: {args:?}"
    );
}

/// `panels/chart/mod.rs:ChartSettingsSig::eq` must compare the report axis.
///
/// Dropping that comparison leaves an idle chart asleep after a core's clock offset is measured,
/// so its closed-trade arrows continue to use stale timestamps until an unrelated repaint.
#[test]
fn chart_settings_signature_changes_when_the_report_axis_changes() {
    let identity_axis = ReportAxis::from_measured(Default::default(), chrono_tz::UTC);
    let offset_axis = ReportAxis::from_measured(
        HashMap::from([(
            42,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: 3_600,
            }],
        )]),
        chrono_tz::UTC,
    );
    let identity = ChartSettingsSig {
        theme: ChartTheme::default(),
        orders: OrdersStyleSet::default(),
        follow: false,
        chart_graphics: ChartGraphicsCfg::default(),
        candle_view: CandleViewCfg::default(),
        chart_labels: Rc::new(ChartLabelsCfg::default()),
        report_axis: identity_axis,
    };
    let offset = ChartSettingsSig {
        report_axis: offset_axis,
        ..identity.clone()
    };

    assert!(
        identity != offset,
        "a newly measured core offset must wake an idle chart"
    );
}

/// Pins the ORDER the three mouse-down handlers offer a press to their layers.
///
/// The order IS the behaviour: whoever is asked first takes the press, and every "my gesture
/// stopped working" report on this chart traces back to a layer that moved. Nothing else records
/// it — not a type, not a test — so a refactor that regroups these blocks changes what a click
/// does with a green build.
///
/// Plausible breakage: lifting the figure-delete gesture above `try_fig_click` would let a
/// left-bound delete eat the modifier press that draws and grabs figures.
#[test]
fn mouse_down_handlers_offer_a_press_to_their_layers_in_a_fixed_order() {
    let input = include_str!("render_input.rs");
    let handler = |name: &str| {
        input
            .split(&format!("pub(super) fn {name}("))
            .nth(1)
            .unwrap_or_else(|| panic!("missing handler: {name}"))
            .split("\npub(super) fn ")
            .next()
            .expect("handler body")
            .to_string()
    };
    // Each list is the layers of ONE handler, in the order that handler must ask them.
    for (name, layers) in [
        (
            "mouse_down_left",
            vec![
                "try_open_arb_venue",
                "try_fig_click",
                "fig_delete_press",
                "try_place_order_click",
                "try_move_orders_click",
            ],
        ),
        (
            "mouse_down_right",
            vec![
                "try_open_arb_venue",
                "try_open_volume_menu",
                "fig_delete_press",
                "try_open_figure_menu",
                "try_move_orders_click",
                "try_open_order_menu",
                // Last, under both menus — and left out of this list until 10.09.2026, which is how
                // the settings page came to caption a working right-button placement gesture "the
                // right button places no orders".
                "try_place_order_click",
            ],
        ),
        (
            "mouse_down_middle",
            vec![
                "fig_delete_press",
                "try_place_order_click",
                "try_move_orders_click",
                "sync_x_scale_window",
            ],
        ),
    ] {
        let body = handler(name);
        let mut at = 0usize;
        for layer in layers {
            let found = body[at..]
                .find(layer)
                .unwrap_or_else(|| panic!("{name} no longer offers the press to {layer}"));
            at += found + layer.len();
        }
    }
}
