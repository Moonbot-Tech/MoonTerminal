//! Live backend that owns one Moonbot core connection and its MoonProtoBeta event loop.
//!
//! Flow: event-driven. `MoonEventSink` wakes the backend thread after an actual event; market data
//! remains in an immutable read-model snapshot, and only a lightweight signal reaches this module.
//!
//! `run()` is the main event loop; role commands live in [`commands`], pure moonproto-to-terminal
//! converters in [`convert`], and dirty-market calculation in [`dirty`].

mod client_settings;
mod commands;
mod convert;
mod dirty;

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moonproto::state::{AccountEvent, MarketHistorySizing, OrderEvent, SettingsEvent};
use moonproto::{
    ClientConfig, ConnectConfig, Event, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
    MoonEventSink, ReportEvent, ReportHistoryDepth, ReportSyncRequest, TransportMode,
};

use super::assets::{build_assets, build_transfer_assets};
use super::strategies::{
    alert_params, build_schema_model, fmt_field, schema_default_fields, strat_db_dump,
    strat_kind_name,
};
use super::{
    ConnStatus, CoreCmd, CoreLogLine, DetectRow, ExchangeId, FeedMsg, FeedTx, SharedMoonClient,
    StrategyRow,
};
use crate::config::ServerConfig;
use crate::db::{DbMsg, ReportTx};
use crate::util::{now_unix_ms as now_ms, now_unix_ms_i64 as now_ms_i64};

pub(in crate::feed) use client_settings::ClientSettingsSequence;
use commands::{drain_commands, LocalStratEdits};
use convert::{
    build_order_rows, client_settings_from_proto, lev_manage_from_proto, license_state_from_proto,
    runtime_state_from_proto, settings_event_snapshot, sys_status_from_proto,
};
use dirty::market_dirty_from_events;

struct ClientSlotGuard {
    slot: SharedMoonClient,
}

impl Drop for ClientSlotGuard {
    fn drop(&mut self) {
        self.slot.set(None);
    }
}

