//! Shared coin-menu value and delayed workspace-authority regressions.

mod layout;

/// Every mutating callback in `own.rs` must validate current workspace authority
/// inside its Backend update and before its first command helper.
///
/// Mutation: remove or move the guard after any listed command. A menu opened on core 7 could then
/// blacklist, join, split, cancel, or open an editor after Auto switched the group to core 9.
///
/// Returns:
///     Nothing; every listed callback must retain guard-before-effect ordering.
#[test]
fn shared_menu_mutations_revalidate_before_their_first_side_effect() {
    let source = include_str!("own.rs");
    let cases = [
        ("\"coin-order-edit\"", "crate::panels::open_order_edit("),
        ("\"coin-order-join\"", "b.session.join_sells("),
        ("\"coin-order-split\"", "b.session.split_order("),
        ("\"coin-order-split-n\"", "b.session.split_order("),
        ("\"coin-order-cancel\"", "b.session.cancel_order("),
    ];

    for (key, side_effect) in cases {
        let callback = source
            .split_once(key)
            .unwrap_or_else(|| panic!("missing shared-menu action {key}"))
            .1;
        let update = callback
            .find(".update(app, |b, _|")
            .unwrap_or_else(|| panic!("{key} must re-read Backend authority"));
        let guard = callback
            .find("workspace_action_allows_cores(")
            .unwrap_or_else(|| panic!("{key} must validate its captured core targets"));
        let effect = callback
            .find(side_effect)
            .unwrap_or_else(|| panic!("{key} lost its expected side effect"));

        assert!(
            update < guard && guard < effect,
            "stale-action guard moved in {key}"
        );
    }
}

/// Every blacklist row — permanent or temporary — must revalidate the workspace inside its Backend
/// update and BEFORE it writes anything.
///
/// The rows share two dispatchers rather than repeating the guard once per row, so this is where
/// the ordering now lives: `target_row` for the permanent list, `hour_rows` and the lift row for
/// the temporary one.
///
/// Mutation: move either guard after its write. A menu opened on core 7 could then ban a coin on
/// core 9 after Auto switched the group under it.
#[test]
fn blacklist_rows_revalidate_before_they_write() {
    let source = include_str!("blacklist.rs");
    let cases = [
        ("fn target_row(", "write(b, &cores)"),
        ("fn hour_rows(", "send_temp_ban("),
        ("\"tbl-clear\"", "send_temp_ban("),
    ];

    for (anchor, side_effect) in cases {
        let body = source
            .split_once(anchor)
            .unwrap_or_else(|| panic!("missing blacklist dispatcher {anchor}"))
            .1;
        let update = body
            .find(".update(app, |b, _|")
            .unwrap_or_else(|| panic!("{anchor} must re-read Backend authority"));
        let guard = body
            .find("workspace_action_allows_cores(")
            .unwrap_or_else(|| panic!("{anchor} must validate its captured core targets"));
        let effect = body
            .find(side_effect)
            .unwrap_or_else(|| panic!("{anchor} lost its expected side effect"));

        assert!(
            update < guard && guard < effect,
            "stale-action guard moved in {anchor}"
        );
    }

    // And every permanent-list row goes through that one dispatcher rather than writing directly.
    let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
    for key in ["coin-bl-core", "coin-bl-cores", "coin-bl-strat"] {
        assert!(
            compact.contains(&format!("target_row(\"{key}\"")),
            "{key} must be built by target_row, which is where the guard lives"
        );
    }
}

/// Selected-cores must lift or add as one action: a mixed group adds, a fully listed group lifts.
///
/// Mutation: call `write_core_blacklist` with a per-core contains check instead of one `lift` for
/// the whole target set. Clicking "Selected cores (N)" on a mixed set would then remove the coin
/// from cores that already listed it while adding it to the rest.
#[test]
fn selected_cores_share_one_lift_decision() {
    let source = include_str!("blacklist.rs");
    let writer = source
        .split_once("fn core_blacklist_writer(")
        .expect("core_blacklist_writer must exist")
        .1;
    let compact: String = writer.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(
        compact.contains(".all(|(core,token)|blacklist_contains("),
        "the group writer must decide lift from every target before writing any"
    );
    assert!(
        compact.contains("write_core_blacklist(b,*core,token,lift)"),
        "every core must receive the same lift flag"
    );
}

