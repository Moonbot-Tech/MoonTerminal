//! One donor: the recorder's own client of one core, in the station's mode (`STATION.md` §3.1).
//!
//! Compact history, no periodic market refresh, no strategies, and no trade subscription except
//! the pairs whose archive is on its way. moonproto's capture-station recipe (`docs/trades.md`,
//! "Compact Capture Stations") is followed as written: select the pair, `request_chart` at once,
//! copy the ring after `Ready`, forget the pair — the remaining pairs re-selected, and the last one
//! with `unsubscribe_all_trades`, because an empty selection means ALL markets.
//!
//! The terminal's own client of the same core is untouched: the subscription is per client (the
//! core shares only the trades packet), and the archive merges into this client's rings alone.

use std::time::{Duration, Instant};

use moonproto::state::{MarketHistoryEvent, MarketHistorySizing};
use moonproto::{
    ClientConfig, ConnectConfig, Event, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
    RefreshConfig, TradesStreamMode,
};

use crate::config::ServerConfig;
use crate::feed::{Side, Tick};
use crate::session::CoreId;

/// How long a new donor may take to finish Init before the recorder gives up on its core.
pub(super) const READY_WAIT: Duration = Duration::from_secs(60);

/// How long an asked archive may stay unanswered. moonproto re-asks every 15 s of silence on its
/// own and never gives up; past this the pair is forgotten, which cancels those retries.
pub(super) const ANSWER_WAIT: Duration = Duration::from_secs(45);

/// What one archive request came back as.
pub(super) enum Reply {
    /// Merged into this client's rings: copy now, then forget the pair.
    Ready(String),
    /// Rejected by the client or the core, with its reason.
    Failed(String, String),
}

pub(super) struct Donor {
    pub core: CoreId,
    client: MoonClient,
    connected_at: Instant,
    ready_at: Option<Instant>,
    /// Pairs whose archive is on its way, with when each was asked — the retained selection.
    asked: Vec<(String, Instant)>,
    /// When the donor last had a pair asked, for dropping one nobody needs any more.
    pub last_used: Instant,
    events: Vec<Event>,
    lifecycle: Vec<LifecycleEvent>,
}

impl Donor {
    /// Connect a client of `server` in the station's mode. Returns at once; [`Self::pump`]
    /// notices when Init finished.
    pub(super) fn connect(server: &ServerConfig, now: Instant) -> anyhow::Result<Self> {
        let info = moonproto::parse_key_info(server.key.expose())
            .ok_or_else(|| anyhow::anyhow!("key unreadable"))?;
        let (endpoint, transport) =
            crate::feed::live::connection_target(info.network.as_ref(), server.transport);
        let cfg = ClientConfig::new(
            endpoint.address.to_string(),
            endpoint.port,
            info.keys.master_key,
            info.keys.mac_key,
        )
        .with_transport_mode(transport)
        .with_market_history(MarketHistorySizing::Compact)
        .with_refresh(RefreshConfig {
            update_markets_every: None,
            check_tags_every: None,
        });
        // No subscriptions at Init; the strategies list is required or Init never completes.
        let init = InitConfig {
            initial_strategies: Some(InitialStrategies::new(0, Vec::new())),
            ..Default::default()
        };
        let client = MoonClient::connect(
            cfg,
            ConnectConfig::new(init).with_connect_timeout(Duration::from_secs(15)),
        )?;
        Ok(Self {
            core: server.id,
            client,
            connected_at: now,
            ready_at: None,
            asked: Vec::new(),
            last_used: now,
            events: Vec::new(),
            lifecycle: Vec::new(),
        })
    }

    /// Whether Init finished — requests are sent only then.
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

    pub(super) fn in_flight(&self) -> usize {
        self.asked.len()
    }

    pub(super) fn is_asking(&self, market: &str) -> bool {
        self.asked.iter().any(|(m, _)| m == market)
    }