/// Run one core's live MoonProto event loop until shutdown or a terminal connection error.
///
/// Publishes account snapshots and lifecycle state through `tx`, consumes commands from
/// `cmd_rx`, and exposes the active client through `client_slot`. Returns an error when setup or
/// the live loop cannot continue; dropping the internal guard clears the shared client slot.
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
) -> anyhow::Result<()> {
    let _ = tx.send(FeedMsg::Status(ConnStatus::Connecting));

    // 1. Decode the key into master/MAC keys and its suggested network.
    let info = moonproto::parse_key_info(server.key.expose())
        .ok_or_else(|| anyhow::anyhow!("не удалось разобрать ключ Moonbot (server.key)"))?;

    // 2. Derive the endpoint from the key, which embeds host/port/transport; the config no longer
    //    has separate fields for them.
    let net = info.network.as_ref();
    let host: String = net
        .and_then(|n| n.address)
        .map(|a| a.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port: u16 = net.map(|n| n.port).filter(|p| *p != 0).unwrap_or(3000);
    let transport = net.map(|n| n.transport_mode).unwrap_or(TransportMode::V0);
    log::info!("live connect {host}:{port} market={}", server.market);

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
    // library sends after connection and repeats on its own after a hard reconnect. The cursor is
    // max(newRecID)+1 in the local replica; 0 means the replica is empty, so start fresh with ALL
    // retained core history, replacing the legacy table. Catch-up is PAGED: the next page is not
    // requested until the writer commits and acknowledges the current one (backpressure by design).
    if server.feed.reports {
        if let Some(sink) = reports {
            let from = sink.next_cursor(server.uid);
            let req = if from > 0 {
                ReportSyncRequest::resume(from)
            } else {
                ReportSyncRequest::fresh(ReportHistoryDepth::All)
            };
            match client.reports().sync(req) {
                Ok(_) => log::info!(
                    "отчёты: core={} sync запрошен (from_rec_id={from})",
                    server.uid
                ),
                Err(e) => log::warn!("отчёты: core={} sync не запустился: {e:?}", server.uid),
            }
            // Open rows may have closed or been deleted offline BELOW the cursor. Register them
            // for checking; the library retains the set and repeats it on hard reconnect, and the
            // results arrive as ordinary RowUpsert/RowDelete events.
            let open = sink.open_rows(server.uid);
            if !open.is_empty() {
                if let Err(e) = client.reports().check_open_rows(&open) {
                    log::warn!("отчёты: core={} check_open_rows не ушёл: {e:?}", server.uid);
                }
            }
        }
    }

    // The core's market role, assigned by the coordinator through SetMarket.
    // is_provider controls whether ALL exchange trades are retained via subscribe_all_trades.
    // wanted lists the markets actively served through subscriptions and the snapshot source.
    let mut is_provider = false;
    let mut wanted: Vec<String> = Vec::new();
    // Markets with order-book subscriptions: a subset of wanted, enabled in at least one window.
    let mut wanted_orderbook: Vec<String> = Vec::new();
    let mut identity_sent = false;
    let mut last_orders = Instant::now();
    let mut orders_table_pending = false;
    let mut last_strats = Instant::now();
    // Assets snapshot rate cap: minimum 1 s between publishes while the Assets view is active,
    // otherwise 5 s. Publication still requires a domain event; this is not a periodic timer.
    let mut last_assets = Instant::now();
    // Throttle for proactive CORE balance requests after fills; see the block in the loop.
    let mut last_balance_refresh = Instant::now();
    let mut last_wallet_refresh = Instant::now();
    // Window for logging Balance events after our refresh, used to diagnose phantom Assets entries.
    let mut balance_refresh_log_until: Option<Instant> = None;
    // Transfer-assets cursor: publish only when the revision changes (request/response).
    let mut last_transfer_rev: u64 = u64::MAX;
    // Strategy-export cursors: schema revision plus the contents/checked signature. Publish only
    // changes because strategy fields are expensive and need not be sent every second.
    let mut last_schema_rev: u64 = u64::MAX;
    let mut last_strat_sig: u64 = u64::MAX;
    // strat_db: per-kind defaults for dump normalization, a 30-second timestamp heuristic for
    // `origin=local`, and the flag for the first set published by this run, whose origins can
    // predate the run. Recent remote changes can look local, delayed local echoes can look remote,
    // and an internal reconnect does not reset the first-set flag.
    let mut strat_schema_defaults: std::collections::HashMap<
        u8,
        Vec<(String, moonproto::FieldValue)>,
    > = std::collections::HashMap::new();
    let mut local_strat_edits = LocalStratEdits::new();
    let mut strat_db_initial = true;
    // Monotonic per-core detect number used as the ingestion cursor for the UI detect feed.
    let mut detect_seq: u64 = 0;
    // File writer for this core's server log (logs/<date>_<core>.log), with daily rotation. Write
    // on the FEED THREAD rather than the UI thread because log volume is high and the UI must not
    // wait for disk. Only an in-memory copy reaches the UI for live viewing and search.
    let mut log_writer = crate::applog::DatedWriter::new(&server.name);
    let mut events = Vec::new();
    let mut lifecycle_events = Vec::new();
    let mut force_market_sample = false;
    loop {
        // `SetMarket` contains complete desired state; the other coordinator commands are deltas
        // or actions. A closed channel means the coordinator has exited, so disconnect.
        let mut orders_mutated = false;
        if drain_commands(
            cmd_rx,
            &client,
            server,
            &mut is_provider,
            &mut wanted,
            &mut wanted_orderbook,
            &mut force_market_sample,
            &mut orders_mutated,
            &mut local_strat_edits,
            client_settings_sequence,
        ) {
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
                        server.id,
                        code.stable_id(),
                        dex,
                        id
                    );
                    let _ = tx.send(FeedMsg::Identity(id));
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
            let _ = tx.send(FeedMsg::Status(st));
            if request_license_state {
                if let Err(error) = client.settings().request_kernel_license_state() {
                    log::warn!(
                        "core {} request kernel license state failed: {error}",
                        server.id
                    );
                }
                // Request the complete ClientSettings snapshot (TP/SL/sell/...). The core sends
                // LevManage/RuntimeState after connection itself, so only refresh settings here.
                if let Err(error) = client.settings().refresh() {
                    log::warn!("core {} request client settings failed: {error}", server.id);
                }
                // Account hedge mode for the toolbar toggle.
                if let Err(error) = client.account().refresh_hedge_mode() {
                    log::warn!("core {} request hedge mode failed: {error}", server.id);
                }
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
                        server.id
                    );
                }
                // Chart alerts are authoritative on the core. Request a full snapshot after
                // initialization/reconnect so the local set cannot lag behind the server.
                if server.feed.alerts {
                    if let Err(error) = client.chart_alerts().request_snapshot() {
                        log::warn!(
                            "core {} request chart alerts snapshot failed: {error}",
                            server.id
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
        // Filling/cancelling an order changes position and balance, but the core does NOT always
        // push a fresh balance. A sold token can remain as a phantom in Assets until reconnect
        // because the snapshot goes stale. Proactively request a fresh snapshot FROM THE CORE:
        // `Command::Balance` goes to the core (Delphi `SendBalanceCmd`), NOT the exchange, so it is
        // cheap. Throttle to 3 s so a sequence of fills does not spam requests. The response arrives
        // as a Balance event on the next iteration and updates the snapshot.
        if has_orders_table_event && last_balance_refresh.elapsed() >= Duration::from_secs(3) {
            last_balance_refresh = Instant::now();
            match client.balances().refresh() {
                // Diagnose phantom Assets entries by recording the request and, below in the event
                // loop, WHAT the core returned: a full snapshot (SnapshotApplied, which clears
                // missing coins) or an increment (IncrementalApplied, which does NOT clear a coin
                // whose balance reached zero, exposing a core-side gap).
                Ok(()) => {
                    balance_refresh_log_until = Some(Instant::now() + Duration::from_secs(5));
                    log::info!(
                        "core {} balance refresh requested (orders event)",
                        server.id
                    );
                }
                Err(error) => {
                    log::warn!("core {} balance refresh request failed: {error}", server.id)
                }
            }
        }
        // On wallet-based spot exchanges such as Bitget and Hyperliquid spot, purchased coins exist
        // ONLY in transfer_assets; the core sends no per-market balances. Without polling again, a
        // newly purchased coin does not appear in Assets until the core is clicked manually. Query
        // one Spot wallet rather than all three and do it less often than balance refreshes because
        // the core forwards this request to the exchange (CheckAssets can time out in core logs).
        if has_orders_table_event && last_wallet_refresh.elapsed() >= Duration::from_secs(10) {
            last_wallet_refresh = Instant::now();
            if let Err(error) = client
                .balances()
                .refresh_transfer_assets_kind(moonproto::ExchangeKind::Spot)
            {
                log::warn!(
                    "core {} spot wallet refresh request failed: {error}",
                    server.id
                );
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
                                server.id,
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
                                server.id,
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
                            server.id,
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
                        server.id,
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
                    log::warn!("core {} candles snapshot failed: {error}", server.id);
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
                        server.id
                    );
                }
                // Diagnostic window after our balance refresh for phantom Assets entries. Response
                // type determines the stuck coin's fate: Snapshot clears missing entries while
                // Incremental does not. Do not log outside this window because balances are pushed
                // continuously.
                Event::Balance(bev) => {
                    if balance_refresh_log_until.is_some_and(|t| Instant::now() < t) {
                        log::info!("core {} balance event after refresh: {bev:?}", server.id);
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
        // Hedge mode arrives directly in the Engine API response event.
        let hedge_mode = events.iter().find_map(|ev| match ev {
            Event::Account(AccountEvent::HedgeModeUpdated { hedge_mode, .. }) => Some(*hedge_mode),
            _ => None,
        });
        if let Some(on) = hedge_mode {
            if tx.send(FeedMsg::HedgeMode(on)).is_err() {
                break;
            }
        }
        let dirty_markets = if is_provider && !wanted.is_empty() {
            market_dirty_from_events(&events, &wanted, force_market_sample)
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
                        // Write to disk immediately through the buffer; split time into date and clock.
                        let (date, hms) = crate::applog::split_unix_ms(ms);
                        log_writer.write(&date, &hms, "INFO", "", &l.msg);
                        logs.push(CoreLogLine {
                            time_ms: ms,
                            recv_ms,
                            msg: l.msg.clone(),
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
                    // Typed report-database replica: send schema/rows/catch-up completion to the
                    // SQLite writer, the sole owner of the write connection.
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
                                    "отчёты: core={} sync начат (from_rec_id={})",
                                    server.uid,
                                    request.from_rec_id,
                                ),
                                // Catch-up page: the writer applies it in a transaction and
                                // acknowledges it with `page_applied` after commit. Until then the
                                // library does not request another page. The writer also handles
                                // `database_recreated` by wiping data, and the library restarts itself.
                                ReportEvent::SyncPage(page) => sink.send(DbMsg::Page {
                                    core_uid: server.uid,
                                    core_name: server.name.clone(),
                                    page: page.clone(),
                                    ack: client.reports(),
                                }),
                                ReportEvent::SyncComplete(done) => {
                                    sink.send(DbMsg::SyncComplete {
                                        core_uid: server.uid,
                                        done: done.clone(),
                                    });
                                }
                                ReportEvent::OpenRowsCheckStarted { rec_ids } => log::info!(
                                    "отчёты: core={} проверка открытых строк начата ({} шт)",
                                    server.uid,
                                    rec_ids.len(),
                                ),
                                ReportEvent::OpenRowsCheckComplete { rec_ids } => log::info!(
                                    "отчёты: core={} проверка открытых строк завершена ({} шт)",
                                    server.uid,
                                    rec_ids.len(),
                                ),
                                ReportEvent::SchemaRejected { reason } => log::error!(
                                    "отчёты: core={} схема отвергнута: {reason}",
                                    server.uid,
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
                            "отчёты(diag): core={} пакет отвергнут: cmd={cmd:?} len={len}{extra}",
                            server.uid,
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
        if had_domain_event
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

                    // Send the same snapshot to the local strat_db (head plus versions based on
                    // content diffs; the writer deduplicates echoes and cosmetic changes). Wait for
                    // the schema because dumps are incomplete until its defaults are materialized,
                    // which would create phantom versions when the schema arrives.
                    if !strat_schema_defaults.is_empty() {
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
                            sink.send(crate::strat_db::StratMsg::FullSet {
                                core_uid: server.uid,
                                core_name: server.name.clone(),
                                initial: strat_db_initial,
                                strategies: dumps,
                            });
                            strat_db_initial = false;
                        }
                    }
                }
            }
        }

        // Core assets for the Assets window: after a domain event, publish no more often than about
        // 1 Hz while the window is active. Without the window, the 5-second value is a minimum
        // interval, not a periodic publish; a quiet core emits nothing. The snapshot is still needed
        // for header balance and leverage, but rebuilding every market for N cores on every event is
        // needless work. Prices come from the market, so publish the full snapshot; the UI gates
        // repainting by placing assets_rev into one-second buckets. Publish transfer assets only on
        // revision changes.
        let assets_every = if crate::feed::assets_view_active() {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(5)
        };
        if had_domain_event && last_assets.elapsed() >= assets_every {
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

        let order_wait = if server.feed.orders && orders_table_pending {
            let elapsed = last_orders.elapsed();
            Some(Duration::from_millis(250).saturating_sub(elapsed))
        } else {
            None
        };
        let wake_result = match order_wait {
            Some(timeout) => wake_rx.recv_timeout(timeout).map_err(|err| match err {
                std::sync::mpsc::RecvTimeoutError::Timeout => None,
                std::sync::mpsc::RecvTimeoutError::Disconnected => Some(()),
            }),
            None => wake_rx.recv().map_err(|_| Some(())),
        };
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