/// The strategy row must re-read the schema inside its own write, not only when the menu is built;
/// removing that check sends a stale field edit after the strategy is gone, which the view editor
/// discards without a word.
#[test]
fn strategy_blacklist_row_revalidates_live_identity_and_schema() {
    let source = include_str!("blacklist.rs");
    let row = source
        .split_once("\"coin-bl-strat\"")
        .expect("strategy blacklist row must exist")
        .1;
    let schema_guard = row
        .find("b.session.strategy_has_blacklist(core, sid)")
        .expect("the write must revalidate the exact strategy schema");
    let effect = row
        .find("write_strategy_blacklist(b, core, sid")
        .expect("the write must retain its intended edit");

    assert!(schema_guard < effect);
}

/// `CoinMenuCtx::workspace_group` must distinguish group-owned panels and charts from intentionally
/// unscoped global Assets and standalone Report surfaces.
///
/// Mutation: pass `None` from a group panel or a group from an unscoped host. The former restores
/// stale Auto writes; the latter silently removes Classic/global action authority.
///
/// Returns:
///     Nothing; source wiring must preserve each host's intended authority boundary.
#[test]
fn menu_callers_preserve_scoped_and_unscoped_authority() {
    let compact =
        |source: &str| -> String { source.chars().filter(|ch| !ch.is_whitespace()).collect() };
    let orders = compact(include_str!("../../panels/orders/table.rs"));
    let assets = compact(include_str!("../../panels/assets/table.rs"));
    let report_actions = compact(include_str!("../../panels/report/actions.rs"));
    let report_columns = compact(include_str!("../../panels/report/columns.rs"));
    let chart = compact(include_str!("../../panels/chart/trade.rs"));

    assert!(orders.contains("workspace_group:Some(workspace_group)"));
    assert!(assets.contains("AssetsScope::Group(group)=>Some(group.clone())"));
    assert!(assets.contains("AssetsScope::All=>None"));
    assert!(report_actions.contains("(!self.standalone).then(||self.group.clone())"));
    assert!(report_columns.contains("workspace_group,"));
    assert!(chart.contains("workspace_group:self.workspace_group.clone()"));
}

/// Shared-menu navigation keeps the same captured group authority as order mutations.
#[test]
fn shared_menu_navigation_revalidates_captured_workspace_scope() {
    let own = include_str!("own.rs");
    let coin = include_str!("coin.rs");
    let strategy = include_str!("strategy.rs");

    assert!(own.contains("b.open_on_main_if_authorized("));
    assert!(coin.contains("b.open_compare_if_authorized("));
    let goto = strategy
        .split("crate::strategies::open_goto(")
        .nth(1)
        .expect("strategy navigation must exist");
    assert!(goto.contains("workspace_group.clone(),"));
}

/// Strategy Report must still open a Report for the clicked strategy.
#[test]
fn strategy_report_opens_the_scoped_report() {
    let source = include_str!("strategy.rs");
    assert!(source.contains("crate::panels::open_scoped_report("));
    assert!(source.contains("crate::panels::ReportScope::for_strategy("));
}

use super::plan::{CoinMenuOrigin, MenuFacts, Order, ReportFacts, Slot, plan};
use Slot::*;
use moon_core::feed::ForceCancelGate;

/// A listed market with a printed ticker and a resolved blacklist token.
fn market_facts() -> MenuFacts<(), Result<(), &'static str>> {
    MenuFacts {
        surface: CoinMenuOrigin::OrderTable,
        has_market: true,
        coin_shown: true,
        strategy: None,
        order: None,
        split_n: 7,
        report: None,
        screener_copy: false,
        blacklist: true,
        force_cancel: ForceCancelGate::NotEligible,
    }
}

