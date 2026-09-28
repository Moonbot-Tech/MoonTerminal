//! One donor: the recorder's own client of one core, in the station's mode (`STATION.md` §3.1).
//!
//! Compact history, no periodic market refresh, no strategies, and no trade subscription except
//! the pairs some trade is being recorded for. moonproto's capture-station recipe (`docs/trades.md`,
//! "Compact Capture Stations") is followed as written: select the pair with the complete set still
//! needed, `request_chart` at once, restart the pair's cursor from the oldest row after `Ready`,
//! drain the ring's new rows by cursor for as long as the pair stays selected, and forget it by
//! selecting the rest — the last one with `unsubscribe_all_trades`, because an empty selection
//! means ALL markets. No archive is asked again while a pair records.
//!
//! The terminal's own client of the same core is untouched: the subscription is per client (the
//! core shares only the trades packet), and the archive merges into this client's rings alone.
//!
//! # Is the stream alive
//!
//! A selected pair makes the core send the donor its exchange's whole trade stream, so a trades
//! packet arrives many times a second whatever the pair itself does; each is a
//! `TradesEvent::Applied`. The last one's time is the recording's frontier ([`Donor::alive_ms`]).
//! A reconnect, or a packet after more than [`STALL_MS`] of silence, starts a new stream EPOCH:
//! the rows of the gap are gone, and every recording of the old epoch has to be seeded again.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use moonproto::state::{
    MarketHistoryEvent, SeqRingCursor, SeqRingReader, TradeHistoryRow, TradesEvent,
};
use moonproto::{Event, LifecycleEvent};

use crate::config::ServerConfig;
use crate::feed::station::StationLink;
use crate::feed::{Side, Tick};
use crate::session::CoreId;

/// How long a new donor may take to finish Init before the recorder gives up on its core.
pub(super) const READY_WAIT: Duration = Duration::from_secs(60);

/// How long an asked archive may stay unanswered. moonproto re-asks every 15 s of silence on its
/// own; past this the recording starts from the live rows alone, and an answer that still comes
/// later seeds it again.
pub(super) const ANSWER_WAIT: Duration = Duration::from_secs(45);

/// A trade stream silent this long has broken rather than paused: a selected pair brings the
/// exchange's whole stream, which is never quiet for this long, and moonproto itself resubscribes
/// after 15 s without a packet.
pub(super) const STALL_MS: i64 = 10_000;

/// Rows copied per bounded drain call.
const DRAIN_BATCH: usize = 4_096;

/// What one archive request came back as.
pub(super) enum Reply {
    /// Merged into this client's ring: restart the pair's cursor from its oldest row.
    Ready(String),
    /// Rejected by the client or the core, with its reason.
    Failed(String, String),
}

/// What one drain read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Drained {
    /// The ring overwrote rows before they were read; the first row read after that is at this
    /// stamp.
    pub resumed_ms: Option<i64>,
}

/// One selected pair.
#[derive(Default)]
struct Pair {
    /// When its archive was asked, while the answer is on its way.
    asked: Option<Instant>,
    /// The ring and the cursor into it; taken from the snapshot when the recording (re)starts.
    ring: Option<(SeqRingReader<TradeHistoryRow>, SeqRingCursor)>,
}

pub(super) struct Donor {
    pub core: CoreId,
    client: StationLink,
    connected_at: Instant,
    ready_at: Option<Instant>,
    /// The selection the core was last sent.
    pairs: BTreeMap<String, Pair>,
    /// When the last trades packet arrived, true-UTC milliseconds; `None` while nothing is
    /// selected or since the stream broke.
    alive_ms: Option<i64>,
    /// [`Self::alive_ms`] of the epoch before the current one.
    previous_alive_ms: Option<i64>,
    epoch: u64,
    /// When the donor last had a pair selected, for dropping one nobody needs any more.
    pub last_used: Instant,
    events: Vec<Event>,
    lifecycle: Vec<LifecycleEvent>,
    rows: Vec<TradeHistoryRow>,
}

impl Donor {
    /// Connect a client of `server` in the station's mode. Returns at once; [`Self::pump`]
    /// notices when Init finished.
    pub(super) fn connect(server: &ServerConfig, now: Instant) -> anyhow::Result<Self> {
        let client = StationLink::connect(server)?;
        Ok(Self {
            core: server.id,
            client,
            connected_at: now,
            ready_at: None,
            pairs: BTreeMap::new(),
            alive_ms: None,
            previous_alive_ms: None,
            epoch: 0,
            last_used: now,
            events: Vec::new(),
            lifecycle: Vec::new(),
            rows: Vec::new(),
        })
    }

    /// Whether Init finished — selections and requests are sent only then.
    pub(super) fn is_ready(&self) -> bool {
        self.ready_at.is_some()
    }

    /// How long Init took, once it did.
    pub(super) fn init_took(&self) -> Option<Duration> {
        self.ready_at.map(|at| at.duration_since(self.connected_at))
    }

    /// Init has not finished within [`READY_WAIT`].
    pub(super) fn stalled(&self, now: Instant) -> bool {
        self.ready_at.is_none() && now.duration_since(self.connected_at) > READY_WAIT
    }

    /// Archives asked and not answered.
    pub(super) fn in_flight(&self) -> usize {
        self.pairs.values().filter(|p| p.asked.is_some()).count()
    }

    /// Pairs selected.
    pub(super) fn selected(&self) -> usize {
        self.pairs.len()
    }

    /// The current stream epoch — see the module header.
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// When the stream of `epoch` was last seen alive: the current one's last packet, or the
    /// previous one's, for filing what it brought before it broke.
    pub(super) fn alive_ms(&self, epoch: u64) -> Option<i64> {
        if epoch == self.epoch {
            self.alive_ms
        } else if epoch.saturating_add(1) == self.epoch {
            self.previous_alive_ms
        } else {
            None
        }
    }

