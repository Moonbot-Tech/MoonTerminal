//! What one feed remembers of its report rows so a CLOSING upsert can be turned into a print
//! capture — see `market::trade_replay::worker::capture`.
//!
//! A live `RowUpsert` is partial: it carries the fields that changed, and the core has no reason
//! to resend a coin or an entry stamp set at the open when the exit arrives (`db::rep`,
//! `RowSource::Live`). So the close cannot be read off its own upsert alone. This tracker keeps,
//! per open row, the two things the close will not repeat — the coin token and the entry stamp —
//! from whichever row first carried them: the upsert that opened the trade, or a catch-up page
//! row, which carries the core's whole column set. The upsert that closes the row completes
//! itself from that memory, and the row is forgotten.
//!
//! The tracker also announces the OPEN, once, when a live upsert first carries a row's coin and
//! entry — for the tape recorder (`market::tape_recorder`), which asks the core's archive for the
//! run-up while it is still there. A page row is remembered but never announced: a page is history,
//! and a trade open before this connection is not an entry happening now.
//!
//! One announcement per row: a closed row upserted again (a later PnL or comment edit carries
//! `CloseDate` too) is recognised by its `rec_id` and not announced twice, so the worker never
//! copies the same span a second time on the core's account.

use std::collections::{HashMap, VecDeque};

use crate::db::ReportStamp;

/// Open rows the tracker may hold before it forgets them all and starts over.
///
/// A row leaves on its close, so this is the number of simultaneously OPEN trades — thousands
/// only on a core that never closes anything, where the memory is worth nothing anyway.
const MAX_OPEN_ROWS: usize = 20_000;

/// Closed `rec_id`s remembered against a second closing upsert.
const MAX_RECENT_CLOSED: usize = 512;

/// Field indices the capture reads: the coin token and both stamps, the millisecond columns when
/// the core reports them.
///
/// Resolved once per schema revision; names are matched without case because the replica files
/// them lowercased while the wire spells them `Coin`, `BuyDate`.
#[derive(Clone, Copy, Debug)]
pub(super) struct CaptureFields {
    pub coin: u16,
    pub buy_date: u16,
    pub buy_ms: Option<u16>,
    pub close_date: u16,
    pub close_ms: Option<u16>,
}

impl CaptureFields {
    pub(super) fn from_schema(schema: &moonproto::ReportSchema) -> Option<Self> {
        let field = |name: &str, kind: moonproto::ReportFieldKind| {
            schema
                .fields()
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(name) && f.kind == kind)
                .map(|f| f.index)
        };
        use moonproto::ReportFieldKind::{Integer, Text};
        Some(Self {
            coin: field("Coin", Text)?,
            buy_date: field("BuyDate", Integer)?,
            buy_ms: field("BuyDateMs", Integer),
            close_date: field("CloseDate", Integer)?,
            close_ms: field("CloseDateMs", Integer),
        })
    }
}

/// A trade whose close was just announced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClosedTrade {
    pub coin: String,
    pub buy: ReportStamp,
    pub close: ReportStamp,
}

/// What one live upsert meant for its trade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RowEdge {
    /// A row this feed had not seen open carried its coin and entry for the first time.
    Opened { coin: String, buy: ReportStamp },
    /// The row closed.
    Closed(ClosedTrade),
}

#[cfg(test)]
impl RowEdge {
    /// The close, when this edge is one.
    pub(super) fn closed(self) -> Option<ClosedTrade> {
        match self {
            Self::Closed(trade) => Some(trade),
            Self::Opened { .. } => None,
        }
    }
}

/// Per-feed memory of open rows, keyed by `rec_id`.
#[derive(Debug)]
pub(super) struct CaptureTracker {
    fields: CaptureFields,
    open: HashMap<i64, (String, ReportStamp)>,
    recent_closed: VecDeque<i64>,
}

impl CaptureTracker {
    pub(super) fn new(fields: CaptureFields) -> Self {
        Self {
            fields,
            open: HashMap::new(),
            recent_closed: VecDeque::new(),
        }
    }

