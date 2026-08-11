//! Live backend that owns one Moonbot core connection and its MoonProtoBeta event loop.
//!
//! Flow: event-driven. `MoonEventSink` wakes the backend thread after an actual event; market data
//! remains in an immutable read-model snapshot, and only a lightweight signal reaches this module.
//!
//! `run()` is the main event loop; commands live in [`commands`], persistent market assignment in
//! [`market_role`], pure moonproto-to-terminal converters in [`convert`], and dirty-market
//! calculation in [`dirty`].

mod account_reconciliation;
mod archive_probe;
mod client_settings;
mod commands;
mod convert;
mod dirty;
mod market_role;
#[cfg(test)]
mod tests;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::{sync_channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use moonproto::state::{AccountEvent, MarketHistorySizing, OrderEvent, SettingsEvent};
use moonproto::{
    ClientConfig, ConnectConfig, Event, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
    MoonEventSink, ReportAliveMapOutcome, ReportAliveMapTicket, ReportEvent, ReportHistoryDepth,
    ReportSyncCheckpoint, ReportSyncComplete, ReportSyncRequest, TransportMode,
};

use super::assets::{build_assets, build_transfer_assets};
use super::strategies::{
    alert_params, build_schema_model, fmt_field, schema_default_fields, strat_db_dump,
    strat_kind_name,
};
use super::{
    ConnStatus, CoreCmd, CoreEndpoint, CoreLogLine, DetectRow, ExchangeId, FeedMsg, FeedTx,
    LatestMarketRole, SharedMoonClient, StrategyRow,
};
use crate::config::ServerConfig;
use crate::db::{DbMsg, ReportStart, ReportTx};
use crate::util::{now_unix_ms as now_ms, now_unix_ms_i64 as now_ms_i64};

/// Return whether one normalized strategy generation still needs database delivery.
fn strategy_db_export_due(
    schema_ready: bool,
    schema_revision: u64,
    strategy_signature: u64,
    delivered: Option<(u64, u64)>,
) -> bool {
    schema_ready && Some((schema_revision, strategy_signature)) != delivered
}

/// Apply one writer acknowledgement without consuming a failed strategy generation.
fn apply_strategy_delivery_ack(
    generation: (u64, u64),
    committed: bool,
    delivered: &mut Option<(u64, u64)>,
    retry_due: &mut bool,
    initial: &mut bool,
) {
    *retry_due = !committed;
    if committed {
        *delivered = Some(generation);
        *initial = false;
    }
}

use account_reconciliation::AccountReconciliation;
pub(in crate::feed) use client_settings::ClientSettingsSequence;
use commands::{drain_commands, CommandDrain, LocalStratEdits, StrategyPlacementGuard};
use convert::{
    build_order_rows, client_settings_from_proto, lev_manage_from_proto, license_state_from_proto,
    runtime_state_from_proto, settings_event_snapshot, sys_status_from_proto,
};
use dirty::market_dirty_from_events;
pub(in crate::feed) use market_role::MarketRoleState;

/// What to do with an arriving report alive map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AliveAction {
    /// Route the map and checkpoint for atomic application by the writer.
    Apply(ReportSyncCheckpoint),
    /// The core serves another report database: wipe the replica and sync from zero.
    Wipe,
    /// Drop the map, naming why for the log.
    Ignore(&'static str),
}

/// Decide what an arriving alive map may do, given the catch-up this feed asked it to reconcile.
///
/// Every rejection here protects the checkpoint. A map is authoritative over
/// `1..=covered_up_to`, so applying one that does not describe the catch-up whose checkpoint is
/// about to be stored would hide live rows and then record the damage as reconciled. The ticket
/// match matters because a second `SyncComplete` replaces the pending pair — and the library
/// likewise replaces its active request — so a late map from the previous pass must be dropped,
/// not applied against the newer checkpoint. A matching `DatabaseRecreated` result bypasses the
/// epoch and coverage comparison because its purpose is to report that those values cannot agree.
fn alive_map_action(
    pending: Option<&(ReportAliveMapTicket, ReportSyncComplete)>,
    ticket: ReportAliveMapTicket,
    epoch: i32,
    covered_up_to: i64,
    outcome: ReportAliveMapOutcome,
) -> AliveAction {
    let Some((wanted, done)) = pending else {
        return AliveAction::Ignore("карта не запрашивалась");
    };
    if *wanted != ticket {
        return AliveAction::Ignore("карта от другого запроса");
    }
    if outcome == ReportAliveMapOutcome::DatabaseRecreated {
        return AliveAction::Wipe;
    }
    if epoch != done.epoch || covered_up_to != done.max_rec_id {
        return AliveAction::Ignore("карта описывает другой catch-up");
    }
    AliveAction::Apply(done.checkpoint())
}

struct ClientSlotGuard {
    slot: SharedMoonClient,
}

impl Drop for ClientSlotGuard {
    fn drop(&mut self) {
        self.slot.set(None);
    }
}

/// Resolve the connection endpoint and transport carried by a parsed MoonBot key.
///
/// Args:
///     network: Optional network metadata from `moonproto::parse_key_info`.
///
/// Returns:
///     Decoded endpoint plus transport, with the same localhost/3000/V0 fallbacks used for legacy
///     exports.
fn connection_target(
    network: Option<&moonproto::ImportedNetworkConfig>,
) -> (CoreEndpoint, TransportMode) {
    let address = network
        .and_then(|network| network.address)
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let port = network
        .map(|network| network.port)
        .filter(|port| *port != 0)
        .unwrap_or(3000);
    let transport = network
        .map(|network| network.transport_mode)
        .unwrap_or(TransportMode::V0);
    (CoreEndpoint { address, port }, transport)
}

/// Run one core's live MoonProto event loop until shutdown or a terminal connection error.
///
/// Publishes account snapshots and lifecycle state through `tx`, consumes commands from `cmd_rx`,
/// exposes the active client through `client_slot`, and applies the retained `market_role` for this
/// connection attempt. Returns an error when setup or the live loop cannot continue; dropping the
/// internal guard clears the shared client slot.
///
/// Args:
///     server: Core configuration containing the exported MoonBot key.
///     chart_memory_percent: Retained market-history budget for the MoonProto client.
///     tx: Account-plane channel, including the decoded endpoint and lifecycle updates.
///     cmd_rx: Commands directed to this core.
///     wake_tx: Coordinator wake sender.
///     wake_rx: Feed wake receiver.
///     reports: Optional report-database channel.
///     client_slot: Shared active-client slot cleared when this attempt ends.
///     client_settings_sequence: Reconnect-safe manual-settings sequence.
///     market_role: Market-provider role retained between attempts.
///     latest_market_role: Latest successfully queued role, independent of the bounded backlog.
///
/// Returns:
///     Success after orderly shutdown, or the terminal setup/live-loop error.
///
/// Errors:
///     Returns an error when the key is invalid, the client cannot connect, or the event loop fails.
pub(super) fn run(
    server: &ServerConfig,
    chart_memory_percent: u16,
    tx: &FeedTx,
    cmd_rx: &Receiver<CoreCmd>,
    wake_tx: &Sender<()>,
    wake_rx: &Receiver<()>,
    reports: Option<&ReportTx>,
    client_slot: SharedMoonClient,
    client_settings_sequence: &mut ClientSettingsSequence,
    market_role: &mut MarketRoleState,
    latest_market_role: &LatestMarketRole,
) -> anyhow::Result<()> {
    let _ = tx.send(FeedMsg::Status(ConnStatus::Connecting));
    market_role.begin_client();

    // 1. Decode the key into master/MAC keys and its suggested network.
    let info = moonproto::parse_key_info(server.key.expose())
        .ok_or_else(|| anyhow::anyhow!("не удалось разобрать ключ Moonbot (server.key)"))?;

    // 2. Derive the endpoint from the key, which embeds host/port/transport; the config no longer
    //    has separate fields for them.
    let (endpoint, transport) = connection_target(info.network.as_ref());
    let address = endpoint.address;
    let port = endpoint.port;
    let host = address.to_string();
    let _ = tx.send(FeedMsg::Endpoint(endpoint));
    // Name the core, never its address: the endpoint still reaches the UI through `FeedMsg::
    // Endpoint` for Core Status, but a log file is shared far more casually than a screen is.
    log::info!(
        "live connect core={} market={} transport={transport:?}",
        server.name,
        server.market
    );

    let client_cfg = ClientConfig::new(host, port, info.keys.master_key, info.keys.mac_key)
        .with_transport_mode(transport)
        .with_market_history(MarketHistorySizing::auto_with_budget_percent(
            chart_memory_percent,
        ));

    // 3. Initialize WITHOUT market subscriptions. The coordinator assigns the core's market role
    //    via SetMarket after learning its exchange (Identity) and electing a provider. Only ONE
    //    core per exchange calls subscribe_all_trades; the others publish account data only. This
    //    fetches exchange trades once instead of once from each of up to 200 cores.
    //    initial_strategies IS REQUIRED, or initialization hangs after Connected.
    let init = InitConfig {
        initial_strategies: Some(InitialStrategies::new(0, Vec::new())),
        ..Default::default()
    };

    // `connect` is nonblocking; add connect_timeout so a stuck initialization step arrives as
    // ConnectFailed with a reason instead of remaining silent.
    let event_wake_tx = wake_tx.clone();
    let (event_sink, event_queue) = MoonEventSink::queue_with_waker(move || {
        let _ = event_wake_tx.send(());
    });
    let client = Arc::new(MoonClient::connect_with_sink(
        client_cfg,
        ConnectConfig::new(init).with_connect_timeout(Duration::from_secs(15)),
        event_sink,
    )?);
    client_slot.set(Some(client.clone()));
    let _client_slot_guard = ClientSlotGuard {
        slot: client_slot.clone(),
    };

    // Typed report-database replica: declare catch-up immediately. It remains an intent that the
    // library sends after connection and repeats on its own after a hard reconnect. Where it
    // starts comes from the writer's durable start state: a checkpoint pairs the numeric cursor
    // with the core's database epoch, so a replaced core database is detected even when the new
    // one already grew past the old ids. Catch-up is PAGED: the next page is not requested until
    // the writer commits and acknowledges the current one (backpressure by design).
    if server.feed.reports {
        if let Some(sink) = reports {
            let start = sink.next_start(server.uid);
            let started = match start {
                ReportStart::Fresh => client
                    .reports()
                    .sync(ReportSyncRequest::fresh(ReportHistoryDepth::All))
                    .map(|_| "fresh(All)".to_string()),
                ReportStart::Resume(from) => client
                    .reports()
                    .sync(ReportSyncRequest::resume(from))
                    .map(|_| format!("resume(from_rec_id={from}) — миграция без epoch")),
                ReportStart::Checkpoint(checkpoint) => {
                    client.reports().sync_from(checkpoint).map(|_| {
                        format!(
                            "sync_from(epoch={}, from_rec_id={})",
                            checkpoint.epoch, checkpoint.next_from_rec_id
                        )
                    })
                }
            };
            match started {
                Ok(how) => log::info!(
                    "отчёты: core={} «{}» sync запрошен: {how}",
                    server.uid,
                    server.name
                ),
                Err(e) => log::warn!(
                    "отчёты: core={} «{}» sync не запустился: {e:?}",
                    server.uid,
                    server.name
                ),
            }
            // Open rows may have closed or been deleted offline BELOW the catch-up start. Register
            // them for checking; the library retains the set and repeats it on hard reconnect,
            // and the results arrive as ordinary RowUpsert/RowDelete events.
            let open = sink.open_rows(server.uid);
            if !open.is_empty() {
                if let Err(e) = client.reports().check_open_rows(&open) {
                    log::warn!(
                        "отчёты: core={} «{}» check_open_rows не ушёл: {e:?}",
                        server.uid,
                        server.name
                    );
                }
            }
        }
    }

    let mut identity_sent = false;
    let mut last_orders = Instant::now();
    let mut orders_table_pending = false;
    let mut last_strats = Instant::now();
    // Assets snapshot rate cap: minimum 1 s between publishes while the Assets view is active,
    // otherwise 5 s. Publication still requires a domain event; this is not a periodic timer.
    let mut last_assets = Instant::now();
    // Coalesced repair requests for account changes that need a fresh balance or wallet snapshot,
    // plus the recurring API-key expiration poll.
    let mut account_reconciliation = AccountReconciliation::new(Instant::now());
    // Window for logging Balance events after our refresh, used to diagnose phantom Assets entries.
    let mut balance_refresh_log_until: Option<Instant> = None;
    // Transfer-assets cursor: publish only when the revision changes (request/response).
    let mut last_transfer_rev: u64 = u64::MAX;
    // Strategy-export cursors: schema revision plus the contents/checked signature. Publish only
    // changes because strategy fields are expensive and need not be sent every second.
    let mut last_schema_rev: u64 = u64::MAX;
    let mut last_strat_sig: u64 = u64::MAX;
    let mut last_strat_db_generation: Option<(u64, u64)> = None;
    let mut pending_strat_db_delivery: Option<((u64, u64), Receiver<bool>)> = None;
    let mut strat_db_retry_due = false;
    // strat_db: per-kind defaults for dump normalization, a 30-second timestamp heuristic for
    // `origin=local`, and the flag for the first set published by this run, whose origins can
    // predate the run. Recent remote changes can look local, delayed local echoes can look remote,
    // and an internal reconnect does not reset the first-set flag.
    let mut strat_schema_defaults: std::collections::HashMap<
        u8,
        Vec<(String, moonproto::FieldValue)>,
    > = std::collections::HashMap::new();
    let mut local_strat_edits = LocalStratEdits::new();
    // Shadow full-list syncs already accepted by MoonProto's asynchronous runtime queue. Its
    // public snapshot can lag these commands, so conditional destructive actions check both.
    let mut strategy_placements = StrategyPlacementGuard::new();
    let mut strat_db_initial = true;
    // Monotonic per-core detect number used as the ingestion cursor for the UI detect feed.
    let mut detect_seq: u64 = 0;
    // The catch-up whose alive map this feed is waiting for, paired with its request ticket.
    // A `run()` local on purpose: a hard drop ends this loop and discards it, and the next `run()`
    // re-syncs from the writer's durable start state and reconciles again. Hoisting it into the
    // shared sink would let a stale completion outlive the connection that produced it.
    let mut pending_alive: Option<(ReportAliveMapTicket, ReportSyncComplete)> = None;
    // File writer for this core's server log (logs/<date>_<core>.log), with daily rotation. Write
    // on the FEED THREAD rather than the UI thread because log volume is high and the UI must not
    // wait for disk. Only an in-memory copy reaches the UI for live viewing and search.
    let mut log_writer = crate::applog::DatedWriter::new(&server.name);
    let mut events = Vec::new();
    let mut lifecycle_events = Vec::new();
    let mut force_market_sample = false;
    // Latest lifecycle state, and whether this core has already failed one API-key check: an older
    // MoonBot never answers that method, and a warn per retry for the life of the session would be
    // noise. The first failure is worth seeing; the rest are not.
    let mut is_ready = false;
    let mut api_expiry_failed_before = false;
    loop {
        // `SetMarket` contains complete desired state; the other coordinator commands are deltas
        // or actions. A closed channel means the coordinator has exited, so disconnect.
        let mut orders_mutated = false;
        let command_drain = drain_commands(
            cmd_rx,
            &client,
            server,
            latest_market_role,
            market_role,
            &mut force_market_sample,
            &mut orders_mutated,
            &mut local_strat_edits,
            &mut strategy_placements,
            client_settings_sequence,
        );
        if command_drain == CommandDrain::Disconnected {
            return Ok(());
        }
        // Publish an immediate best-effort order snapshot after a flagged command, bypassing the
        // event gate and 250 ms throttle. Some retained-order edits may already be visible locally,
        // but queued work such as new-order creation may not be; this snapshot can precede the
        // asynchronous mutation, and later order events reconcile the rows.
        if orders_mutated && server.feed.orders {
            if let Some(snap) = client.snapshot() {
                let order_rows = build_order_rows(server.id, &snap, &[]);
                last_orders = Instant::now();
                orders_table_pending = false;
                if tx.send(FeedMsg::Orders(order_rows)).is_err() {
                    break;
                }
            }
        }

        // Send the core's exchange from server_info after BaseCheck to the coordinator for grouping
        // and provider election. Publish it once, as soon as the identity is known.
        if !identity_sent {
            if let Some(info) = client.server_info() {
                if let Some(code) = info.exchange_code {
                    // dex_name is nonempty only for Hyperliquid HIP-3 futures. Include it in the
                    // identity so cores from different DEXes are NOT deduplicated onto one provider
                    // with an incomplete market list; see ExchangeId.
                    let dex = info.dex_name.as_deref().unwrap_or("");
                    let id = ExchangeId::with_dex(code.stable_id(), dex);
                    log::info!(
                        "core {} identity: exchange_code={} dex_name={:?} -> {:?}",
                        crate::feed::core_label(server.id),
                        code.stable_id(),
                        dex,
                        id
                    );
                    let _ = tx.send(FeedMsg::Identity {
                        id,
                        dex: dex.to_string(),
                        // The core's own caption travels with the identity so no consumer has to
                        // reach back into a client snapshot for it while rendering.
                        reported: info.exchange_name.clone().unwrap_or_default(),
                    });
                    // The account base currency selects the USD conversion used for manual orders.
                    let base = info.base_currency_name.unwrap_or_default();
                    if !base.is_empty() {
                        let _ = tx.send(FeedMsg::CoreBase { base });
                    }
                    identity_sent = true;
                }
            }
        }

        // Map lifecycle events to status so stages and errors appear directly in the badge.
        // ConnectFailed is a TERMINAL failure of the initial connect/init. The moonproto background
        // runtime breaks and does NOT reconnect; its auto-reconnect handles only link loss AFTER a
        // successful connection. Meanwhile MoonClient::connect is nonblocking and has already
        // returned Ok, so without an explicit exit this loop would run forever in Failed state and
        // the app-level reconnect in feed/mod.rs would never start. Catch ConnectFailed and return
        // Err so the outer loop recreates the client with backoff. This is the exact "5/7, no
        // auto-reconnect" bug.
        let mut connect_failed: Option<String> = None;
        lifecycle_events.clear();
        event_queue.drain_lifecycle_events_into(&mut lifecycle_events);
        for ev in lifecycle_events.drain(..) {
            log::info!("lifecycle: {ev:?}");
            let request_license_state = match &ev {
                LifecycleEvent::Ready => true,
                LifecycleEvent::Connected { fresh } => !*fresh,
                _ => false,
            };
            let st = match ev {
                LifecycleEvent::Connecting => ConnStatus::Stage("connecting…".into()),
                LifecycleEvent::Connected { fresh } => {
                    // fresh=true means one-time initialization follows, so wait for Ready.
                    // fresh=false means a reconnect: moonproto does NOT repeat initialization or
                    // emit Ready again, but subscriptions/indexes are restored and the client is
                    // operational. Otherwise status would remain stuck at "reconnected" (0/N)
                    // forever even while data flows. Therefore a reconnect is immediately Ready.
                    if fresh {
                        ConnStatus::Stage("connected, init…".into())
                    } else {
                        ConnStatus::Ready
                    }
                }
                LifecycleEvent::InitStepCompleted { step, .. } => {
                    ConnStatus::Stage(format!("init: {step}"))
                }
                LifecycleEvent::Ready => ConnStatus::Ready,
                LifecycleEvent::Reconnecting => ConnStatus::Stage("reconnecting…".into()),
                LifecycleEvent::ServerRestart => ConnStatus::Stage("server restart…".into()),
                LifecycleEvent::ConnectFailed { error } => {
                    let msg = error.to_string();
                    connect_failed = Some(msg.clone());
                    ConnStatus::Failed(msg)
                }
                LifecycleEvent::BindFailed {
                    consecutive_failures,
                } => ConnStatus::Failed(format!(
                    "UDP bind failed x{consecutive_failures} (VPN/firewall/порты?)"
                )),
                LifecycleEvent::Disconnected => ConnStatus::Disconnected,
            };
            // Tracked for the API-key poll below: an Engine API request sent to a core that is not
            // Ready buys nothing but a pending timeout. Reaching Ready is also the moment to ask —
            // the key may have been replaced while this core was away — subject to the poll's own
            // cooldown, which is what keeps a flapping core from asking on every reconnect.
            is_ready = st == ConnStatus::Ready;
            if is_ready {
                account_reconciliation.poll_api_expiry_on_ready(Instant::now());
            }
            let _ = tx.send(FeedMsg::Status(st));
            if request_license_state {
                if let Err(error) = client.settings().request_kernel_license_state() {
                    log::warn!(
                        "core {} request kernel license state failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                // Request the complete ClientSettings snapshot (TP/SL/sell/...). The core sends
                // LevManage/RuntimeState after connection itself, so only refresh settings here.
                if let Err(error) = client.settings().refresh() {
                    log::warn!(
                        "core {} request client settings failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                // Account hedge mode for the toolbar toggle.
                if let Err(error) = client.account().refresh_hedge_mode() {
                    log::warn!(
                        "core {} request hedge mode failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                // The API-key check is deliberately NOT fired from here. It reaches the exchange, and
                // a flapping core would issue one per reconnect; the recurring poll below owns it and
                // is due immediately on the first pass, so the first value still arrives at startup.

                // Balances are NOT re-pushed on a reconnect: moonproto skips init, so the
                // retained snapshot keeps feeding pre-outage figures while the status is already
                // back to Ready — and `CoreData::balance_state()` would then classify them Live.
                // Request a refresh here so a successful response can shorten that window. This
                // remains best-effort: a failed request, missing response, or unrelated Assets
                // rebuild can still leave pre-outage figures classified as Live, so authoritative
                // freshness needs a connection generation or balance revision on the payload.
                if let Err(error) = client.balances().refresh() {
                    log::warn!(
                        "core {} post-connect balance refresh failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                account_reconciliation.mark_balance_attempt(Instant::now());
                // Chart alerts are authoritative on the core. Request a full snapshot after
                // initialization/reconnect so the local set cannot lag behind the server.
                if server.feed.alerts {
                    if let Err(error) = client.chart_alerts().request_snapshot() {
                        log::warn!(
                            "core {} request chart alerts snapshot failed: {error}",
                            crate::feed::core_label(server.id)
                        );
                    }
                }
            }
        }
        // Propagate a terminal startup failure as Err so the app-level loop recreates the client;
        // moonproto cannot revive this runtime itself.
        if let Some(e) = connect_failed {
            return Err(anyhow::anyhow!("{e}"));
        }

        // Drain domain events from MoonEventSink. Read ticks/order books/orders from the snapshot
        // only after an actual event instead of polling continuously every 8 ms.
        events.clear();
        event_queue.drain_events_into(&mut events);
        let had_domain_event = !events.is_empty();
        // v4 delivers Stop/VStop changes as ordinary `OrderEvent::Updated` field
        // mutations rather than dedicated events, so `Updated` (already matched
        // below) covers them.
        let has_order_line_event = events.iter().any(|ev| {
            matches!(
                ev,
                Event::Order(
                    OrderEvent::Created(_)
                        | OrderEvent::Updated(_)
                        | OrderEvent::Removed(_)
                        | OrderEvent::TracePoint { .. }
                        | OrderEvent::CorridorChanged(_)
                        | OrderEvent::Snapshot
                )
            )
        });
        let has_orders_table_event = events.iter().any(|ev| {
            matches!(
                ev,
                Event::Order(
                    OrderEvent::Created(_)
                        | OrderEvent::Updated(_)
                        | OrderEvent::Removed(_)
                        | OrderEvent::Snapshot
                        | OrderEvent::CorridorChanged(_)
                )
            )
        });
        // Account changes normally produce incremental balance/wallet pushes. Explicit repair
        // requests run immediately after an idle period and then coalesce inside their cooldown:
        // presentation-only order events are ignored, and authoritative full/Spot updates cancel
        // pending work.
        let account_now = Instant::now();
        account_reconciliation.observe_events(&events, account_now);
        if account_reconciliation.balance_due(account_now) {
            match client.balances().refresh() {
                Ok(()) => {
                    balance_refresh_log_until = Some(Instant::now() + Duration::from_secs(5));
                    log::info!(
                        "core {} balance repair requested (account order change)",
                        crate::feed::core_label(server.id)
                    );
                }
                Err(error) => {
                    log::warn!(
                        "core {} balance refresh request failed: {error}",
                        crate::feed::core_label(server.id)
                    )
                }
            }
            account_reconciliation.mark_balance_attempt(account_now);
        }
        // On wallet-based spot exchanges such as Bitget and Hyperliquid spot, purchased coins exist
        // ONLY in transfer_assets; the core sends no per-market balances. Without polling again, a
        // newly purchased coin does not appear in Assets until the core is clicked manually. Use the
        // same account-relevant signal but a separate 10-second cooldown because this request reaches
        // the exchange (CheckAssets can time out in core logs).
        if account_reconciliation.spot_wallet_due(account_now) {
            if let Err(error) = client
                .balances()
                .refresh_transfer_assets_kind(moonproto::ExchangeKind::Spot)
            {
                log::warn!(
                    "core {} spot wallet refresh request failed: {error}",
                    crate::feed::core_label(server.id)
                );
            }
            account_reconciliation.mark_spot_wallet_attempt(account_now);
        }
        // API-key expiration: a pure poll, since no event announces that a key aged a day. Gated on
        // Ready — a request to a core that is not connected only buys a pending timeout — and the
        // attempt is marked whether or not the request left, so a core stuck mid-connect cannot ask
        // on every wake-up.
        if account_reconciliation.api_expiry_due(account_now) {
            if is_ready {
                if let Err(error) = client.account().refresh_api_expiration_time() {
                    log::debug!(
                        "core {} api expiration poll not sent: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                account_reconciliation.mark_api_expiry_attempt(account_now);
            } else {
                // A due deadline that the Ready gate declines must still move, or the wait below
                // computes zero every pass and this thread spins on `recv_timeout(0)` for as long
                // as the core stays down.
                account_reconciliation.defer_api_expiry(account_now);
            }
        }
        // Track the outcome of the bulk 5-minute candle snapshot that moonproto requests
        // automatically after subscribe_all_trades. Retained-history candle metrics such as
        // screener H.vol/72h depend on it, and failure would otherwise be completely silent because
        // no one consumed the event. Account for a known moonproto bug reported upstream: the
        // snapshot can be silently discarded even AFTER Ready because of the server timezone.
        // Surface Engine action results (leverage/hedge/cancel-all/transfer/...) as UI toasts. They
        // also arrive on disconnect with `success=false`, making "did not reach the server" visible.
        let mut engine_actions: Vec<crate::feed::EngineActionResult> = Vec::new();
        // Chart alerts (figures with Alert checked): the core is the authoritative source.
        let mut chart_alerts: Vec<crate::feed::ChartAlertUpdate> = Vec::new();
        for ev in &events {
            match ev {
                Event::ChartAlert(ev) if server.feed.alerts => {
                    // During reverse engineering of the TChartObject.Save blob, log the full hex.
                    // Alerts are created manually and are few, so log volume is not a concern.
                    match ev {
                        moonproto::ChartAlertEvent::Upserted(obj) => {
                            log::info!(
                                "core {} chart alert upserted: {} uid={} blob[{}]={}",
                                crate::feed::core_label(server.id),
                                obj.market_name,
                                obj.obj_uid,
                                obj.blob.len(),
                                hex_dump(&obj.blob)
                            );
                            chart_alerts.push(crate::feed::ChartAlertUpdate::Upserted(
                                crate::feed::ChartAlertRow {
                                    market: obj.market_name.clone(),
                                    obj_uid: obj.obj_uid,
                                    blob: obj.blob.clone(),
                                },
                            ));
                        }
                        moonproto::ChartAlertEvent::Deleted {
                            market_name,
                            obj_uid,
                        } => {
                            log::info!(
                                "core {} chart alert deleted: {} uid={}",
                                crate::feed::core_label(server.id),
                                market_name,
                                obj_uid
                            );
                            chart_alerts.push(crate::feed::ChartAlertUpdate::Deleted {
                                market: market_name.clone(),
                                obj_uid: *obj_uid,
                            });
                        }
                    }
                }
                Event::EngineAction(e) => {
                    if !e.success {
                        log::warn!(
                            "core {} engine action failed: {:?} code={} msg={}",
                            crate::feed::core_label(server.id),
                            e.kind,
                            e.error_code,
                            e.error_msg
                        );
                    }
                    engine_actions.push(convert::engine_action_result(e));
                }
                Event::CandlesSnapshot(moonproto::state::CandlesSnapshotEvent::Ready {
                    summary,
                    ..
                }) => {
                    log::debug!(
                        "core {} candles snapshot ready: markets {}/{} candles {}/{}",
                        crate::feed::core_label(server.id),
                        summary.retained_markets,
                        summary.received_markets,
                        summary.retained_candles,
                        summary.received_candles
                    );
                }
                Event::CandlesSnapshot(moonproto::state::CandlesSnapshotEvent::Failed {
                    error,
                    ..
                }) => {
                    log::warn!(
                        "core {} candles snapshot failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                // The demand-driven chart archive. `received` counts the rows the core sent;
                // `retained` counts what the ring holds after archive and live tail are merged —
                // NOT how much of the archive survived. MoonProto trims the merged set from the
                // FRONT, so a ring already full of live rows can report a large `retained` while
                // every archive row was discarded. Both numbers are logged because their ratio is
                // the only hint available from here; neither one alone means success.
                Event::MarketHistory(moonproto::state::MarketHistoryEvent::Ready {
                    ticket,
                    summary,
                }) => {
                    log::info!(
                        "core {} chart archive {} merged: sent trades {} minis {} prices {} liq {}; \
                         rings now hold {} / {} / {} / {}",
                        crate::feed::core_label(server.id),
                        ticket.market,
                        summary.received.futures_trades,
                        summary.received.mini_candles,
                        summary.received.last_prices,
                        summary.received.liquidations,
                        summary.retained.futures_trades,
                        summary.retained.mini_candles,
                        summary.retained.last_prices,
                        summary.retained.liquidations,
                    );
                    // `Ready` is an apply barrier, so this is the first moment the merged rings can
                    // be measured. Off unless MOON_ARCHIVE_PROBE is set.
                    archive_probe::probe(
                        &client,
                        &ticket.market,
                        crate::feed::core_label(server.id),
                    );
                }
                Event::MarketHistory(moonproto::state::MarketHistoryEvent::Failed {
                    ticket,
                    error,
                }) => {
                    log::warn!(
                        "core {} chart archive {} failed: {error}",
                        crate::feed::core_label(server.id),
                        ticket.market
                    );
                }
                // A failed CoinCard request for deep chart history used to fall into `_ => {}`
                // SILENTLY, so candles "did not arrive" without any trace in the log.
                Event::CoinCardCandles(moonproto::state::CoinCardCandlesEvent::UpdateFailed {
                    market,
                    kind,
                    error,
                    ..
                }) => {
                    log::warn!(
                        "core {} coin-card {market} {kind:?} failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                // Diagnostic window after our balance refresh for phantom Assets entries. Response
                // type determines the stuck coin's fate: Snapshot clears missing entries while
                // Incremental does not. Do not log outside this window because balances are pushed
                // continuously.
                Event::Balance(bev) => {
                    if balance_refresh_log_until.is_some_and(|t| Instant::now() < t) {
                        log::info!(
                            "core {} balance event after refresh: {bev:?}",
                            crate::feed::core_label(server.id)
                        );
                    }
                }
                // News/tags feed is consumed below via `news_snapshot_from_proto` reading the
                // retained `client.snapshot().news()`, matching the license/KernelHealth idiom.
                Event::News(_) => {}
                // `Event::KernelHealth` is consumed below via `settings_event_snapshot`
                // reading the retained `kernel_health()` snapshot, not here.
                _ => {}
            }
        }
        if !engine_actions.is_empty() && tx.send(FeedMsg::EngineActions(engine_actions)).is_err() {
            break;
        }
        if !chart_alerts.is_empty() && tx.send(FeedMsg::ChartAlerts(chart_alerts)).is_err() {
            break;
        }
        let license_state = settings_event_snapshot(
            &events,
            &client,
            |ev| {
                matches!(
                    ev,
                    &Event::Settings(SettingsEvent::KernelLicenseStateUpdated)
                )
            },
            |state| {
                state
                    .settings()
                    .kernel_license_state
                    .map(license_state_from_proto)
            },
        );
        if let Some(license) = license_state {
            if tx.send(FeedMsg::License(license)).is_err() {
                break;
            }
        }
        // ClientSettings/LevManage/RuntimeState are core settings snapshots. Read each from the
        // snapshot ONLY when its event arrives rather than on every tick, as with license above.
        let client_settings = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::ClientSettingsUpdated)),
            |state| {
                state
                    .settings()
                    .client_settings
                    .as_ref()
                    .map(client_settings_from_proto)
            },
        );
        if let Some(settings) = client_settings {
            client_settings_sequence.observe_update();
            client_settings_sequence.drive(&client, server.id);
            if tx.send(FeedMsg::ClientSettings(settings)).is_err() {
                break;
            }
        }
        let lev_manage = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::LevManageUpdated)),
            |state| {
                state
                    .settings()
                    .lev_manage
                    .as_ref()
                    .map(lev_manage_from_proto)
            },
        );
        if let Some(lev) = lev_manage {
            if tx.send(FeedMsg::LevManage(lev)).is_err() {
                break;
            }
        }
        let runtime_state = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::RuntimeStateUpdated)),
            |state| {
                state
                    .settings()
                    .runtime_state
                    .as_ref()
                    .map(runtime_state_from_proto)
            },
        );
        if let Some(state) = runtime_state {
            if tx.send(FeedMsg::RuntimeState(state)).is_err() {
                break;
            }
        }
        // Core resource telemetry (protocol v4 `Event::KernelHealth`). Read from the
        // RETAINED snapshot (`kernel_health()`), not the event payload, matching the
        // license/settings idiom above: the retained value keeps the last memory sample
        // between CPU-only Pings. The store bumps `sys_rev` only on a metric change, and
        // repaints are capped by the 250ms backend throttle + the panel `RenderGate`.
        let sys_status = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::KernelHealth(_)),
            |state| Some(sys_status_from_proto(state.kernel_health(), now_ms_i64())),
        );
        if let Some(sys) = sys_status {
            if tx.send(FeedMsg::SysStatus(sys)).is_err() {
                break;
            }
        }
        // News/tags: read the retained `NewsState` only when an `Event::News` arrived, matching the
        // license/settings idiom. The store gates the panel with `news_rev` only on a real change,
        // so a duplicate frame that reduces to the same logical set does not repaint.
        let news = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::News(_)),
            |state| Some(convert::news_snapshot_from_proto(state.news())),
        );
        if let Some(news) = news {
            if tx.send(FeedMsg::News(news)).is_err() {
                break;
            }
        }
        // Account-plane answers, both of which arrive directly in an Engine API response event.
        // ONE pass over the batch: `events` is the whole domain drain (ticks, book, orders) on a
        // thread that runs per core, and these two answers are rare enough that a second traversal
        // would cost far more than it carries.
        //
        // API-key expiration publishes successful answers only. A failed check is logged and
        // otherwise ignored, so one dropped request does not erase the last known day count and
        // cannot be mistaken for "this key has no expiration".
        let mut hedge_mode = None;
        let mut api_expiry = None;
        for ev in &events {
            match ev {
                Event::Account(AccountEvent::HedgeModeUpdated { hedge_mode: on, .. }) => {
                    // FIRST match in the batch, as the `find_map` this replaced took — the single
                    // pass is an efficiency change, not a behaviour one.
                    hedge_mode = hedge_mode.or(Some(*on));
                }
                Event::Account(AccountEvent::ApiExpirationUpdated { expiration, .. }) => {
                    let expiry = convert::api_key_expiry_from_proto(*expiration, SystemTime::now());
                    // Logged because nothing else observes this path: the value changes about once
                    // a day, so a silent success is indistinguishable from a request that never
                    // left. Rare enough (once per connect, then six-hourly) to cost nothing. The
                    // RAW count is logged beside the accepted one so core-side oddities stay
                    // visible instead of vanishing into the display: a legacy answer's `-1000`,
                    // which the sanity range drops, and the round `+1000` current cores send.
                    log::info!(
                        "core {} api key: known={} days_left={:?} reported={:?}",
                        crate::feed::core_label(server.id),
                        expiry.known,
                        expiry.days_left,
                        expiration.reported_days_left()
                    );
                    api_expiry = Some(expiry);
                }
                Event::Account(AccountEvent::ApiExpirationUpdateFailed { error, .. }) => {
                    // Retry sooner than the full interval: a core that was merely busy should not
                    // wait six hours for its first day count.
                    account_reconciliation.retry_api_expiry(Instant::now());
                    if api_expiry_failed_before {
                        log::debug!(
                            "core {} api expiration check failed again: {error}",
                            crate::feed::core_label(server.id)
                        );
                    } else {
                        api_expiry_failed_before = true;
                        log::warn!(
                            "core {} api expiration check failed: {error}",
                            crate::feed::core_label(server.id)
                        );
                    }
                }
                _ => {}
            }
        }
        if let Some(on) = hedge_mode {
            if tx.send(FeedMsg::HedgeMode(on)).is_err() {
                break;
            }
        }
        if let Some(expiry) = api_expiry {
            if tx.send(FeedMsg::ApiExpiry(expiry)).is_err() {
                break;
            }
        }
        let wanted = market_role.wanted();
        let dirty_markets = if market_role.is_provider() && !wanted.is_empty() {
            market_dirty_from_events(&events, wanted, force_market_sample)
        } else {
            Vec::new()
        };
        let want_log = server.feed.log;
        // detect-diag: report a subset of server feed flags once per process. The line includes
        // detects/reports/log but not alerts; `Event::Detect` can still run with detects=false when
        // alerts=true. Controlled by MOON_DETECT_DIAG, off by default.
        {
            use std::sync::OnceLock;
            static FLAGS_ONCE: OnceLock<()> = OnceLock::new();
            if crate::detect_diag::enabled() && FLAGS_ONCE.set(()).is_ok() {
                crate::detect_diag::line(&format!(
                    "[live] flags: feed.detects={} feed.reports={} feed.log={}",
                    server.feed.detects, server.feed.reports, want_log
                ));
            }
        }
        // Alert fires (`DETECT_KIND_ALERT`) arrive as Event::Detect. Also enter this path when
        // feed.alerts is enabled so alerts work without the general detect stream.
        let want_detects = server.feed.detects || server.feed.alerts;
        if want_detects || (server.feed.reports && reports.is_some()) || want_log {
            let mut detects: Vec<DetectRow> = Vec::new();
            let mut logs: Vec<CoreLogLine> = Vec::new();
            // Snapshot for fields of the strategy that produced the detect (SoundAlert/KeepAlert/sound).
            let detect_snap = want_detects.then(|| client.snapshot()).flatten();
            // Strategy schema for fallback to default_value: the server omits fields equal to the
            // schema default, including sound/SoundAlert.
            let detect_schema = detect_snap
                .as_ref()
                .and_then(|s| s.strats().strategy_schema());
            for ev in &events {
                match ev {
                    Event::ServerLog(l) if want_log => {
                        let ms = l.unix_millis();
                        let recv_ms = now_ms_i64();
                        // Core text is foreign input and can name the machine it runs on. The file
                        // writer redacts on its own; this call covers the UI copy below, which does
                        // not pass through it.
                        let msg = crate::applog::redact::addresses(&l.msg);
                        // Write to disk immediately through the buffer; split time into date and clock.
                        let (date, hms) = crate::applog::split_unix_ms(ms);
                        log_writer.write(&date, &hms, "INFO", "", &msg);
                        logs.push(CoreLogLine {
                            time_ms: ms,
                            recv_ms,
                            msg: msg.into_owned(),
                        });
                    }
                    Event::Detect(d)
                        if server.feed.detects || (server.feed.alerts && d.is_alert_fire()) =>
                    {
                        let strat = detect_snap
                            .as_ref()
                            .and_then(|s| s.strats().snapshot(d.strategy_id));
                        let params = strat
                            .map(|st| alert_params(st, detect_schema))
                            .unwrap_or_default();
                        crate::detect_diag::line(&format!(
                            "[feed] detect market={} strat_id={} strat_found={} sound_alert={} sound={:?} is_alert={}",
                            d.market_name,
                            d.strategy_id,
                            strat.is_some(),
                            params.sound_alert,
                            params.sound_name,
                            d.is_alert_fire(),
                        ));
                        detect_seq += 1;
                        detects.push(DetectRow {
                            seq: detect_seq,
                            market: d.market_name.clone(),
                            time_ms: now_ms(),
                            sound_alert: params.sound_alert,
                            keep_alert_secs: params.keep_alert_secs,
                            add_to_chart: params.add_to_chart,
                            keep_in_chart_secs: params.keep_in_chart_secs,
                            sound_name: params.sound_name,
                            is_alert: d.is_alert_fire(),
                            // Kind of the strategy that produced the detect, used for its type badge.
                            // Without a strategy snapshot, mark an alert fire as Alerts kind (22).
                            kind: strat
                                .map(|st| st.kind().ordinal())
                                .unwrap_or(if d.is_alert_fire() { 22 } else { 0 }),
                            is_short: strat.map(|st| st.is_short()).unwrap_or(false),
                        });
                    }
                    // The core committed a checkbox delta. Published as its own message because the
                    // strategy SNAPSHOT cannot carry this fact: the protocol library applies a
                    // local `set_checked` to its own snapshot before anything is sent, and the
                    // echo it later receives updates only an acknowledgement field the snapshot
                    // does not expose. Anything that must not proceed until the core agreed —
                    // deleting a strategy after disabling it — waits on this.
                    Event::Strat(
                        moonproto::state::StratEvent::CheckedEcho { .. }
                        | moonproto::state::StratEvent::CheckedSynced { .. },
                    ) => {
                        let _ = tx.send(FeedMsg::StrategiesAck);
                    }
                    // Typed report-database replica: send schema, rows, catch-up state, and
                    // reconciliation results to the SQLite writer, the sole write-connection owner.
                    Event::Report(rev) if server.feed.reports => {
                        if let Some(sink) = reports {
                            match rev {
                                ReportEvent::Schema(s) => sink.send(DbMsg::Schema {
                                    core_uid: server.uid,
                                    schema: s.clone(),
                                }),
                                ReportEvent::RowUpsert(row) => sink.send(DbMsg::Upsert {
                                    core_uid: server.uid,
                                    core_name: server.name.clone(),
                                    row: row.clone(),
                                }),
                                ReportEvent::RowDelete { rec_id } => sink.send(DbMsg::Delete {
                                    core_uid: server.uid,
                                    rec_id: *rec_id,
                                }),
                                // Bulk soft-delete/restore echo: flip the `deleted` flag on the
                                // named rows rather than dropping them, so a restore can undo it.
                                ReportEvent::RowsDeleted(change) => sink.send(DbMsg::SetDeleted {
                                    core_uid: server.uid,
                                    change: change.clone(),
                                }),
                                ReportEvent::SyncStarted { request, .. } => log::info!(
                                    "отчёты: core={} «{}» sync начат (from_rec_id={})",
                                    server.uid,
                                    server.name,
                                    request.from_rec_id,
                                ),
                                // Catch-up page: the writer applies it in a transaction and
                                // acknowledges it with `page_applied` after commit. Until then the
                                // library requests no next page. A recreation also clears the local
                                // replica and re-declares the terminal's full-history policy after ACK.
                                ReportEvent::SyncPage(page) => sink.send(DbMsg::Page {
                                    core_uid: server.uid,
                                    core_name: server.name.clone(),
                                    page: page.clone(),
                                    ack: client.reports(),
                                }),
                                // Catch-up advances by newRecID, so it cannot see a soft-delete,
                                // restore or retention removal of an OLDER row that happened while
                                // this terminal was offline. Ask for the core's compact alive map
                                // and hold the completion: its checkpoint is committed by the
                                // transaction that applies that map, never here.
                                ReportEvent::SyncComplete(done) => {
                                    sink.send(DbMsg::SyncComplete {
                                        core_uid: server.uid,
                                        done: done.clone(),
                                    });
                                    match client.reports().reconcile_alive(done) {
                                        Ok(ticket) => {
                                            pending_alive = Some((ticket, done.clone()));
                                        }
                                        // Fail-closed by construction: with no map the checkpoint
                                        // does not advance, and the next connection repeats
                                        // catch-up from the last committed start state.
                                        Err(e) => {
                                            pending_alive = None;
                                            log::warn!(
                                                "отчёты: core={} «{}» запрос карты живых строк не \
                                                 ушёл: {e:?}",
                                                server.uid,
                                                server.name,
                                            );
                                        }
                                    }
                                }
                                ReportEvent::OpenRowsCheckStarted { rec_ids } => log::info!(
                                    "отчёты: core={} «{}» проверка открытых строк начата ({} шт)",
                                    server.uid,
                                    server.name,
                                    rec_ids.len(),
                                ),
                                ReportEvent::OpenRowsCheckComplete { rec_ids } => log::info!(
                                    "отчёты: core={} «{}» проверка открытых строк завершена ({} шт)",
                                    server.uid,
                                    server.name,
                                    rec_ids.len(),
                                ),
                                // Authoritative visibility for 1..=covered_up_to. Only a map that
                                // describes the catch-up this feed asked about may be applied;
                                // anything else would hide live rows and then record that as
                                // reconciled.
                                ReportEvent::AliveMapComplete(map) => {
                                    match alive_map_action(
                                        pending_alive.as_ref(),
                                        map.ticket,
                                        map.epoch,
                                        map.covered_up_to,
                                        map.outcome,
                                    ) {
                                        AliveAction::Apply(checkpoint) => {
                                            pending_alive = None;
                                            sink.send(DbMsg::AliveMap {
                                                core_uid: server.uid,
                                                map: map.clone(),
                                                checkpoint,
                                            });
                                        }
                                        AliveAction::Wipe => {
                                            pending_alive = None;
                                            sink.send(DbMsg::ReplicaRecreated {
                                                core_uid: server.uid,
                                                reports: client.reports(),
                                            });
                                        }
                                        AliveAction::Ignore(why) => log::warn!(
                                            "отчёты: core={} «{}» карта живых строк отклонена \
                                             ({why}): epoch={}, covered_up_to={}, outcome={:?}",
                                            server.uid,
                                            server.name,
                                            map.epoch,
                                            map.covered_up_to,
                                            map.outcome,
                                        ),
                                    }
                                }
                                ReportEvent::SchemaRejected { reason } => log::error!(
                                    "отчёты: core={} «{}» схема отвергнута: {reason}",
                                    server.uid,
                                    server.name,
                                ),
                            }
                        }
                    }
                    // Diagnostics through the moonproto-diagnostics feature, OFF by default. In a
                    // normal build, packets rejected by the library parser/validation disappear
                    // without a trace. This is how BB1 report sync stalled: a page was rejected
                    // silently and the cursor did not move for days. For a catch-up page (CmdId=39),
                    // decode the header to show WHAT the server sent (row_count/last/max_rec_id)
                    // and why validation failed.
                    #[cfg(feature = "moonproto-diagnostics")]
                    Event::ParseFailed { cmd, len, payload } => {
                        let mut extra = String::new();
                        if payload.first() == Some(&39) && payload.len() >= 37 {
                            let i = |a: usize| {
                                i64::from_le_bytes(payload[a..a + 8].try_into().unwrap())
                            };
                            let ru = u64::from_le_bytes(payload[11..19].try_into().unwrap());
                            let rc = u16::from_le_bytes(payload[35..37].try_into().unwrap());
                            extra = format!(
                                " sync-page: request_uid={ru} last_rec_id={} max_rec_id={} row_count={rc}",
                                i(19),
                                i(27),
                            );
                        }
                        log::warn!(
                            "отчёты(diag): core={} «{}» пакет отвергнут: cmd={cmd:?} len={len}{extra}",
                            server.uid,
                            server.name,
                        );
                    }
                    _ => {}
                }
            }
            if !logs.is_empty() {
                log_writer.flush(); // One flush per batch, not per line, keeps disk from becoming a bottleneck.
                if tx.send(FeedMsg::ServerLog(logs)).is_err() {
                    break;
                }
            }
            // detect-diag: count the Event::Detect items actually drained and those with
            // AddToChart>0. raw>0 with with_chart=0 means the strategy lacks AddToChart, so no tab
            // should appear and this is not a chart bug. This block runs only when `detects` is
            // nonempty, so it never logs raw=0.
            if server.feed.detects && !detects.is_empty() {
                let raw = detects.len();
                let with_chart = detects.iter().filter(|d| d.add_to_chart > 0).count();
                crate::detect_diag::line(&format!(
                    "[live] drained detects raw={raw} add_to_chart>0={with_chart}"
                ));
            }
            if !detects.is_empty() && tx.send(FeedMsg::Detects(detects)).is_err() {
                break;
            }
        }

        // A snapshot is cheap (an Arc clone), but read it only after an actual domain event. Throttle
        // the UI order table to about 4 Hz while updating the chart/order-line store immediately on
        // OrderEvent. Otherwise a short terminal status (Cancel/Fail with deferred-removal=0) can be
        // missed between two table ticks.
        if server.feed.orders && has_orders_table_event && !orders_table_pending {
            orders_table_pending = true;
        }
        if server.feed.orders && (had_domain_event || orders_table_pending) {
            let orders_due = last_orders.elapsed() >= Duration::from_millis(250);
            let orders_table_due = orders_table_pending && orders_due;
            let order_lines_due = has_order_line_event && !orders_table_due;
            if orders_table_due || order_lines_due {
                let Some(snap) = client.snapshot() else {
                    continue;
                };
                let order_rows = build_order_rows(server.id, &snap, &events);
                if orders_table_due {
                    last_orders = Instant::now();
                    orders_table_pending = false;
                    if tx.send(FeedMsg::Orders(order_rows)).is_err() {
                        break;
                    }
                } else if order_lines_due && tx.send(FeedMsg::OrderLines(order_rows)).is_err() {
                    break;
                }
            }
        }

        // Core strategies for the Strategies window: check on domain events at no more than about
        // 1 Hz and publish only changes.
        if (had_domain_event || pending_strat_db_delivery.is_some() || strat_db_retry_due)
            && server.feed.strategies
            && last_strats.elapsed() >= Duration::from_secs(1)
        {
            last_strats = Instant::now();
            if let Some(snap) = client.snapshot() {
                let strats = snap.strats();

                // Publish the schema (per-kind sections/fields) when its revision changes.
                let sr = strats.strategy_schema_revision();
                if sr != last_schema_rev {
                    last_schema_rev = sr;
                    if let Some(schema) = strats.strategy_schema() {
                        // Field defaults used to normalize strat_db dumps; see below.
                        strat_schema_defaults = schema_default_fields(schema);
                        if tx
                            .send(FeedMsg::StrategySchema(build_schema_model(schema)))
                            .is_err()
                        {
                            break;
                        }
                    }
                }

                // Publish contents/values when the signature changes (id/ver/last_date/checked).
                let mut sig = 0u64;
                for s in strats.snapshots() {
                    sig = sig
                        .wrapping_mul(1099511628211)
                        .wrapping_add(s.strategy_id)
                        .wrapping_add((s.strategy_ver as u32 as u64).wrapping_shl(1))
                        .wrapping_add(s.last_date)
                        .wrapping_add(s.checked as u64);
                }
                let delivery_result =
                    pending_strat_db_delivery
                        .as_ref()
                        .and_then(|(generation, ack)| match ack.try_recv() {
                            Ok(committed) => Some((*generation, committed)),
                            Err(TryRecvError::Empty) => None,
                            Err(TryRecvError::Disconnected) => Some((*generation, false)),
                        });
                if let Some((generation, committed)) = delivery_result {
                    pending_strat_db_delivery = None;
                    apply_strategy_delivery_ack(
                        generation,
                        committed,
                        &mut last_strat_db_generation,
                        &mut strat_db_retry_due,
                        &mut strat_db_initial,
                    );
                }
                if sig != last_strat_sig {
                    last_strat_sig = sig;
                    // The order table's Strat column resolves strat_id to kind through this same
                    // registry in `build_order_row`. The registry is populated AFTER orders, while
                    // the table normally rebuilds only on order events, so raw strat_id values are
                    // visible until then. A strategy-set change must resolve the names again: set
                    // orders_table_pending so the table rebuilds within about 250 ms even without a
                    // new order event; order_wait below wakes the loop on its timer.
                    if server.feed.orders {
                        orders_table_pending = true;
                    }
                    let strategies: Vec<StrategyRow> = strats
                        .snapshots()
                        .map(|s| {
                            let name = s
                                .strategy_name()
                                .filter(|n| !n.is_empty())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("strat {}", s.strategy_id));
                            let fields = s
                                .fields
                                .iter()
                                .map(|(n, v)| (n.to_string(), fmt_field(v)))
                                .collect();
                            StrategyRow {
                                id: s.strategy_id,
                                name,
                                kind: strat_kind_name(s.kind().ordinal()).to_string(),
                                kind_ordinal: s.kind().ordinal(),
                                folder_path: s.path.to_string(),
                                checked: s.checked,
                                is_short: s.is_short(),
                                fields,
                            }
                        })
                        .collect();
                    if tx.send(FeedMsg::Strategies(strategies)).is_err() {
                        break;
                    }
                }
                // The database cursor is separate from the UI cursor: schema defaults can arrive
                // after an unchanged strategy set, and a full writer queue must leave the set due
                // for retry. Dumps are incomplete until defaults are available.
                if pending_strat_db_delivery.is_none()
                    && strategy_db_export_due(
                        !strat_schema_defaults.is_empty(),
                        sr,
                        sig,
                        last_strat_db_generation,
                    )
                {
                    if let Some(sink) = crate::strat_db::sink() {
                        local_strat_edits.prune();
                        let dumps: Vec<crate::strat_db::StratDump> = strats
                            .snapshots()
                            .map(|s| {
                                strat_db_dump(
                                    s,
                                    &strat_schema_defaults,
                                    local_strat_edits.is_local(s.strategy_id),
                                )
                            })
                            .collect();
                        let (ack, ack_rx) = sync_channel(1);
                        if sink.send(crate::strat_db::StratMsg::FullSet {
                            core_uid: server.uid,
                            core_name: server.name.clone(),
                            initial: strat_db_initial,
                            strategies: dumps,
                            ack,
                        }) {
                            pending_strat_db_delivery = Some(((sr, sig), ack_rx));
                            strat_db_retry_due = false;
                        } else {
                            strat_db_retry_due = true;
                        }
                    }
                }
            }
        }

        // Core assets for the Assets window: publish balance changes immediately because the header
        // reads free/total funds from this snapshot. Other domain events remain rate-limited to about
        // 1 Hz while the window is active and 0.2 Hz otherwise; a quiet core emits nothing. Prices
        // come from the market, so publish the full snapshot; the UI gates repainting by placing
        // assets_rev into one-second buckets. Publish transfer assets only on revision changes.
        let assets_every = if crate::feed::assets_view_active() {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(5)
        };
        if should_publish_assets(&events, last_assets.elapsed(), assets_every) {
            last_assets = Instant::now();
            if let Some(snap) = client.snapshot() {
                // The account base currency (USDT/BTC/...) is required to convert `btc_balance_*`,
                // historically denominated in the base currency, into USDT. The same server info
                // identifies a futures core through the BaseCheck mask; the UI restricts futures
                // assets to positions.
                let info = client.server_info();
                let base = info
                    .as_ref()
                    .and_then(|i| i.base_currency_name.clone())
                    .unwrap_or_default();
                let futures_account = info
                    .as_ref()
                    .is_some_and(|i| i.supports(moonproto::ExchangeTypeMask::FUTURES));
                let assets = build_assets(snap.markets(), snap.balances(), &base, futures_account);
                if tx.send(FeedMsg::Assets(assets)).is_err() {
                    break;
                }
            }
        }

        // Check transfer assets on EVERY iteration rather than in the 1 Hz/domain-event block so a
        // `refresh_transfer_assets` response, requested by clicking the core in the Assets window,
        // reaches the UI immediately even when the core has no stream of market events.
        if let Some(snap) = client.snapshot() {
            let tr = snap.transfer_assets();
            let rev = tr.revision();
            if rev != last_transfer_rev {
                last_transfer_rev = rev;
                let msg = build_transfer_assets(snap.markets(), tr);
                if tx.send(FeedMsg::TransferAssets(msg)).is_err() {
                    break;
                }
            }
        }

        // Do NOT copy market data here. The feed only signals that the provider has a fresh
        // read-model snapshot; the visible chart pulls the markets it needs itself.
        if !dirty_markets.is_empty() {
            if tx.send(FeedMsg::MarketDataChanged(dirty_markets)).is_err() {
                let _ = client.disconnect();
                return Ok(());
            }
        }
        force_market_sample = false;

        if !command_drain.may_wait() {
            continue;
        }

        let order_wait = if server.feed.orders && orders_table_pending {
            let elapsed = last_orders.elapsed();
            Some(Duration::from_millis(250).saturating_sub(elapsed))
        } else {
            None
        };
        let wait_now = Instant::now();
        let account_wait = account_reconciliation.next_wait(wait_now);
        // The API-key poll is the one deadline that is ALWAYS pending, so the wait now always has a
        // ceiling — which is what makes the poll fire on a core whose event stream went quiet. The
        // loop therefore no longer blocks indefinitely, and the former unbounded `recv()` arm is
        // gone with it.
        let poll_wait = account_reconciliation.api_expiry_wait(wait_now);
        let wake_wait = [order_wait, account_wait]
            .into_iter()
            .flatten()
            .fold(poll_wait, Duration::min);
        let wake_result = wake_rx.recv_timeout(wake_wait).map_err(|err| match err {
            std::sync::mpsc::RecvTimeoutError::Timeout => None,
            std::sync::mpsc::RecvTimeoutError::Disconnected => Some(()),
        });
        match wake_result {
            Ok(()) => while wake_rx.try_recv().is_ok() {},
            Err(None) => {}
            Err(Some(())) => {
                let _ = client.disconnect();
                return Ok(());
            }
        }
    }

    let _ = client.disconnect();
    Ok(())
}

/// Returns whether this event batch must publish the retained assets snapshot.
///
/// Balance events bypass the ordinary market-driven rate limit so the header reflects confirmed
/// free funds immediately. Other domain events publish only after `assets_every` has elapsed.
fn should_publish_assets(
    events: &[Event],
    assets_elapsed: Duration,
    assets_every: Duration,
) -> bool {
    !events.is_empty()
        && (assets_elapsed >= assets_every
            || events
                .iter()
                .any(|event| matches!(event, Event::Balance(_))))
}

/// Returns bytes as a separator-free hex string for dumping chart-alert blobs while reverse
/// engineering the `TChartObject.Save()` format; see alert stage 0 in the internal docs.
fn hex_dump(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