/// A Report row with a resolved task and an available filter action.
fn report_facts() -> MenuFacts<(), Result<(), &'static str>> {
    MenuFacts {
        report: Some(ReportFacts {
            trade_window: (),
            trade_log: Ok(()),
            filter_coin: Some(()),
        }),
        ..market_facts()
    }
}

/// Moving strategy above coin actions would break the approved order on Report rows.
#[test]
fn plan_report_row_with_strategy() {
    let facts = MenuFacts {
        strategy: Some(19),
        ..report_facts()
    };
    assert_eq!(
        plan(&facts),
        [
            TradeWindow(()),
            ShowOnChart,
            TradeLog(Ok(())),
            Sep,
            FilterHere(()),
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            StrategyGoto(19),
            StrategyReport(19),
            Sep,
            Blacklist
        ]
    );
}

/// A manual Report row must still expose its coin actions and omit the empty strategy group.
#[test]
fn plan_report_row_without_strategy() {
    assert_eq!(
        plan(&report_facts()),
        [
            TradeWindow(()),
            ShowOnChart,
            TradeLog(Ok(())),
            Sep,
            FilterHere(()),
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            Blacklist
        ]
    );
}

/// Dropping an unavailable log would hide why the clicked trade cannot show its log.
#[test]
fn plan_report_row_missing_task_keeps_disabled_log_in_place() {
    let mut facts = report_facts();
    facts.report.as_mut().unwrap().trade_log = Err("report.trade_log.no_task");
    assert_eq!(
        plan(&facts),
        [
            TradeWindow(()),
            ShowOnChart,
            TradeLog(Err("report.trade_log.no_task")),
            Sep,
            FilterHere(()),
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            Blacklist
        ]
    );
    // A stored Report ticker remains usable even when the market catalog cannot resolve it.
    facts.has_market = false;
    facts.blacklist = false;
    assert_eq!(
        plan(&facts),
        [
            TradeWindow(()),
            NotReady,
            TradeLog(Err("report.trade_log.no_task")),
            Sep,
            FilterHere(()),
            CoinReport,
            CopyTicker
        ]
    );
}

/// The sell line groups split commands behind its own separator and avoids duplicate split counts.
#[test]
fn plan_chart_sell_line_splits_behind_own_separator() {
    let mut facts = MenuFacts {
        surface: CoinMenuOrigin::ChartLine,
        strategy: Some(19),
        order: Some(Order::Sell(23)),
        ..market_facts()
    };
    assert_eq!(
        plan(&facts),
        [
            OrderEdit(23),
            JoinSells,
            Sep,
            Split(23, 3),
            Split(23, 7),
            Sep,
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            StrategyGoto(19),
            StrategyReport(19),
            Sep,
            Blacklist
        ]
    );
    facts.split_n = 3;
    assert_eq!(
        plan(&facts),
        [
            OrderEdit(23),
            JoinSells,
            Sep,
            Split(23, 3),
            Sep,
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            StrategyGoto(19),
            StrategyReport(19),
            Sep,
            Blacklist
        ]
    );
}

/// A buy line must keep Cancel behind the order separator and omit Report-only filtering.
#[test]
fn plan_chart_buy_line_cancel_after_separator_no_filter() {
    let facts = MenuFacts {
        surface: CoinMenuOrigin::ChartLine,
        order: Some(Order::Buy(23)),
        ..market_facts()
    };
    assert_eq!(
        plan(&facts),
        [
            OrderEdit(23),
            Sep,
            Cancel(23),
            Sep,
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            Blacklist
        ]
    );
}