    /// Feed one catch-up page row: an OPEN row that carries its coin and entry is remembered,
    /// everything else is ignored — a page is history, and its closed rows are not closes
    /// happening now.
    ///
    /// Args:
    ///     row: The row as the page carried it.
    pub(super) fn on_page_row(&mut self, row: &moonproto::ReportRow) {
        let (coin, buy, close) = self.read(row);
        if let (Some(coin), Some(buy), None) = (coin, buy, close) {
            self.remember_open(row.rec_id, coin, buy);
        }
    }

    /// Feed one live upsert and get the trade back when this upsert OPENS or CLOSES one.
    ///
    /// An open row (no usable `CloseDate`) that carries its coin and entry is remembered, and
    /// announced the first time. A closed row completes itself from the row first, then from
    /// memory, and is announced once; a closed row this feed cannot complete — closed before the
    /// terminal ever saw it open, and not on any page it received — is not a trade this can
    /// locate, and is skipped.
    ///
    /// Args:
    ///     row: The row as the core sent it.
    ///
    /// Returns:
    ///     The open, on the first upsert that carries it; the close, on the one upsert that closes
    ///     the trade and can be completed.
    pub(super) fn on_row(&mut self, row: &moonproto::ReportRow) -> Option<RowEdge> {
        let (coin, buy, close) = self.read(row);
        let Some(close) = close else {
            // A just-closed row upserted again without its CloseDate is no entry: announced, it
            // would leave an open trade behind that no close will ever come for.
            let (Some(coin), Some(buy), false) =
                (coin, buy, self.recent_closed.contains(&row.rec_id))
            else {
                return None;
            };
            return self
                .remember_open(row.rec_id, coin.clone(), buy)
                .then_some(RowEdge::Opened { coin, buy });
        };
        let remembered = self.open.remove(&row.rec_id);
        if self.recent_closed.contains(&row.rec_id) {
            return None;
        }
        let (coin, buy) = match (coin, buy, remembered) {
            (Some(coin), Some(buy), _) => (coin, buy),
            (coin, buy, Some((old_coin, old_buy))) => {
                (coin.unwrap_or(old_coin), buy.unwrap_or(old_buy))
            }
            _ => return None,
        };
        self.recent_closed.push_back(row.rec_id);
        while self.recent_closed.len() > MAX_RECENT_CLOSED {
            self.recent_closed.pop_front();
        }
        Some(RowEdge::Closed(ClosedTrade { coin, buy, close }))
    }

    /// The coin and both stamps a row carries, each `None` when absent or zero.
    ///
    /// Core-local stamps, resolved per column exactly as the replica reads them back
    /// (`ReportStamp::resolve`): a positive millisecond value wins, else the seconds column.
    fn read(
        &self,
        row: &moonproto::ReportRow,
    ) -> (Option<String>, Option<ReportStamp>, Option<ReportStamp>) {
        let f = self.fields;
        let integer = |ix: u16| match row.value(ix) {
            Some(moonproto::ReportValue::Integer(v)) => Some(*v),
            _ => None,
        };
        let coin = match row.value(f.coin) {
            Some(moonproto::ReportValue::Text(coin)) if !coin.is_empty() => Some(coin.clone()),
            _ => None,
        };
        let buy = integer(f.buy_date)
            .filter(|v| *v != 0)
            .map(|secs| ReportStamp::resolve(secs, f.buy_ms.and_then(integer)));
        let close = integer(f.close_date)
            .filter(|v| *v != 0)
            .map(|secs| ReportStamp::resolve(secs, f.close_ms.and_then(integer)));
        (coin, buy, close)
    }

    /// Remember what a close will not repeat. Whether the row was not remembered before.
    fn remember_open(&mut self, rec_id: i64, coin: String, buy: ReportStamp) -> bool {
        if self.open.len() >= MAX_OPEN_ROWS {
            self.open.clear();
        }
        self.open.insert(rec_id, (coin, buy)).is_none()
    }
}

#[cfg(test)]
mod tests;