    fn next_epoch(&mut self) {
        self.previous_alive_ms = self.alive_ms.take();
        self.epoch += 1;
    }

    /// Drain everything the client queued — most of it (orders, balances, logs the core sends
    /// every client) is dropped here — note the stream's pulse, and collect the archive answers
    /// into `replies`.
    pub(super) fn pump(&mut self, now: Instant, now_ms: i64, replies: &mut Vec<Reply>) {
        let mut lifecycle = std::mem::take(&mut self.lifecycle);
        lifecycle.clear();
        self.client.drain_lifecycle_events_into(&mut lifecycle);
        for event in &lifecycle {
            match event {
                LifecycleEvent::Ready if self.ready_at.is_none() => self.ready_at = Some(now),
                // A reconnect: whatever the stream sent while the link was down is gone.
                LifecycleEvent::Connected { fresh: false } => self.next_epoch(),
                _ => {}
            }
        }
        self.lifecycle = lifecycle;
        let mut events = std::mem::take(&mut self.events);
        events.clear();
        self.client.drain_events_into(&mut events);
        for event in events.drain(..) {
            match event {
                Event::MarketHistory(MarketHistoryEvent::Ready { ticket, .. }) => {
                    if let Some(pair) = self.pairs.get_mut(ticket.market.as_str()) {
                        pair.asked = None;
                        replies.push(Reply::Ready(ticket.market.clone()));
                    }
                }
                Event::MarketHistory(MarketHistoryEvent::Failed { ticket, error }) => {
                    if let Some(pair) = self.pairs.get_mut(ticket.market.as_str()) {
                        pair.asked = None;
                        replies.push(Reply::Failed(ticket.market.clone(), error.to_string()));
                    }
                }
                // Packets still in flight after the last pair was let go are not a pulse.
                Event::Trade(TradesEvent::Applied { .. }) if !self.pairs.is_empty() => {
                    if self.alive_ms.is_some_and(|at| now_ms - at > STALL_MS) {
                        self.next_epoch();
                    }
                    self.alive_ms = Some(now_ms);
                }
                _ => {}
            }
        }
        self.events = events;
    }

    /// Send the complete set of pairs still needed, when it changed. A pair left out is
    /// forgotten with its ring; a pair added starts with an empty one.
    pub(super) fn select(
        &mut self,
        markets: &BTreeSet<String>,
        now: Instant,
    ) -> Result<(), String> {
        if self.pairs.keys().eq(markets.iter()) {
            return Ok(());
        }
        self.client
            .select_pairs(markets.iter().map(String::as_str))?;
        self.pairs.retain(|market, _| markets.contains(market));
        for market in markets {
            self.pairs.entry(market.clone()).or_default();
        }
        if markets.is_empty() {
            // The stream stops because it was let go, not because it broke.
            self.alive_ms = None;
        } else {
            self.last_used = now;
        }
        Ok(())
    }

    /// Request the archive of a selected `market`.
    pub(super) fn ask(&mut self, market: &str, now: Instant) -> Result<(), String> {
        let Some(pair) = self.pairs.get_mut(market) else {
            return Err("not selected".to_string());
        };
        self.client.request_chart(market)?;
        pair.asked = Some(now);
        self.last_used = now;
        Ok(())
    }

    /// Pairs whose archive was asked longer than [`ANSWER_WAIT`] ago; each is reported once.
    pub(super) fn expired(&mut self, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        for (market, pair) in &mut self.pairs {
            if pair
                .asked
                .is_some_and(|at| now.duration_since(at) > ANSWER_WAIT)
            {
                pair.asked = None;
                out.push(market.clone());
            }
        }
        out
    }

    /// Point the pair's cursor at the oldest row its ring holds — after the archive merged, or
    /// when the recording starts without one.
    pub(super) fn restart(&mut self, market: &str) {
        let ring = self.client.trades_ring(market).map(|reader| {
            let cursor = reader.cursor_from_oldest();
            (reader, cursor)
        });
        if let Some(pair) = self.pairs.get_mut(market) {
            pair.ring = ring;
        }
    }

    /// Append every row of `market`'s ring past its cursor to `out`, converted exactly as the
    /// terminal's own ring copy converts it (`market::source::replay`), so the two can be compared
    /// print for print. A pair whose ring was not found yet looks for it again, from its oldest
    /// row.
    pub(super) fn drain(&mut self, market: &str, out: &mut Vec<Tick>) -> Drained {
        let found = self.pairs.get(market).is_some_and(|p| p.ring.is_some());
        if !found {
            self.restart(market);
        }
        let Some((reader, cursor)) = self.pairs.get_mut(market).and_then(|p| p.ring.as_mut())
        else {
            return Drained::default();
        };
        let mut drained = Drained::default();
        loop {
            let meta = reader.drain_new_bounded(cursor, DRAIN_BATCH, &mut self.rows);
            // Taken from the raw row: a first row the filter below drops still marks where the
            // read resumed.
            if meta.clipped && drained.resumed_ms.is_none() {
                drained.resumed_ms = self.rows.first().map(|row| row.unix_millis());
            }
            for row in &self.rows {
                let tick = Tick {
                    time_ms: row.unix_millis() as f64,
                    price: row.price,
                    qty: row.quantity(),
                    side: if row.is_buy() { Side::Buy } else { Side::Sell },
                };
                if tick.time_ms.is_finite() && tick.price.is_finite() && tick.price > 0.0 {
                    out.push(tick);
                }
            }
            if meta.caught_up || meta.copied == 0 {
                return drained;
            }
        }
    }
}