    /// Drain everything the client queued — most of it (orders, balances, logs the core sends
    /// every client) is dropped here — and collect the archive answers into `replies`.
    pub(super) fn pump(&mut self, now: Instant, replies: &mut Vec<Reply>) {
        self.lifecycle.clear();
        self.client.drain_lifecycle_events_into(&mut self.lifecycle);
        if self
            .lifecycle
            .iter()
            .any(|e| matches!(e, LifecycleEvent::Ready))
            && self.ready_at.is_none()
        {
            self.ready_at = Some(now);
        }
        self.events.clear();
        self.client.drain_events_into(&mut self.events);
        for event in self.events.drain(..) {
            match event {
                Event::MarketHistory(MarketHistoryEvent::Ready { ticket, .. }) => {
                    replies.push(Reply::Ready(ticket.market.clone()));
                }
                Event::MarketHistory(MarketHistoryEvent::Failed { ticket, error }) => {
                    replies.push(Reply::Failed(ticket.market.clone(), error.to_string()));
                }
                _ => {}
            }
        }
    }

    /// Select `market` alongside the pairs already asked and request its archive.
    pub(super) fn ask(&mut self, market: &str, now: Instant) -> Result<(), String> {
        self.asked.push((market.to_string(), now));
        self.last_used = now;
        if let Err(e) = self.select() {
            self.asked.pop();
            let _ = self.select();
            return Err(e);
        }
        match self.client.history().request_chart(market) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.forget(market);
                Err(e.to_string())
            }
        }
    }

    /// Pairs asked longer than [`ANSWER_WAIT`] ago.
    pub(super) fn expired(&self, now: Instant) -> Vec<String> {
        self.asked
            .iter()
            .filter(|(_, at)| now.duration_since(*at) > ANSWER_WAIT)
            .map(|(market, _)| market.clone())
            .collect()
    }

    /// Every pair still asked, for a caller dropping this donor.
    pub(super) fn asked_markets(&self) -> Vec<String> {
        self.asked
            .iter()
            .map(|(market, _)| market.clone())
            .collect()
    }

    /// Stop retaining `market`: re-select the rest, or unsubscribe when nothing is left.
    pub(super) fn forget(&mut self, market: &str) {
        self.asked.retain(|(m, _)| m != market);
        if let Err(e) = self.select() {
            log::warn!(
                "tape recorder: core {} selection update failed: {e}",
                crate::feed::core_label(self.core)
            );
        }
    }

    fn select(&self) -> Result<(), String> {
        let result = match self.asked.is_empty() {
            true => self.client.streams().unsubscribe_all_trades(),
            false => self.client.streams().subscribe_trades_for(
                TradesStreamMode::TradesOnly,
                self.asked.iter().map(|(market, _)| market.as_str()),
            ),
        };
        result.map_err(|e| e.to_string())
    }

    /// Everything the ring of `market` holds, ascending — converted exactly as the terminal's
    /// own ring copy converts it (`market::source::replay`), so the two can be compared print for
    /// print.
    pub(super) fn copy_ring(&self, market: &str) -> Vec<Tick> {
        let Some(readers) = self
            .client
            .snapshot_versioned()
            .and_then(|snapshot| snapshot.market_history_readers(market))
        else {
            return Vec::new();
        };
        let Some(reader) = readers.futures_trades.or(readers.spot_trades) else {
            return Vec::new();
        };
        let mut ticks = reader.with_last(reader.capacity(), |view| {
            let mut out = Vec::new();
            view.for_each(|row| {
                let tick = Tick {
                    time_ms: row.unix_millis() as f64,
                    price: row.price,
                    qty: row.quantity(),
                    side: if row.is_buy() { Side::Buy } else { Side::Sell },
                };
                if tick.time_ms.is_finite() && tick.price.is_finite() && tick.price > 0.0 {
                    out.push(tick);
                }
            });
            out
        });
        ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        ticks
    }
}
