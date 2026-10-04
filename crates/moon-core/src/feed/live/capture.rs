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
//! The tracker also announces the OPEN, once, when a live upsert first completes a row's coin and
//! entry — for the tape recorder (`market::tape_recorder`), which asks the core's archive for the
//! run-up while it is still there, and for the Telegram event of the entry.
//!
//! A limit buy files its row when the order is placed, with its coin and strategy and no entry
//! stamp, and the fill comes later as a partial upsert of the stamp alone; on UDP the two may also
//! arrive the other way round. What a row carried before its entry is kept ([`Filed`]) and
//! completes the announcement on whichever upsert brings the last piece.
//!
//! A page row is remembered but never announced: a page is history, and a trade open before this
//! connection is not an entry happening now. (The open-row check after a reconnect resends such
//! rows as upserts, which this cannot tell from an entry — the listener filters them by their
//! stamp.)
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

/// Rows filed and not yet entered — buys waiting for their fill — the tracker holds; past it the
/// longest-filed goes first. A cancelled buy that never closes stays until it is pushed out.
const MAX_FILED_ROWS: usize = 4_096;

/// Field indices the capture reads: the coin token and both stamps, the millisecond columns when
/// the core reports them, and the entry's strategy and emulator flag for its announcement.
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
    pub strategy: Option<u16>,
    pub emulator: Option<u16>,
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
            strategy: field("StrategyID", Integer),
            emulator: field("Emulator", Integer),
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
    ///
    /// "First time" is per tracker, and a tracker lives one connection: after a reconnect the
    /// core's open-row check resends every open row as an upsert, and each is announced again.
    /// The listener tells a real entry by its stamp (`market::tape_recorder`).
    Opened {
        rec_id: i64,
        coin: String,
        buy: ReportStamp,
        /// The entry's strategy id; `None` when no upsert of the row carried it.
        strategy: Option<i64>,
        /// Whether the entry is an emulated one; `None` when no upsert carried the flag.
        emulator: Option<bool>,
    },
    /// The row `rec_id` closed.
    Closed(i64, ClosedTrade),
}

#[cfg(test)]
impl RowEdge {
    /// The close, when this edge is one.
    pub(super) fn closed(self) -> Option<ClosedTrade> {
        match self {
            Self::Closed(_, trade) => Some(trade),
            Self::Opened { .. } => None,
        }
    }
}

/// What a row carried before its entry: the parts of the announcement a partial upsert of the
/// fill does not repeat.
#[derive(Clone, Debug, Default)]
struct Filed {
    coin: Option<String>,
    /// The entry stamp, when a fill overtook the row's placement.
    buy: Option<ReportStamp>,
    strategy: Option<i64>,
    emulator: Option<bool>,
}

impl Filed {
    /// Take what `other` carries over what is kept.
    fn merge(&mut self, other: Filed) {
        self.coin = other.coin.or(self.coin.take());
        self.buy = other.buy.or(self.buy);
        self.strategy = other.strategy.or(self.strategy);
        self.emulator = other.emulator.or(self.emulator);
    }

    /// Whether it holds nothing.
    fn is_empty(&self) -> bool {
        self.coin.is_none()
            && self.buy.is_none()
            && self.strategy.is_none()
            && self.emulator.is_none()
    }
}

/// Per-feed memory of open rows, keyed by `rec_id`.
#[derive(Debug)]
pub(super) struct CaptureTracker {
    fields: CaptureFields,
    open: HashMap<i64, (String, ReportStamp)>,
    /// Rows seen without an entry yet, with what they carried.
    filed: HashMap<i64, Filed>,
    /// The order rows were first filed in, oldest first: who goes when [`MAX_FILED_ROWS`] is hit.
    filed_order: VecDeque<i64>,
    recent_closed: VecDeque<i64>,
}

impl CaptureTracker {
    pub(super) fn new(fields: CaptureFields) -> Self {
        Self {
            fields,
            open: HashMap::new(),
            filed: HashMap::new(),
            filed_order: VecDeque::new(),
            recent_closed: VecDeque::new(),
        }
    }

