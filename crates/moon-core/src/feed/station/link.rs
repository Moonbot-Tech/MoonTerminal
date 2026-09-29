//! `StationLink`: the one `MoonClient` a station component owns, narrowed to what the station is
//! allowed to ask a core (`docs-internal/STATION.md` §3.1, level 3).
//!
//! The tape recorder's donors are built on it: a client in the station's mode — Compact
//! history, no periodic market refresh, no subscription at Init — that can select the pairs of
//! trades in progress, ask their chart archive, read their rings and drain its events. It cannot
//! subscribe to every market (`subscribe_all_trades` is not reachable through it), send a
//! trading command, or touch a core's settings or strategies: those methods do not exist here,
//! and `crates/moon-station/tests/station_contract.rs` fails the build if a station component
//! names `MoonClient` instead.
//!
//! The report replica, the order traces and the core's identity still reach the station through
//! the terminal's own feed (`feed::live`) in station mode ([`super::Profile::keeps`]); they move
//! behind a link of this kind when the station gets a core link of its own (phases 2–3).

use std::time::Duration;

use moonproto::state::{MarketHistorySizing, SeqRingReader, TradeHistoryRow};
use moonproto::{
    ClientConfig, ConnectConfig, Event, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
    RefreshConfig, TradesStreamMode,
};

use crate::config::ServerConfig;

/// How long the transport handshake may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A station-mode client of one core.
pub struct StationLink {
    client: MoonClient,
}

impl StationLink {
    /// Connect to `server` in the station's mode. Returns at once; Init finishes in the
    /// background and announces itself as [`LifecycleEvent::Ready`].
    pub fn connect(server: &ServerConfig) -> anyhow::Result<Self> {
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
            ConnectConfig::new(init).with_connect_timeout(CONNECT_TIMEOUT),
        )?;
        Ok(Self { client })
    }

    /// Select exactly `markets` for the trade stream, replacing the previous selection. An
    /// empty set unsubscribes — an empty `subscribe_trades_for` would mean ALL markets.
    pub fn select_pairs<'a>(
        &self,
        markets: impl ExactSizeIterator<Item = &'a str>,
    ) -> Result<(), String> {
        let result = match markets.len() {
            0 => self.client.streams().unsubscribe_all_trades(),
            _ => self
                .client
                .streams()
                .subscribe_trades_for(TradesStreamMode::TradesOnly, markets),
        };
        result.map_err(|e| e.to_string())
    }

    /// Ask the chart archive of a selected market; the answer arrives as an
    /// `Event::MarketHistory`.
    pub fn request_chart(&self, market: &str) -> Result<(), String> {
        self.client
            .history()
            .request_chart(market)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The retained trade ring of a selected market — the futures tape, or the spot one.
    pub fn trades_ring(&self, market: &str) -> Option<SeqRingReader<TradeHistoryRow>> {
        let readers = self
            .client
            .snapshot_versioned()?
            .market_history_readers(market)?;
        readers.futures_trades.or(readers.spot_trades)
    }

    /// Move every queued event into `out`.
    pub fn drain_events_into(&self, out: &mut Vec<Event>) {
        self.client.drain_events_into(out);
    }

    /// Move every queued lifecycle event into `out`.
    pub fn drain_lifecycle_events_into(&self, out: &mut Vec<LifecycleEvent>) {
        self.client.drain_lifecycle_events_into(out);
    }
}
