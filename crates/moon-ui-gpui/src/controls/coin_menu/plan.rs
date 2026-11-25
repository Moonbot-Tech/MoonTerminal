//! Decide which shared coin-menu rows appear and in what order.

use moon_core::feed::ForceCancelGate;

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
    /// Whether the clicked order gets a forced cancellation, decided by
    /// `CoreFeatures::force_cancel_gate` from its status and the core's build. Read only when
    /// [`Self::order`] is set.
    pub(super) force_cancel: ForceCancelGate,
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
    /// The forced cancellation of a stuck order, opening its confirmation.
    ForceCancel(u64),
    /// The same row, disabled, on a core that has not proved it is 7.73 N2+.
    ForceCancelNeedsCore,
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

/// The force-cancel row for `uid` under `gate`, or `None` when the order has no active leg.
fn force_cancel_slot<Row, Log>(gate: ForceCancelGate, uid: u64) -> Option<Slot<Row, Log>> {
    match gate {
        ForceCancelGate::NotEligible => None,
        ForceCancelGate::NeedsNewerCore => Some(Slot::ForceCancelNeedsCore),
        ForceCancelGate::Offered => Some(Slot::ForceCancel(uid)),
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
            // A sell line has no ordinary Cancel, so the forced one stands behind its own
            // separator instead of reading as a third split.
            if let Some(slot) = force_cancel_slot(facts.force_cancel, uid) {
                own.extend([Sep, slot]);
            }
        }
        Some(Order::Buy(uid) | Order::Table(uid)) => {
            own.extend([OrderEdit(uid), Sep, Cancel(uid)]);
            // Directly under the ordinary Cancel: the two are read as one choice.
            own.extend(force_cancel_slot(facts.force_cancel, uid));
        }
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