    /// Read a new schema revision's columns, keeping what was remembered: a revision appends
    /// columns and keeps the rows, so a buy filed before it is filled after it.
    pub(super) fn refield(&mut self, fields: CaptureFields) {
        self.fields = fields;
    }

    /// Feed one catch-up page row: an OPEN row that carries its coin and entry is remembered, and
    /// a buy still waiting is filed for its fill; everything else is ignored — a page is history,
    /// and its closed rows are not closes happening now.
    ///
    /// Args:
    ///     row: The row as the page carried it.
    pub(super) fn on_page_row(&mut self, row: &moonproto::ReportRow) {
        let (coin, buy, close) = self.read(row);
        match (coin, buy, close) {
            (Some(coin), Some(buy), None) => {
                self.remember_open(row.rec_id, coin, buy);
            }
            // A buy still waiting on the page: its fill may come live, as a partial upsert.
            (_, None, None) => {
                self.file(row);
            }
            _ => {}
        }
    }

    /// Feed one live upsert and get the trade back when this upsert OPENS or CLOSES one.
    ///
    /// An open row (no usable `CloseDate`) is filed until its coin and entry are both known — from
    /// this upsert or earlier ones — then remembered and announced, once. A closed row completes itself from the row first, then from
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
            // would leave an open trade behind that no close will ever come for. An open row
            // upserted again is no second entry either.
            if self.recent_closed.contains(&row.rec_id) || self.open.contains_key(&row.rec_id) {
                return None;
            }
            let filed = self.file(row);
            let (Some(buy), Some(coin)) = (buy.or(filed.buy), coin.or(filed.coin)) else {
                return None;
            };
            self.unfile(row.rec_id);
            self.remember_open(row.rec_id, coin.clone(), buy);
            return Some(RowEdge::Opened {
                rec_id: row.rec_id,
                coin,
                buy,
                strategy: filed.strategy,
                emulator: filed.emulator,
            });
        };
        // A close that overtook the last piece of the entry completes itself from what was filed.
        let filed = self
            .filed
            .get(&row.rec_id)
            .and_then(|filed| Some((filed.coin.clone()?, filed.buy?)));
        self.unfile(row.rec_id);
        let remembered = self.open.remove(&row.rec_id).or(filed);
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
        Some(RowEdge::Closed(
            row.rec_id,
            ClosedTrade { coin, buy, close },
        ))
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

    /// Keep what an open row carries towards its entry, merged over what earlier upserts carried.
    ///
    /// Returns:
    ///     Everything kept for the row now.
    fn file(&mut self, row: &moonproto::ReportRow) -> Filed {
        let f = self.fields;
        let integer = |ix: Option<u16>| match ix.and_then(|ix| row.value(ix)) {
            Some(moonproto::ReportValue::Integer(v)) => Some(*v),
            _ => None,
        };
        let carried = Filed {
            coin: match row.value(f.coin) {
                Some(moonproto::ReportValue::Text(coin)) if !coin.is_empty() => Some(coin.clone()),
                _ => None,
            },
            buy: integer(Some(f.buy_date))
                .filter(|v| *v != 0)
                .map(|secs| ReportStamp::resolve(secs, integer(f.buy_ms))),
            strategy: integer(f.strategy),
            emulator: integer(f.emulator).map(|v| v != 0),
        };
        if !self.filed.contains_key(&row.rec_id) {
            // An edit of a row this never saw open — a PnL fix of an old trade — is not filed.
            if carried.is_empty() {
                return carried;
            }
            if self.filed.len() >= MAX_FILED_ROWS {
                if let Some(oldest) = self.filed_order.pop_front() {
                    self.filed.remove(&oldest);
                }
            }
            self.filed_order.push_back(row.rec_id);
        }
        let kept = self.filed.entry(row.rec_id).or_default();
        kept.merge(carried);
        kept.clone()
    }

    /// Forget what was filed for `rec_id`: its entry came, or it closed.
    fn unfile(&mut self, rec_id: i64) {
        if self.filed.remove(&rec_id).is_some() {
            self.filed_order.retain(|id| *id != rec_id);
        }
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