/// Orders still opens the chart before order actions, unlike an order line on the chart.
#[test]
fn plan_orders_table_row() {
    let facts = MenuFacts {
        order: Some(Order::Table(23)),
        ..market_facts()
    };
    assert_eq!(
        plan(&facts),
        [
            OpenOnChart,
            OrderEdit(23),
            Sep,
            Cancel(23),
            Sep,
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            Blacklist
        ]
    );
}

/// Assets and Screener omit order and strategy actions. Screener keeps its own copy entries.
#[test]
fn plan_assets_screener_row_no_strategy_no_order() {
    let mut facts = market_facts();
    assert_eq!(
        plan(&facts),
        [
            OpenOnChart,
            Sep,
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            Blacklist
        ]
    );
    facts.screener_copy = true;
    assert_eq!(
        plan(&facts),
        [
            OpenOnChart,
            Sep,
            ScreenerCopy,
            Sep,
            CoinReport,
            Compare,
            CopyTicker,
            Sep,
            Blacklist
        ]
    );
}

/// Skipping any group, including a chart ban lock's own group, must leave no dangling separators.
#[test]
fn plan_never_leading_trailing_or_double_separator() {
    for surface in [CoinMenuOrigin::ChartLine, CoinMenuOrigin::OrderTable] {
        for order in [
            None,
            Some(Order::Sell(23)),
            Some(Order::Buy(23)),
            Some(Order::Table(23)),
        ] {
            for (mask, force_cancel) in (0..64u8).flat_map(|mask| {
                [
                    ForceCancelGate::NotEligible,
                    ForceCancelGate::NeedsNewerCore,
                    ForceCancelGate::Offered,
                ]
                .map(|gate| (mask, gate))
            }) {
                let has = |bit: u8| mask & bit != 0;
                let slots = plan(&MenuFacts::<(), Result<(), &str>> {
                    surface,
                    has_market: has(1),
                    coin_shown: has(2),
                    strategy: has(4).then_some(19),
                    order,
                    split_n: 7,
                    report: has(8).then_some(ReportFacts {
                        trade_window: (),
                        trade_log: Ok(()),
                        filter_coin: Some(()),
                    }),
                    screener_copy: has(16),
                    blacklist: has(32),
                    force_cancel,
                });
                assert_ne!(slots.first(), Some(&Sep));
                assert_ne!(slots.last(), Some(&Sep));
                assert!(!slots.windows(2).any(|pair| pair == [Sep, Sep]));
            }
        }
    }
}

/// Exactly the Report trade window and the order editor receive emphasis.
#[test]
fn emphasized_slot_is_main_action_per_surface() {
    let order = |surface, order| MenuFacts {
        surface,
        order: Some(order),
        ..market_facts()
    };
    for (facts, expected) in [
        (report_facts(), vec![TradeWindow(())]),
        (
            order(CoinMenuOrigin::ChartLine, Order::Sell(23)),
            vec![OrderEdit(23)],
        ),
        (
            order(CoinMenuOrigin::ChartLine, Order::Buy(23)),
            vec![OrderEdit(23)],
        ),
        (
            order(CoinMenuOrigin::OrderTable, Order::Table(23)),
            vec![OrderEdit(23)],
        ),
        (market_facts(), vec![]),
    ] {
        let emphasized: Vec<_> = plan(&facts).into_iter().filter(Slot::emphasized).collect();
        assert_eq!(emphasized, expected);
    }
}

