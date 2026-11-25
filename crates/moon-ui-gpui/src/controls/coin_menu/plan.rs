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
    Sell(u64),
    Buy(u64),
    Table(u64),
}

/// Captured Report actions, including the log result used by the row renderer.
#[derive(Clone, Copy)]
pub(super) struct ReportFacts<Row, Log> {
    pub(super) trade_window: Row,
    pub(super) trade_log: Log,
    pub(super) filter_coin: Option<Row>,
}

/// Captured row and market facts needed to place menu entries.
pub(super) struct MenuFacts<Row, Log> {
    pub(super) surface: CoinMenuOrigin,
    pub(super) has_market: bool,
    pub(super) coin_shown: bool,
    pub(super) strategy: Option<u64>,
    pub(super) order: Option<Order>,
    pub(super) split_n: i32,
    pub(super) report: Option<ReportFacts<Row, Log>>,
    pub(super) screener_copy: bool,
    pub(super) blacklist: bool,
}

/// One menu row or a set of adjacent rows realized from the same snapshot.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Slot<Row, Log> {
    TradeWindow(Row),
    ShowOnChart,
    OpenOnChart,
    NotReady,
    TradeLog(Log),
    OrderEdit(u64),
    JoinSells,
    Split(u64, i32),
    Cancel(u64),
    ScreenerCopy,
    FilterHere(Row),
    CoinReport,
    Compare,
    CopyTicker,
    StrategyGoto(u64),
    StrategyReport(u64),
    Blacklist,
    Sep,
}

impl<Row, Log> Slot<Row, Log> {
    /// Emphasize the action that opens the clicked trade or order for inspection.
    pub(super) fn emphasized(&self) -> bool {
        matches!(self, Self::TradeWindow(_) | Self::OrderEdit(_))
    }
}

/// Place each nonempty group after the preceding group, with one separator between them.
pub(super) fn plan<Row: Copy, Log: Copy>(facts: &MenuFacts<Row, Log>) -> Vec<Slot<Row, Log>> {
    use Slot::*;

    let mut own = Vec::new();
    if let Some(report) = facts.report {
        own.extend([
            TradeWindow(report.trade_window),
            if facts.has_market {
                ShowOnChart
            } else {
                NotReady
            },
            TradeLog(report.trade_log),
        ]);
    } else if facts.has_market && facts.surface != CoinMenuOrigin::ChartLine {
        own.push(OpenOnChart);
    }
    match facts.order {
        Some(Order::Sell(uid)) => {
            let fixed = moon_core::config::SPLIT_ORDER_PARTS;
            own.extend([OrderEdit(uid), JoinSells, Sep, Split(uid, fixed)]);
            if facts.split_n != fixed {
                own.push(Split(uid, facts.split_n));
            }
        }
        Some(Order::Buy(uid) | Order::Table(uid)) => own.extend([OrderEdit(uid), Sep, Cancel(uid)]),
        None => {}
    }
    let screener = if facts.screener_copy {
        vec![ScreenerCopy]
    } else {
        Vec::new()
    };

    let mut coin = Vec::new();
    if facts.coin_shown {
        if let Some(filter) = facts.report.and_then(|report| report.filter_coin) {
            coin.push(FilterHere(filter));
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
        .map(|sid| vec![StrategyGoto(sid), StrategyReport(sid)])
        .unwrap_or_default();
    let blacklist = if facts.blacklist {
        vec![Blacklist]
    } else {
        Vec::new()
    };
    let mut slots = Vec::new();
    for group in [own, screener, coin, strategy, blacklist] {
        if !group.is_empty() {
            if !slots.is_empty() {
                slots.push(Sep);
            }
            slots.extend(group);
        }
    }
    slots
}
