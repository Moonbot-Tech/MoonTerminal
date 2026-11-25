//! Decide which shared coin-menu rows appear and in what order.

/// Origin of the menu, which determines whether opening the chart would be redundant.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CoinMenuOrigin {
    OrderTable,
    ChartLine,
}

/// Order actions available at the clicked point.
#[derive(Clone, Copy)]
pub(super) enum Order {
    Sell,
    Buy,
    Table,
}

/// Report-only row availability, with the locale key for an unavailable trade log.
#[derive(Clone, Copy)]
pub(super) struct ReportFacts {
    pub(super) trade_log: Result<(), &'static str>,
    pub(super) filter_coin: bool,
}

/// Captured row and market facts needed to place menu entries.
pub(super) struct MenuFacts {
    pub(super) surface: CoinMenuOrigin,
    pub(super) has_market: bool,
    pub(super) coin_shown: bool,
    pub(super) strategy: bool,
    pub(super) order: Option<Order>,
    pub(super) split_n: i32,
    pub(super) report: Option<ReportFacts>,
    pub(super) screener_copy: bool,
    pub(super) blacklist: bool,
}

/// One menu row or a set of adjacent rows realized from the same snapshot.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Slot {
    TradeWindow,
    ShowOnChart,
    OpenOnChart,
    NotReady,
    TradeLog {
        disabled_reason: Option<&'static str>,
    },
    OrderEdit,
    JoinSells,
    Split(i32),
    Cancel,
    ScreenerCopy,
    FilterHere,
    CoinReport,
    Compare,
    CopyTicker,
    StrategyGoto,
    StrategyReport,
    Blacklist,
    Sep,
}

/// Place each nonempty group after the preceding group, with one separator between them.
pub(super) fn plan(facts: &MenuFacts) -> Vec<Slot> {
    use Slot::*;

    let mut own = Vec::new();
    if let Some(report) = facts.report {
        own.extend([
            TradeWindow,
            if facts.has_market {
                ShowOnChart
            } else {
                NotReady
            },
            TradeLog {
                disabled_reason: report.trade_log.err(),
            },
        ]);
    } else if facts.has_market && facts.surface != CoinMenuOrigin::ChartLine {
        own.push(OpenOnChart);
    }
    match facts.order {
        Some(Order::Sell) => {
            let fixed = moon_core::config::SPLIT_ORDER_PARTS;
            own.extend([OrderEdit, JoinSells, Sep, Split(fixed)]);
            if facts.split_n != fixed {
                own.push(Split(facts.split_n));
            }
        }
        Some(Order::Buy | Order::Table) => own.extend([OrderEdit, Sep, Cancel]),
        None => {}
    }
    if facts.screener_copy {
        own.push(ScreenerCopy);
    }

    let mut coin = Vec::new();
    if facts.coin_shown {
        if facts.report.is_some_and(|report| report.filter_coin) {
            coin.push(FilterHere);
        }
        coin.push(CoinReport);
    }
    if facts.has_market {
        coin.push(Compare);
    }
    if facts.coin_shown {
        coin.push(CopyTicker);
    }

    let strategy = facts
        .strategy
        .then(|| vec![StrategyGoto, StrategyReport])
        .unwrap_or_default();
    let blacklist = facts.blacklist.then(|| vec![Blacklist]).unwrap_or_default();
    let mut slots = Vec::new();
    for group in [own, coin, strategy, blacklist] {
        if !group.is_empty() {
            if !slots.is_empty() {
                slots.push(Sep);
            }
            slots.extend(group);
        }
    }
    slots
}