/// The forced cancellation sits directly under Cancel on a buy line and an Orders row.
///
/// Mutation: drop the slot for `Offered`, or emit it for `NotEligible`. A stuck BuySet would lose
/// its only way out, or a pending entry would offer an action MoonProto never sends.
#[test]
fn plan_force_cancel_follows_cancel_only_when_offered() {
    for (surface, order) in [
        (CoinMenuOrigin::ChartLine, Order::Buy(23)),
        (CoinMenuOrigin::OrderTable, Order::Table(23)),
    ] {
        let facts = |force_cancel| MenuFacts {
            surface,
            order: Some(order),
            force_cancel,
            ..market_facts()
        };
        let own = |facts: &MenuFacts<(), Result<(), &'static str>>| {
            let slots = plan(facts);
            let cancel = slots.iter().position(|slot| *slot == Cancel(23)).unwrap();
            slots[cancel + 1..]
                .iter()
                .take_while(|slot| **slot != Sep)
                .count()
        };
        let offered = plan(&facts(ForceCancelGate::Offered));
        let cancel = offered.iter().position(|slot| *slot == Cancel(23)).unwrap();
        assert_eq!(offered[cancel + 1], ForceCancel(23));
        let needs = plan(&facts(ForceCancelGate::NeedsNewerCore));
        let cancel = needs.iter().position(|slot| *slot == Cancel(23)).unwrap();
        assert_eq!(needs[cancel + 1], ForceCancelNeedsCore);
        assert_eq!(own(&facts(ForceCancelGate::NotEligible)), 0);
        assert!(
            !plan(&facts(ForceCancelGate::NotEligible))
                .iter()
                .any(|slot| matches!(slot, ForceCancel(_) | ForceCancelNeedsCore))
        );
    }
}

/// A sell line has no ordinary Cancel, so the forced one stands behind its own separator.
///
/// Mutation: append it to the split group. It would read as a third split row.
#[test]
fn plan_force_cancel_on_a_sell_line_has_its_own_group() {
    let slots = plan(&MenuFacts {
        surface: CoinMenuOrigin::ChartLine,
        order: Some(Order::Sell(23)),
        force_cancel: ForceCancelGate::Offered,
        ..market_facts()
    });
    assert_eq!(
        slots[..7],
        [
            OrderEdit(23),
            JoinSells,
            Sep,
            Split(23, 3),
            Split(23, 7),
            Sep,
            ForceCancel(23)
        ]
    );
}

/// No order, no forced cancellation, whatever the gate says.
///
/// Mutation: read the gate before the order. An Assets row would show a cancel for nothing.
#[test]
fn plan_force_cancel_needs_an_order() {
    let slots = plan(&MenuFacts {
        force_cancel: ForceCancelGate::Offered,
        ..market_facts()
    });
    assert!(
        !slots
            .iter()
            .any(|slot| matches!(slot, ForceCancel(_) | ForceCancelNeedsCore))
    );
}

/// The forced cancellation is sent only from the confirmation's answer, after the workspace, the
/// order's eligibility and its leg are re-checked, and never as the ordinary cancel.
///
/// Mutation: send from the menu row's click, move either check after the send, compare only the
/// gate, or call `cancel_order`. A click would then skip the question, a group switched by Auto
/// would still be forced, a buy that filled into SellSet while the dialog was open would have its
/// take-profit forced instead, or the wrong command would go out.
#[test]
fn force_cancel_sends_only_from_the_checked_answer() {
    let source = include_str!("force_cancel.rs");
    let row = source
        .split_once("\"coin-order-force-cancel\"")
        .expect("the force-cancel row")
        .1
        .split_once("fn needs_core_item(")
        .unwrap()
        .0;
    assert!(row.contains("ask("), "the row must open the question");
    assert!(
        !row.contains("b.session."),
        "the row itself must not send anything"
    );
    let answer = source
        .split_once("fn confirm(")
        .expect("the answer handler")
        .1;
    let update = answer
        .find(".update(app, |b, _|")
        .expect("re-reads Backend");
    let guard = answer
        .find("workspace_action_allows_cores(")
        .expect("re-checks the workspace");
    let still = answer
        .find("order_state(b, core, uid) != Some((ForceCancelGate::Offered, leg.clone()))")
        .expect("re-checks the order and its leg");
    let send = answer
        .find("b.session.force_cancel_order(")
        .expect("sends the forced cancellation");
    assert!(update < guard && guard < send && still < send);
    assert!(
        !source.contains(".cancel_order("),
        "never the ordinary cancel"
    );
}
