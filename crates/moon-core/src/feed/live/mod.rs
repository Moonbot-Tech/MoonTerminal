//! Live backend that owns one Moonbot core connection and its MoonProtoBeta event loop.
//!
//! Flow: event-driven. `MoonEventSink` wakes the backend thread after an actual event; market data
//! remains in an immutable read-model snapshot, and only a lightweight signal reaches this module.
//!
//! `run()` is the main event loop; commands live in [`commands`], persistent market assignment in
//! [`market_role`], pure moonproto-to-terminal converters in [`convert`], and dirty-market
//! calculation in [`dirty`].

mod alive_map;
mod client_slot;
mod loop_helpers;
mod publish_gate;
mod run_guards;
mod strategy_publish;

use alive_map::{AliveAction, alive_map_action};
pub(crate) use client_slot::connection_target;
use client_slot::{ClientSlotGuard, RunStateSeen};
use loop_helpers::{
    closed_row_uid, hex_dump, should_publish_assets, tg_opened, trace_field_indices,
};
use publish_gate::{ORDERS_TABLE_PERIOD, OrdersPublish};
pub(in crate::feed) use run_guards::{EndpointUnusable, KeyUnreadable, NeverOperational};
use run_guards::{reconnect_is_operational, run_failed, startup_poll_settled};
use strategy_publish::{
    apply_strategy_delivery_ack, record_strategy_edit_resolution, strategy_db_export_due,
    strategy_edit_sig,
};

use super::{StrategyFieldChange, strategies};

mod account_reconciliation;
mod archive_probe;
mod capture;
mod client_settings;
mod commands;
mod convert;
mod deadline;
mod dirty;
mod identity_refresh;
mod market_role;
mod pc_clock;
mod ping_clock;
mod report_sync;
mod shared_config;
mod stale;
mod startup_watchdog;
mod telegram;
mod temp_blacklist;
#[cfg(test)]
mod tests;
mod trace_backfill;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, sync_channel};
use std::time::{Duration, Instant, SystemTime};

use moonproto::state::{
    AccountEvent, MarketHistorySizing, OrderEvent, SettingsEvent, StratEvent, StrategyEditStatus,
};
use moonproto::{
    ClientConfig, ConnectConfig, Event, InitConfig, InitialStrategies, LifecycleEvent, MoonClient,
    MoonEventSink, MoonStateSnapshot, ReportAliveMapOutcome, ReportAliveMapTicket, ReportEvent,
    ReportHistoryDepth, ReportSyncCheckpoint, ReportSyncComplete, ReportSyncRequest, TransportMode,
};

use self::trace_backfill::TracePacer;
use super::assets::{build_assets, build_transfer_assets};
use super::strategies::{
    alert_params, build_schema_model, detect_strat_name, fmt_field, schema_default_fields,
    strat_db_dump, strat_display_name, strat_field_bool, strat_kind_name, tg_detect,
};
use super::{
    ChartTextRows, ConnStatus, CoreConfigEditEvent, CoreLogLine, CoreStartupStatus, CoreTgEvent,
    CoreTimeOffsetStatus, DetectRow, ExchangeId, FeedMsg, FeedTx, LatestMarketRole,
    SharedMoonClient, StrategyEditPhase, StrategyEditResolution, StrategyEditResult,
    StrategyEditRow, StrategyEditSnapshot, StrategyRow,
};
use crate::config::{ServerConfig, TransportVersion};
use crate::db::order_traces::{AskSink, TraceDbMsg};
use crate::db::{DbMsg, ReportStart, ReportTx};
use crate::session::core_time_offset::OffsetSource;
use crate::util::{now_unix_ms as now_ms, now_unix_ms_i64 as now_ms_i64};

/// How often a still-starting core's startup snapshot is read.
///
/// Fast enough that the progress figure visibly advances, slow enough that the store's own
/// whole-second gate on `elapsed_ms` dominates it rather than the other way round. It costs nothing
/// on a settled core: the poll is gated on the core still coming up. If MoonProto publishes more
/// slowly than this, the extra reads simply return the same snapshot and
/// `CoreStartupStatus::progress_eq` suppresses the send — the cadence can be too fast, never wrong.
const STARTUP_POLL: Duration = Duration::from_millis(500);

use account_reconciliation::{
    AccountReconciliation, BALANCE_TRACE_LEVEL, balance_refresh_log_window,
};
pub(in crate::feed) use client_settings::ClientSettingsSequence;
pub(in crate::feed) use commands::ChartTextWanted;
use commands::{CommandDrain, LocalStratEdits, StrategyPlacementGuard, drain_commands};
use convert::{
    build_order_rows, client_settings_from_proto, license_state_from_proto,
    profit_state_from_proto, runtime_state_from_proto, settings_event_snapshot,
    sys_status_from_proto, telegram_from_proto,
};
pub use convert::{percentage_stop_price, percentage_take_price};
use deadline::CoalescedDeadline;
use dirty::market_dirty_from_events;
pub(in crate::feed) use market_role::MarketRoleState;
pub use shared_config::FieldMask;
pub(in crate::feed) use shared_config::SharedConfigSequence;
/// The only way to build a [`crate::feed::CoreConfig`] — it deliberately has no `Default` — which
/// the session store's own tests need to drive its edit-row rules with.
///
/// Behind `cfg(test)` because nothing in a shipping build has any business making a projection:
/// the wire makes them and the UI reads them.
#[cfg(test)]
pub(crate) use shared_config::core_config_from_proto;
use startup_watchdog::{StartupStalled, StartupWatchdog};

/// `ServerInfo` allocation observed at `ServerRestart`, pinned until version publication.
///
/// `_snap` is the `Arc<MoonStateSnapshot>` from `client.snapshot()`. That snapshot owns a clone of
/// the inner `Arc<ServerInfo>`, so `addr` stays allocated and cannot be handed to the replacement
/// process's BaseCheck while this value is held. Dropping it releases the pin.
struct PinnedRestartInfo {
    _snap: Arc<MoonStateSnapshot>,
    addr: usize,
}

impl PinnedRestartInfo {
    /// Pin `snap` and record the address of its `ServerInfo`.
    ///
    /// Args:
    ///     snap: Snapshot returned by `client.snapshot()` at `ServerRestart`.
    ///
    /// Returns:
    ///     The pin. [`Self::addr`] is compared and never dereferenced.
    fn pin(snap: Arc<MoonStateSnapshot>) -> Self {
        Self {
            addr: snap.server_info() as *const _ as usize,
            _snap: snap,
        }
    }

    /// Address of the pinned `ServerInfo`.
    ///
    /// Returns:
    ///     The allocation address captured in [`Self::pin`]. Compared, never dereferenced.
    fn addr(&self) -> usize {
        self.addr
    }
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
///     shared_config_sequence: Reconnect-safe safe-share configuration sequence.
///     market_role: Market-provider role retained between attempts.
///     latest_market_role: Latest successfully queued role, independent of the bounded backlog.
///     chart_text: Last requested overlay market, retained between attempts like `market_role`.
///
/// Returns:
///     Success after orderly shutdown, or the terminal setup/live-loop error.
///
/// Errors:
///     Returns [`KeyUnreadable`] when the configured key cannot be decoded; otherwise an error
///     when the client cannot connect, or the event loop fails.
pub(super) fn run(
    server: &ServerConfig,
    chart_memory_percent: u16,
    tx: &FeedTx,
    cmd_rx: &Receiver<crate::feed::QueuedCmd>,
    wake_tx: &Sender<()>,
    wake_rx: &Receiver<()>,
    reports: Option<&ReportTx>,
    client_slot: SharedMoonClient,
    client_settings_sequence: &mut ClientSettingsSequence,
    shared_config_sequence: &mut SharedConfigSequence,
    market_role: &mut MarketRoleState,
    latest_market_role: &LatestMarketRole,
    chart_text: &mut ChartTextWanted,
) -> anyhow::Result<()> {
    let _ = tx.send(FeedMsg::Status(ConnStatus::Connecting));
    // Read once per client: the switch is set before any feed starts and never cleared.
    let station = crate::feed::station::profile();
    // The account path — orders, balances, run state, Assets: a terminal's, and the Mini App
    // station's; the light station runs none of it.
    let account = station.is_none_or(crate::feed::station::Profile::runs_account);
    market_role.begin_client();
    chart_text.begin_client();

    // 1. Decode the key into master/MAC keys and its suggested network.
    let raw = server.key.expose();
    let Some(info) = moonproto::parse_key_info(raw) else {
        let empty = raw.trim().is_empty();
        let _ = tx.send(FeedMsg::ConnFault(convert::key_fault(empty)));
        return Err(KeyUnreadable { empty }.into());
    };

    // 2. Derive the endpoint from the key, which embeds host and port, with the row's hand-typed
    //    override laid over it; the transport mode, too, is only seeded by the key, and a
    //    configured one wins (see `connection_target`). An override that is not an address is
    //    reported, never quietly replaced by the key's: the user typed it to stop dialing that.
    let Ok(endpoint_override) = crate::config::parse_endpoint_override(&server.endpoint_override)
    else {
        let _ = tx.send(FeedMsg::ConnFault(convert::endpoint_fault(false)));
        return Err(EndpointUnusable { unresolved: false }.into());
    };
    let (target, transport) = connection_target(
        info.network.as_ref(),
        server.transport,
        endpoint_override.as_ref(),
    );
    // A host name blocks here on the system resolver; this is the core's own feed thread.
    let Ok(resolved) = target.resolve() else {
        let _ = tx.send(FeedMsg::ConnFault(convert::endpoint_fault(true)));
        return Err(EndpointUnusable { unresolved: true }.into());
    };
    let endpoint = resolved.endpoint;
    let port = endpoint.port;
    let host = resolved.client_host;
    // The resolver above can block for seconds, and a Save in the meantime respawns this core on
    // a NEW thread and drops this one's receiver. A thread that wakes up superseded must stop here,
    // before it builds a client and dials the core with the old address and the same key.
    if tx.send(FeedMsg::Endpoint(endpoint)).is_err() {
        return Ok(());
    }
    // Name the core, never its address: the endpoint still reaches the UI through `FeedMsg::
    // Endpoint` for Core Status, but a log file is shared far more casually than a screen is.
    log::info!(
        "live connect core={} market={} transport={transport:?}",
        server.name,
        server.market
    );

    let client_cfg = ClientConfig::new(host, port, info.keys.master_key, info.keys.mac_key)
        .with_transport_mode(transport);
    // The station charts nothing and elects no provider: the smallest rings. The light station
    // also drops the periodic market-list and tag refresh — the Init steps still load the catalog
    // once; the Mini App's keeps it, since its balances are priced from those ticks.
    let client_cfg = match station {
        Some(profile) => {
            let compact = client_cfg.with_market_history(MarketHistorySizing::Compact);
            if profile.runs_account() {
                compact
            } else {
                compact.with_refresh(moonproto::RefreshConfig {
                    update_markets_every: None,
                    check_tags_every: None,
                })
            }
        }
        None => client_cfg.with_market_history(MarketHistorySizing::auto_with_budget_percent(
            chart_memory_percent,
        )),
    };

    // 3. Initialize WITHOUT market subscriptions. The coordinator assigns the core's market role
    //    via SetMarket after learning its exchange (Identity) and electing a provider. Only ONE
    //    core per exchange calls subscribe_all_trades; the others publish account data only. This
    //    fetches exchange trades once instead of once from each of up to 200 cores.
    //    initial_strategies IS REQUIRED, or initialization hangs after Connected.
    //    Whether the core sends its log at all is the user's `log_delivery` (Connections). The
    //    station never asks for it — the clock offset comes from the Ping (`ping_clock`) and it
    //    keeps no journal; a core older than the request keeps sending, and its event filter
    //    drops the lines.
    let init = InitConfig {
        initial_strategies: Some(InitialStrategies::new(0, Vec::new())),
        subscribe_logs: server.feed.log_delivery && station.is_none(),
        ..Default::default()
    };

    // `connect` is nonblocking; bound the transport/authorization phase before the per-step Init
    // deadlines and the terminal-owned startup watchdog take over.
    let event_wake_tx = wake_tx.clone();
    let (event_sink, event_queue) = MoonEventSink::queue_with_waker(move || {
        let _ = event_wake_tx.send(());
    });
    let client = Arc::new(MoonClient::connect_with_sink(
        client_cfg,
        ConnectConfig::new(init).with_connect_timeout(Duration::from_secs(15)),
        event_sink,
    )?);
    let client_epoch = client_slot.set(Some(client.clone()));
    let _client_slot_guard = ClientSlotGuard {
        slot: client_slot.clone(),
    };

    // Typed report-database replica: declare catch-up immediately. It remains an intent that the
    // library sends after connection and repeats on its own after a hard reconnect. Where it
    // starts comes from the writer's durable start state: a checkpoint pairs the numeric cursor
    // with the core's database epoch, so a replaced core database is detected even when the new
    // one already grew past the old ids. Catch-up is PAGED: the next page is not requested until
    // the writer commits and acknowledges the current one (backpressure by design).
    //
    // Whether an open-row check has STARTED since this feed last asked for one. An
    // `OpenRowsCheckComplete` names only its ids, not the request it answers, so a completion of a
    // check that started before a reconnect can be read after the reconnect restarted the walk —
    // and, when nothing changed, it carries exactly the new walk's first page. Only a check the
    // library began after the latest request may move the walk on (#742).
    let mut open_rows_check_started = false;
    // Catch-up progress for the status bar, published only while the replica is written.
    let mut report_sync = report_sync::ReportSyncTracker::default();
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
            // Open rows may have closed or been deleted offline BELOW the catch-up start. The
            // writer reads them and registers them for checking; the results arrive as ordinary
            // RowUpsert/RowDelete events. Repeated on every reconnect below.
            sink.send(DbMsg::RecheckOpenRows {
                core_uid: server.uid,
                after: None,
                reports: client.reports(),
            });
        }
    }

    let mut identity_sent = false;
    // Tracked separately from `identity_sent`: the two facts come from one payload but are gated
    // on different fields, so one can be publishable while the other is not.
    let mut version_sent = false;
    // Snapshot captured at the last `ServerRestart`, held so its `ServerInfo` address cannot
    // be reused until publication. `None` after publication, so a same-process
    // reconnect still republishes MoonProto's retained snapshot.
    let mut version_restart_info: Option<PinnedRestartInfo> = None;
    // Which run-state halves the MoonBot instance CURRENTLY on the other end has reported.
    //
    // Not "has the terminal ever seen them": MoonProto keeps its retained state across a server
    // restart, so the snapshot can hold a previous instance's answer, and the reconnect republish
    // must not pass that off as this one's. Reset by `LifecycleEvent::ServerRestart`.
    let mut run_state_seen = RunStateSeen::default();
    // The order table's publication throttle. One value, not an `Instant` plus a `bool`: the
    // loop below sleeps on the earliest of every deadline it owns, so "is it due" and "how long
    // until it is" have to come from the same state, or the two answers drift. See `deadline.rs`.
    // `idle_since`, not `new`: the table starts life up to date, and arming the cooldown here
    // keeps the first publication of a connection where it has always been rather than one
    // interval earlier.
    let mut orders_table = CoalescedDeadline::idle_since(ORDERS_TABLE_PERIOD, Instant::now());
    // Latched while the client has no state snapshot to build order rows from, so the condition is
    // reported once per episode instead of on every turn.
    let mut snapshotless_orders = false;
    let mut last_strats = Instant::now();
    // Assets snapshot rate cap: minimum 1 s between publishes while the Assets view is active,
    // otherwise 5 s. Publication still requires a domain event; this is not a periodic timer.
    let mut last_assets = Instant::now();
    // Coalesced repair requests for account changes that need a fresh balance or wallet snapshot,
    // plus the recurring API-key expiration poll.
    let mut account_reconciliation = AccountReconciliation::new(Instant::now());
    let mut trade_sounds = crate::feed::trade_sound::TradeSoundState::default();
    // Window for logging Balance events after our refresh, used to diagnose phantom Assets entries.
    let mut balance_refresh_log_until: Option<Instant> = None;
    // Transfer-assets cursor: publish only when the revision changes (request/response).
    let mut last_transfer_rev: u64 = u64::MAX;
    // Strategy-export cursors: schema revision plus the contents/checked signature. Publish only
    // changes because strategy fields are expensive and need not be sent every second.
    let mut last_schema_rev: u64 = u64::MAX;
    let mut last_strat_sig: u64 = u64::MAX;
    // Folder-tree cursors. Two of them because the tree moves for two independent reasons: the core
    // versions it whenever a folder is created or deleted, and the folders the STRATEGIES imply
    // change whenever a strategy's path does — which the version does not see.
    let mut last_folders_version: i64 = i64::MIN;
    // Seeded with the digest of an EMPTY tree, which is what a core reports before it has said
    // anything: `u64::MAX` would make the first pass publish that nothing-yet as a change.
    let mut last_folders_digest: u64 = 0;
    // Strategy-edit publish cadence, independent of the 1 Hz strategies gate below. The retained
    // latch is set on ANY edit event and cleared only once a publish actually goes out, so a
    // resolution arriving inside the 250 ms shadow of the previous publish is delayed, never
    // dropped, by the rate limit. `strat_edit_desired_cache` remembers each pending edit's desired
    // snapshot from the moment it was submitted, because moonproto removes the edit from its map
    // in the same step that resolves it. The ver/last_date pair used to be enough to log the
    // revision; naming the fields an Adjusted echo changed needs the values too.
    let mut last_strat_edit_pub = Instant::now();
    let mut last_strat_edit_sig: u64 = 0;
    let mut strat_edit_publish_pending = false;
    let mut pending_strat_edit_notes: Vec<StrategyEditResolution> = Vec::new();
    let mut strat_edit_desired_cache: std::collections::HashMap<u64, moonproto::StrategySnapshot> =
        std::collections::HashMap::new();
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
    // Last HyperLiquid quota written to `channels.hl_limit`, so the channel reports the value the
    // snapshot HOLDS and not only the moments it changes. `None` means nothing written yet.
    let mut hl_last_logged: Option<Option<u64>> = None;
    // The core's temporary blacklist: what the UI was last told, what the diagnostic channel last
    // wrote, and the test that tells an edit from a countdown.
    let mut temp_blacklist = temp_blacklist::TempBlacklistPublisher::new();
    // The catch-up whose alive map this feed is waiting for, paired with its request ticket.
    // A `run()` local on purpose: a hard drop ends this loop and discards it, and the next `run()`
    // re-syncs from the writer's durable start state and reconciles again. Hoisting it into the
    // shared sink would let a stale completion outlive the connection that produced it.
    let mut pending_alive: Option<(ReportAliveMapTicket, ReportSyncComplete)> = None;
    // Archived order traces. EVERY `request_traces` this connection sends goes through one paced
    // queue — a window's ask, a row that just closed, the startup backfill — and the writer of the
    // local trace store is the one that says what is still unknown, answering through
    // `trace_ask_rx`. Locals of this attempt like `pending_alive`: a reconnect starts the queue
    // empty, and MoonProto fails what was in flight on its own.
    let mut trace_pacer = TracePacer::default();
    let trace_sink = crate::db::order_traces::sink();
    let (trace_ask_sink, trace_ask_rx) = AskSink::new(wake_tx.clone());
    // Field indices of `ReportUID` and `CloseDate`, resolved once per schema revision as the
    // protocol asks, never by name per row.
    let mut trace_fields: Option<(u16, u16)> = None;
    // Per-feed memory of open report rows, so a partial closing upsert can be completed into
    // a print capture — see `capture::CaptureTracker`.
    let mut capture: Option<capture::CaptureTracker> = None;
    // File writer for this core's server log (logs/<date>_<core>.log), with daily rotation. Write
    // on the FEED THREAD rather than the UI thread because log volume is high and the UI must not
    // wait for disk. Only an in-memory copy reaches the UI for live viewing and search.
    let mut log_writer = crate::applog::DatedWriter::new(&server.name);
    let mut events = Vec::new();
    let mut lifecycle_events = Vec::new();
    // A fresh client's first market list can report every market as new (a price row or a
    // listing notice before it flags a refresh), so turnover counts only once this client has
    // applied a list before the current batch.
    let mut turnover_armed = false;
    let mut force_market_sample = false;
    // Latest lifecycle state, and whether this core has already failed one API-key check: an older
    // MoonBot never answers that method, and a warn per retry for the life of the session would be
    // noise. The first failure is worth seeing; the rest are not.
    let mut is_ready = false;
    // When the connection last became operational, `None` while it is not: the gate a live action
    // in the command queue must pass (`stale`) so one that waited out an outage is not delivered.
    let mut ready_since: Option<Instant> = None;
    let mut api_expiry_failed_before = false;
    // Per-connection clock offset, sampled from the core's Ping on its own deadline whatever the
    // feed's flags say; see the sampling step at the end of the loop and `note_ready` at Ready.
    // A verdict about the log request belongs to the connection that reached it: cleared here and
    // again at every Ready, since an in-run reconnect may land on a core that was upgraded.
    let _ = tx.send(FeedMsg::LogDeliveryIgnored(false));
    let log_delivery_off = !server.feed.log_delivery && station.is_none();
    let mut log_delivery_ignored = false;
    let mut ping_clock = ping_clock::PingClock::new(Instant::now());
    // The offset in force, read once per connection: one indexed row from the replica the writer
    // keeps. Without a report sink nothing is stored for this core and nothing would be adopted
    // into the axis anyway (the adoption below is gated on the same sink). A read that fails
    // starts from `None`, which only means the first adoption is compared with nothing — the
    // writer still skips a value equal to the stored one.
    // The segment's start rides along: a confirmation re-publishes it unchanged, so the report axis
    // the UI builds from this status (`report_axis`, whose segment starts at `observed_at_utc`)
    // does not move — a moved axis re-queries the Report and resets Analytics.
    let mut segment_start_ms: Option<i64> = None;
    if reports.is_some() {
        let stored = crate::db::open_reader()
            .ok()
            .and_then(|conn| crate::db::latest_segment(&conn, server.uid));
        ping_clock.seed(stored.map(|(offset, _)| offset));
        segment_start_ms = stored.map(|(_, from_utc)| from_utc.saturating_mul(1_000));
    }
    // Startup telemetry is POLLED, not pushed: MoonProto publishes it as a passive snapshot behind
    // a lock at its own bounded rate, so nothing wakes us when it advances. `startup_sent` is the
    // last snapshot that actually left, so the send gate compares against what the store holds
    // rather than re-sending an unchanged one.
    let mut last_startup = Instant::now();
    let mut startup_sent: Option<CoreStartupStatus> = None;
    // Whether MoonProto has finished this client's one-time initialization, and the clock that
    // gives up when it never does. Both exist because `Connected { fresh: false }` does NOT mean
    // "already initialized" — see `reconnect_is_operational`, which owns that rule and the reason
    // for it.
    let mut init_completed = false;
    let mut startup_watchdog = StartupWatchdog::default();
    // Stamps each `Submitted` row's wall-clock time (`SharedConfigSequence` has none of its own)
    // and forwards the batch as `FeedMsg::CoreConfigEdit`. Returns `false` on a closed channel, the
    // same break-on-send-error idiom every other publication in this loop follows.
    let send_core_config_events = |batch: Vec<CoreConfigEditEvent>| -> bool {
        for event in batch {
            let event = match event {
                CoreConfigEditEvent::Submitted(mut row) => {
                    row.submitted_at_ms = now_ms_i64();
                    CoreConfigEditEvent::Submitted(row)
                }
                other => other,
            };
            if tx.send(FeedMsg::CoreConfigEdit(event)).is_err() {
                return false;
            }
        }
        true
    };
    loop {
        // `SetMarket` contains complete desired state; the other coordinator commands are deltas
        // or actions. A closed channel means the coordinator has exited, so disconnect.
        let mut orders_mutated = false;
        let mut problems_relist = false;
        let mut core_config_events = Vec::new();
        // Trace asks from windows: queued ahead of the backfill and sent below at the pacer's rate.
        let mut trace_asks = Vec::new();
        let command_drain = drain_commands(
            cmd_rx,
            &client,
            server,
            latest_market_role,
            market_role,
            &mut force_market_sample,
            &mut orders_mutated,
            &mut problems_relist,
            &mut local_strat_edits,
            &mut strategy_placements,
            client_settings_sequence,
            shared_config_sequence,
            &mut core_config_events,
            chart_text,
            &mut trace_asks,
            ready_since,
        );
        if command_drain == CommandDrain::Disconnected {
            return Ok(());
        }
        for uid in trace_asks {
            trace_pacer.push_front(uid);
        }
        while let Ok(uids) = trace_ask_rx.try_recv() {
            trace_pacer.push_back(uids);
        }
        // A request that never left this process is reported through the same message the core's
        // own answer takes, so a window waiting on it sees a failure rather than silence — and it
        // frees its slot like an answered one.
        let trace_now = Instant::now();
        while let Some(report_uid) = trace_pacer.take_due(trace_now) {
            let Err(error) = client.reports().request_traces(report_uid) else {
                continue;
            };
            log::warn!(
                "core {} request_traces({report_uid}) failed: {error}",
                crate::feed::core_label(server.id)
            );
            trace_pacer.answered(report_uid, false);
            if tx
                .send(FeedMsg::ReportTraces {
                    report_uid,
                    outcome: crate::feed::ReportTracesOutcome::Failed(error.to_string()),
                })
                .is_err()
            {
                return Ok(());
            }
        }
        // The edit events the drain produced are NOT sent here: they go out below, after this
        // iteration's configuration snapshot, together with the ones the drive there produces —
        // see the send at the end of the settings block.
        // Publish an immediate best-effort order snapshot after a flagged command, bypassing the
        // event gate and 250 ms throttle. Some retained-order edits may already be visible locally,
        // but queued work such as new-order creation may not be; this snapshot can precede the
        // asynchronous mutation, and later order events reconcile the rows.
        // Without a snapshot this simply does not publish: queuing the table here would arm a
        // wake for work that, in the only state where the snapshot is missing, can never run.
        if orders_mutated && server.feed.orders {
            if let Some(snap) = client.snapshot() {
                snapshotless_orders = false;
                let order_rows = build_order_rows(server.id, &snap, &[]);
                orders_table.mark_attempt(Instant::now());
                if tx.send(FeedMsg::Orders(order_rows)).is_err() {
                    break;
                }
            }
        }

        // Publish what `server_info` reported after BaseCheck: the core's exchange (for grouping
        // and provider election) and its MoonBot build. One snapshot read, TWO independent
        // publications, each once.
        //
        // The build number deliberately does NOT ride the identity gate below. A venue is what
        // `exchange_code` establishes; a build number is a fact about the core process itself, and
        // a core that reports no exchange code — which lands it in the unidentified group, the
        // rows an operator inspects most — would otherwise never report its build either.
        if !identity_sent || (is_ready && !version_sent) {
            if let Some(snap) = client.snapshot() {
                // Address of the Arc's `ServerInfo`, not of this clone. `set_session_identity`
                // allocates a new Arc when the new process's BaseCheck lands, even if the bytes
                // compare equal. Held only for the comparison below.
                let info_addr = snap.server_info() as *const _ as usize;
                let info = snap.server_info().clone();
                // Gated on `is_ready`, which carries the last drained lifecycle batch's verdict.
                // MoonProto sets the snapshot behind `server_info` ONCE, at the first Ready, and
                // never clears it for an internal reconnect — so it keeps answering `Some` all
                // through a `Reconnecting` episode. Without this gate the latch release below would
                // republish the build on the very next pass and repopulate a store that had just
                // cleared it, leaving a core displaying a build while it is visibly not connected.
                //
                // A `ServerRestart` withholds only the letter from the pinned snapshot: event
                // draining and snapshot refresh are unordered, so the pin can already be fresh.
                // Always publish the number so Waiting can advance. The respawned client's new
                // run has no pin and publishes the fresh letter; judging requires that new epoch.
                //
                // The flag itself latches on having EXAMINED a snapshot, not on
                // having sent something: a core that honestly reports no build will not start
                // reporting one later, and latching only inside the `Some` arm would re-clone
                // `server_info` every iteration for the life of the connection — for exactly the
                // absent case this column has to render.
                let predates_restart = publish_gate::server_info_predates_restart(
                    info_addr,
                    version_restart_info.as_ref().map(PinnedRestartInfo::addr),
                );
                if is_ready && !version_sent {
                    if let Some(version) = info.server_version {
                        // Outside the restart pin, preserve the snapshot's letter distinction:
                        // `Some("")` is a release and `None` means no letter was reported.
                        let _ = tx.send(FeedMsg::CoreVersion {
                            version,
                            suffix: publish_gate::letter_to_publish(
                                info.version_suffix.clone(),
                                predates_restart,
                            ),
                        });
                    }
                    version_sent = true;
                    version_restart_info = None;
                }
                if !identity_sent {
                    if let Some(code) = info.exchange_code {
                        // dex_name is nonempty only for Hyperliquid HIP-3 futures. Include it in
                        // the identity so cores from different DEXes are NOT deduplicated onto one
                        // provider with an incomplete market list; see ExchangeId.
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
                            // The core's own caption travels with the identity so no consumer has
                            // to reach back into a client snapshot for it while rendering.
                            reported: info.exchange_name.clone().unwrap_or_default(),
                        });
                        // The account base currency selects the USD conversion used for manual
                        // orders.
                        let base = info.base_currency_name.unwrap_or_default();
                        if !base.is_empty() {
                            let _ = tx.send(FeedMsg::CoreBase { base });
                        }
                        identity_sent = true;
                    }
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
        // `is_ready` is overwritten INSIDE the drain below, so the transition into a settled core
        // is only observable against a value captured before it. Without this the one final
        // startup send would never fire, and because the poll gate is `!is_ready` it could never
        // re-open either — the cell would freeze at its last in-progress figure forever.
        let was_ready = is_ready;
        for ev in lifecycle_events.drain(..) {
            if matches!(
                &ev,
                LifecycleEvent::Connecting
                    | LifecycleEvent::Connected { .. }
                    | LifecycleEvent::Disconnected
            ) {
                trade_sounds.reset();
            }
            // The core is named because these lines are the only record of a connection's shape,
            // and every core in the process writes them into ONE file: without the label, telling
            // which of twenty-two cores never reached `Ready` means correlating by timestamp
            // against unrelated lines that happen to carry a name.
            log::info!(
                "core {} lifecycle: {ev:?}",
                crate::feed::core_label(server.id)
            );
            // The restarted process may run on another exchange, and this client will never read
            // its `ServerInfo` again: only a respawn can publish the venue it has now.
            if let Some(cause) = identity_refresh::stale_on_lifecycle(&ev) {
                let _ = tx.send(FeedMsg::IdentityStale(cause));
            }
            let request_license_state = match &ev {
                LifecycleEvent::Ready => true,
                // Asking a core that is still coming up buys a pending timeout, not a licence, so
                // this reads the same predicate as the status mapping below.
                LifecycleEvent::Connected { fresh } => {
                    reconnect_is_operational(*fresh, init_completed)
                }
                _ => false,
            };
            // Captured before `match ev` moves `ev`. Same predicate as the Ready mapping: an
            // in-loop reconnect is operational only once init has finished.
            let reconnected = matches!(
                &ev,
                LifecycleEvent::Connected { fresh }
                    if reconnect_is_operational(*fresh, init_completed)
            );
            let st = match ev {
                LifecycleEvent::Connecting => ConnStatus::Stage("connecting…".into()),
                LifecycleEvent::Connected { fresh } => {
                    // fresh=true means one-time initialization follows, so wait for Ready.
                    //
                    // fresh=false does NOT mean initialization already ran — that rule and its
                    // reason live on `reconnect_is_operational`. Once it HAS finished, a reconnect
                    // is immediately Ready: moonproto repeats neither init nor `Ready`, but restores
                    // subscriptions and indexes, so waiting for an event that will never arrive
                    // would leave the status stuck at "reconnected" (0/N) forever while data flows.
                    if reconnect_is_operational(fresh, init_completed) {
                        // The first Pings after a reconnect carry its largest delay — the stretch
                        // the estimator's quarantine keeps out.
                        ping_clock.note_ready(now_ms_i64());
                        if std::mem::take(&mut log_delivery_ignored) {
                            let _ = tx.send(FeedMsg::LogDeliveryIgnored(false));
                        }
                        ConnStatus::Ready
                    } else {
                        ConnStatus::Stage("connected, init…".into())
                    }
                }
                // The per-step fact now travels typed on `FeedMsg::StartupStatus`, where the UI can
                // localize it and show it against a denominator. This badge keeps only the coarse
                // phase, so the two can never disagree about what the core is doing — and it stops
                // emitting a raw English step name that no locale could ever translate, since
                // `ConnStatus` lives in this crate and `i18n!` is declared in the UI one.
                LifecycleEvent::InitStepCompleted { .. } => {
                    ConnStatus::Stage("connected, init…".into())
                }
                LifecycleEvent::Ready => {
                    // The one moment initialization is known to have finished. Everything that
                    // treats a later re-handshake as a resumed connection — the status mapping
                    // above, the licence re-request, the startup watchdog — hangs off this latch,
                    // and nothing clears it: a client whose init completed never runs init again,
                    // and a client that is rebuilt gets a fresh `run` with a fresh latch.
                    init_completed = true;
                    ping_clock.note_ready(now_ms_i64());
                    if std::mem::take(&mut log_delivery_ignored) {
                        let _ = tx.send(FeedMsg::LogDeliveryIgnored(false));
                    }
                    ConnStatus::Ready
                }
                LifecycleEvent::Reconnecting => ConnStatus::Stage("reconnecting…".into()),
                LifecycleEvent::ServerRestart => {
                    // A DIFFERENT MoonBot process now answers on this address. MoonProto keeps its
                    // retained settings and strategy state across the event — it clears only news
                    // and session profits — so everything known about the run state belongs to the
                    // process that just went away, and the republish below must not resurrect it.
                    let _ = tx.send(FeedMsg::RunStateForgotten);
                    run_state_seen = RunStateSeen::default();
                    // Queued settings describe the process that went away. This event does not
                    // return from `run`, so `prepare_reconnect` never fires for it — without this
                    // an unsent OK would land on the REPLACEMENT instance.
                    shared_config_sequence.forget_queue();
                    // Same rule, higher stakes: this queue also holds manual ORDERS, priced off a
                    // chart the departed process was feeding.
                    client_settings_sequence.forget_queue(server.id);
                    // moonproto drops its retained Telegram snapshot on a peer-token / ServerToken
                    // change with no event; a restart is the same hole. Nothing left to show muted.
                    let _ = tx.send(FeedMsg::Telegram(None));
                    // Keep this snapshot alive. BaseCheck frees the old `Arc<ServerInfo>`, and the
                    // replacement can be allocated at the same address; holding the snapshot pins
                    // that allocation until its letter-publication decision is made.
                    version_restart_info = client.snapshot().map(PinnedRestartInfo::pin);
                    ConnStatus::Stage("server restart…".into())
                }
                LifecycleEvent::ConnectFailed { error } => {
                    // The typed failure is the whole point of this arm. `ConnStatus::Failed` keeps
                    // only MoonProto's English `Display` text, which is a technical token for the
                    // log and for the honest "could not determine" fallback — the reconnect loop in
                    // `feed::run` overwrites that payload on every retry anyway, so nothing the user
                    // must read may live in it. The fact travels typed instead.
                    let msg = error.to_string();
                    let fault = convert::conn_fault_from_proto(
                        error,
                        client.server_info(),
                        client.startup_status(),
                    );
                    // The panel's live snapshot is republished from the fault's own frozen copy, so
                    // the progress figure beside a reason describes the attempt that reason
                    // explains. The poll below runs on its own cadence and would otherwise leave the
                    // two up to one interval apart.
                    let snap = fault.startup;
                    let _ = tx.send(FeedMsg::ConnFault(fault));
                    startup_sent = Some(snap);
                    let _ = tx.send(FeedMsg::StartupStatus(snap));
                    connect_failed = Some(msg.clone());
                    ConnStatus::Failed(msg)
                }
                LifecycleEvent::BindFailed {
                    consecutive_failures,
                } => {
                    // THIS machine could not bind its own UDP socket. The sentence that used to be
                    // built here named a VPN and a firewall in Russian, inside a crate that cannot
                    // localize; the same advice now lives in `locales/<lang>/core_status.<lang>.yml` behind the
                    // typed kind, and only an English token stays on the status.
                    let _ = tx.send(FeedMsg::ConnFault(convert::bind_fault(
                        consecutive_failures,
                        client.server_info(),
                        client.startup_status(),
                    )));
                    ConnStatus::Failed(format!("udp bind failed x{consecutive_failures}"))
                }
                LifecycleEvent::Disconnected => {
                    let _ = tx.send(FeedMsg::TelegramStale);
                    ConnStatus::Disconnected
                }
            };
            // Tracked for the API-key poll below: an Engine API request sent to a core that is not
            // Ready buys nothing but a pending timeout. Reaching Ready is also the moment to ask —
            // the key may have been replaced while this core was away — subject to the poll's own
            // cooldown, which is what keeps a flapping core from asking on every reconnect.
            let ready = st == ConnStatus::Ready;
            if ready && !is_ready {
                ready_since = Some(Instant::now());
            } else if !ready {
                // Lost, including a moonproto reconnect inside this run: what still waits in the
                // settings sequence would otherwise go out on the next Ready (`stale`).
                if ready_since.take().is_some() {
                    client_settings_sequence.drop_orders_of_lost_connection(server.id);
                }
            }
            is_ready = ready;
            if is_ready {
                account_reconciliation.poll_api_expiry_on_ready(Instant::now());
            }
            // The store drops the reported BUILD on exactly this status, so the latch that stops us
            // republishing it has to be released by exactly this status too. `Reconnecting` and
            // `ServerRestart` are handled INSIDE this loop and never return from `run`, so a local
            // flag that only resets on a fresh `run` would leave the column permanently blank after
            // the first link blip of a session: the store would clear the build, the next
            // `Connected { fresh: false }` would put an already-initialized core back to Ready, and
            // nothing would ever resend it. The two halves must move on the same event or they
            // disagree.
            if !is_ready {
                version_sent = false;
            }
            let _ = tx.send(FeedMsg::Status(st));
            if reconnected {
                // Mark the retained snapshot stale BEFORE asking for a new one, so a pre-outage
                // QR cannot become actionable the instant the badge flips to Ready. The station
                // runs no bot of the core's and asks nothing about it.
                if station.is_none() {
                    let _ = tx.send(FeedMsg::TelegramStale);
                    if let Err(error) = client.telegram().refresh() {
                        log::warn!(
                            "core {} telegram refresh failed: {error}",
                            crate::feed::core_label(server.id)
                        );
                    }
                }
                // A close that happened while the link was down updated a row BELOW the catch-up
                // cursor, so catch-up will not bring it. The library does repeat its retained
                // open-row check after a reconnect, but with the set it was last given, which
                // cannot hold a trade opened since then. Replace it with the replica's open rows
                // as they are now (#742).
                if server.feed.reports {
                    if let Some(sink) = reports {
                        open_rows_check_started = false;
                        sink.send(DbMsg::RecheckOpenRows {
                            core_uid: server.uid,
                            after: None,
                            reports: client.reports(),
                        });
                    }
                }
            }
            // The light station asks the core for none of what a terminal shows — license, run
            // state, settings, hedge mode, balances, chart alerts. The Mini App's asks what a
            // terminal does; chart alerts stay behind `feed.alerts`, which it leaves off.
            if request_license_state && account {
                if let Err(error) = client.settings().request_kernel_license_state() {
                    log::warn!(
                        "core {} request kernel license state failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                // Re-publish the RUN state from MoonProto's retained snapshot.
                //
                // A reconnect (`Connected { fresh: false }`) repeats neither init nor the post-init
                // resync, and the protocol has no request for either half — so nothing would
                // re-report them. The library, however, KEEPS them: `SettingsState` has no reset at
                // all and `StratsState::clear` is only reached when the strategy list itself is
                // rebuilt, never from the reconnect path. Reading the snapshot here is therefore
                // the whole recovery: the values return CONFIRMED rather than as a retained guess.
                // Empty only when this is a replacement client, which is exactly when a guess would
                // have been wrong anyway.
                if let Some(state) = client.snapshot() {
                    // Only the halves THIS MoonBot instance has already reported: the library keeps
                    // its retained state across a server restart, so anything it holds that the
                    // current instance never sent describes the previous one.
                    if run_state_seen.runtime {
                        if let Some(runtime) = state.settings().runtime_state.as_ref() {
                            let _ = tx.send(FeedMsg::RuntimeState(
                                convert::runtime_state_from_proto(runtime),
                            ));
                        }
                    }
                    if run_state_seen.trading {
                        if let Some(running) = state.strats().strategies_running() {
                            let _ = tx.send(FeedMsg::StrategiesRunning(running));
                        }
                    }
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
            return Err(run_failed(init_completed, anyhow::anyhow!("{e}")));
        }

        // Drain domain events from MoonEventSink. Read ticks/order books/orders from the snapshot
        // only after an actual event instead of polling continuously every 8 ms.
        events.clear();
        event_queue.drain_events_into(&mut events);
        // Read before the station filter, which has no reason to keep market-list events: a hot
        // exchange switch shows only as a refresh that adds most of a new venue's universe.
        let mut list_applied = false;
        let mut catalog_changed = false;
        for ev in &events {
            let Event::Markets(markets_ev) = ev else {
                continue;
            };
            list_applied |= matches!(
                markets_ev,
                moonproto::state::MarketsEvent::MarketsListReplaced { .. }
            );
            catalog_changed |= matches!(
                markets_ev,
                moonproto::state::MarketsEvent::MarketsListReplaced { .. }
                    | moonproto::state::MarketsEvent::NewMarketsAdded { .. }
            );
            if !turnover_armed {
                continue;
            }
            let total = || {
                client
                    .snapshot()
                    .map_or(0, |snap| snap.markets().iter().count())
            };
            if let Some(cause) = identity_refresh::stale_on_markets(markets_ev, total) {
                let _ = tx.send(FeedMsg::IdentityStale(cause));
            }
        }
        turnover_armed |= list_applied;
        if catalog_changed {
            client_slot.note_catalog_change();
        }
        if let Some(profile) = station {
            events.retain(|event| profile.keeps(event));
        }
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
        // A station plays no sound: the Mini App station takes orders, and nobody would take the
        // sounds they ring.
        if server.feed.orders && station.is_none() {
            let sound_snapshot = client.snapshot();
            let sounds = trade_sounds.observe(
                &events,
                is_ready,
                sound_snapshot.as_ref().map(|snapshot| snapshot.orders()),
            );
            if !sounds.is_empty() && tx.send(FeedMsg::TradeSounds(sounds)).is_err() {
                break;
            }
        }
        // The light station never repairs an account: it shows the repairs no event, so none is
        // ever queued and no repair deadline exists to hold the wait below at zero.
        if account {
            account_reconciliation.observe_events(&events, account_now);
        }
        if account_reconciliation.balance_due(account_now) {
            match client.balances().refresh() {
                Ok(()) => {
                    balance_refresh_log_until = Some(Instant::now() + balance_refresh_log_window());
                    log::log!(
                        BALANCE_TRACE_LEVEL,
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
        // Startup progress: polled while the core is still coming up, and stopped only once the
        // SNAPSHOT ITSELF reports a terminal phase. A settled core then costs nothing at all here —
        // no lock read, no message, no revision bump.
        //
        // The stop condition deliberately reads the snapshot rather than the lifecycle events,
        // because MoonProto publishes the two in the opposite order on its two paths. First startup
        // writes the status and THEN fires `Ready` (`runtime_loop`'s `mark_ready` sits immediately
        // before that `fire_lifecycle`), so there the event implies the write. An internal reconnect
        // does the reverse: `Connected{fresh:false}` is fired from inside `run_protocol_step`, while
        // the `publish` that flips the status to `Ready` runs later in the same iteration. This is a
        // different thread, woken BY that event, so it can read the pre-reconnect `Reconnecting`
        // snapshot — and stopping on `is_ready` would close the poll forever and freeze the cell
        // showing progress on a core that is actually up.
        //
        // BOTH halves are required. The snapshot alone would latch: once a core reports `Ready` that
        // phase is terminal forever, so a later reconnect episode — which MoonProto does re-publish,
        // and which its own `reconnect_count` exists to report — would never be polled for again.
        // The lifecycle side supplies the RESUME edge, the snapshot side supplies the honest STOP.
        let settled = startup_poll_settled(is_ready, startup_sent);
        let startup_edge = !was_ready && is_ready;
        if !settled && (startup_edge || last_startup.elapsed() >= STARTUP_POLL) {
            // Named rather than reused from `last_startup`: it is the instant the watchdog's clock
            // is measured against, and a later edit moving that assignment must not silently
            // redefine it.
            let polled_at = Instant::now();
            last_startup = polled_at;
            let snap = convert::startup_status_from_proto(client.startup_status());
            if startup_sent.is_none_or(|prev| !prev.progress_eq(&snap)) {
                startup_sent = Some(snap);
                let _ = tx.send(FeedMsg::StartupStatus(snap));
            }
            // Give up on a FIRST startup that has stopped advancing. MoonProto's init spine has no
            // terminal failure of its own — a required step that times out is re-sent forever, and
            // the phases that wait for authorization park indefinitely — so nothing else in this
            // process would ever rebuild the client, and the core would stay half-initialized for
            // the session. Failing the run hands it to the app-level reconnect in `feed::spawn`.
            //
            // `init_completed` is the SINGLE owner of "initialization has finished", and the
            // watchdog checks it itself. The snapshot cannot serve as a second guard: after startup
            // MoonProto publishes `Ready`/`Reconnecting` in step with authorization rather than
            // freezing, so a post-init blip looks exactly like a stalled one from there.
            if startup_watchdog.observe(&snap, polled_at, init_completed) {
                // The fault carries the reason across the crate boundary as FACTS, so the panel
                // words this attempt through the existing "stalled at this step" verdict. Without
                // it the store would keep the PREVIOUS attempt's fault — cleared only on Ready —
                // and explain the stall with an unrelated earlier cause. The reconnect loop logs
                // the error itself, beside the retry it is about to make.
                let _ = tx.send(FeedMsg::ConnFault(convert::stall_fault(
                    client.server_info(),
                    snap,
                )));
                return Err(run_failed(init_completed, StartupStalled::of(&snap)));
            }
        }
        // API-key expiration: a pure poll, since no event announces that a key aged a day. Gated on
        // Ready — a request to a core that is not connected only buys a pending timeout — and the
        // attempt is marked whether or not the request left, so a core stuck mid-connect cannot ask
        // on every wake-up.
        if account_reconciliation.api_expiry_due(account_now) {
            if station.is_some() {
                // Never asked; pushed out a full interval so the due deadline cannot hold the
                // wait below at zero.
                account_reconciliation.defer_api_expiry(account_now);
            } else if is_ready {
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
        let mut chart_texts: Vec<ChartTextRows> = Vec::new();
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
                Event::ChartText(snapshot) => {
                    chart_texts.push(ChartTextRows {
                        market: snapshot.market_name.clone(),
                        filter_lines: snapshot.filter_lines.clone(),
                    });
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
                    // A hedge-mode switch reports its outcome HERE and nowhere else: the account
                    // snapshot is not republished by the write, and `HedgeModeUpdated` — the one
                    // event that feeds `CoreData::hedge_mode` — is only ever answered to a
                    // `refresh_hedge_mode` request. Without this re-read the toolbar checkbox kept
                    // the pre-switch value until the next connect, while the toast already said
                    // the mode was on. Ask the core rather than trusting the echoed flag, so the
                    // stored value stays something the core confirmed.
                    if e.success
                        && matches!(e.kind, moonproto::EngineActionKind::SetHedgeMode { .. })
                    {
                        if let Err(error) = client.account().refresh_hedge_mode() {
                            log::warn!(
                                "core {} re-read hedge mode after switch failed: {error}",
                                crate::feed::core_label(server.id)
                            );
                        }
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
                    let _ = tx.send(FeedMsg::ChartArchiveAnswered {
                        market: ticket.market.clone(),
                        epoch: client_epoch,
                    });
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
                    let _ = tx.send(FeedMsg::ChartArchiveAnswered {
                        market: ticket.market.clone(),
                        epoch: client_epoch,
                    });
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
                // continuously — and see BALANCE_TRACE_LEVEL for why the window alone was not
                // enough to keep this quiet.
                Event::Balance(bev) => {
                    if balance_refresh_log_until.is_some_and(|t| Instant::now() < t) {
                        log::log!(
                            BALANCE_TRACE_LEVEL,
                            "core {} balance event after refresh: {bev:?}",
                            crate::feed::core_label(server.id)
                        );
                    }
                }
                // News/tags feed is consumed below via `news_snapshot_from_proto` reading the
                // retained `client.snapshot().news()`, matching the license/KernelHealth idiom.
                Event::News(_) => {}
                // Strategy-edit lifecycle. The retained latch and note accumulator below are what
                // make the ~250 ms publish block (further down) safe against the 1 Hz strategies
                // gate's cadence; see `strategy_edit_sig` and `record_strategy_edit_resolution`.
                Event::Strat(strat_ev) => match strat_ev {
                    StratEvent::EditSubmitted { strategy_ids } => {
                        strat_edit_publish_pending = true;
                        if let Some(snap) = client.snapshot() {
                            let strats = snap.strats();
                            for &id in strategy_ids {
                                if let Some(edit) = strats.strategy_edit(id) {
                                    strat_edit_desired_cache.insert(id, edit.desired().clone());
                                }
                            }
                        }
                    }
                    StratEvent::EditTimedOut { .. } => {
                        strat_edit_publish_pending = true;
                    }
                    StratEvent::EditConfirmed { strategy_ids } => {
                        strat_edit_publish_pending = true;
                        let echo_snap = client.snapshot();
                        record_strategy_edit_resolution(
                            echo_snap.as_deref(),
                            &mut strat_edit_desired_cache,
                            strategy_ids,
                            StrategyEditResult::Confirmed,
                            server.id,
                            &mut pending_strat_edit_notes,
                        );
                    }
                    StratEvent::EditAdjusted { strategy_ids } => {
                        strat_edit_publish_pending = true;
                        let echo_snap = client.snapshot();
                        record_strategy_edit_resolution(
                            echo_snap.as_deref(),
                            &mut strat_edit_desired_cache,
                            strategy_ids,
                            StrategyEditResult::Adjusted,
                            server.id,
                            &mut pending_strat_edit_notes,
                        );
                    }
                    StratEvent::EditSuperseded { strategy_ids } => {
                        strat_edit_publish_pending = true;
                        let echo_snap = client.snapshot();
                        record_strategy_edit_resolution(
                            echo_snap.as_deref(),
                            &mut strat_edit_desired_cache,
                            strategy_ids,
                            StrategyEditResult::Superseded,
                            server.id,
                            &mut pending_strat_edit_notes,
                        );
                    }
                    _ => {}
                },
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
        if !chart_texts.is_empty() && tx.send(FeedMsg::ChartText(chart_texts)).is_err() {
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
        let client_settings_arrived = client_settings.is_some();
        if let Some(settings) = client_settings {
            client_settings_sequence.observe_update();
            client_settings_sequence.drive(&client, server.id);
            if tx.send(FeedMsg::ClientSettings(settings)).is_err() {
                break;
            }
        }
        // TempBL travels with the settings above but on its own terms — it is deliberately not a
        // field of the projection they carry (`feed::TempBlacklistRow` says why) — and only what
        // survives subtracting the expected countdown reaches the UI. See `temp_blacklist`.
        if let Some(rows) = temp_blacklist.poll(&client, server.id, client_settings_arrived) {
            if tx.send(FeedMsg::TempBlacklist(rows)).is_err() {
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
            run_state_seen.runtime = true;
            if tx.send(FeedMsg::RuntimeState(state)).is_err() {
                break;
            }
        }
        // Full safe-share configuration. The runtime requests it in the background after Ready
        // and retries until it arrives, so reading it here adds no request of its own; it carries
        // the settings pages the compact snapshot above has no room for.
        //
        // Built through `build_shared_config`, NOT read from the retained full snapshot: that
        // builder overlays whatever compact settings arrived after it, and it is what every write
        // and echo comparison uses. Projecting the raw snapshot instead would show — and then let
        // OK write back — values already superseded by a coin blacklisted from the context menu or
        // a take profit changed on the toolbar.
        //
        // Hence the compact events in the predicate too: they change the overlay, so they change
        // this projection even when no new full snapshot arrived.
        let raw_shared = events
            .iter()
            .any(|ev| {
                matches!(
                    ev,
                    &Event::Settings(
                        SettingsEvent::SharedConfigUpdated
                            | SettingsEvent::ClientSettingsUpdated
                            | SettingsEvent::LevManageUpdated
                    )
                )
            })
            .then(|| client.settings().build_shared_config().ok())
            .flatten();
        // Kept beside the projection for `channels.settings` alone: the probe reads fields the
        // projection does not carry, and asking the client twice would take the snapshot twice.
        if let Some(raw) = raw_shared.as_ref() {
            shared_config_sequence.note_settings_text(server.id, raw);
        }
        let core_config = raw_shared.map(|cfg| shared_config::core_config_from_proto(&cfg));
        if let Some(config) = core_config {
            // What the core actually holds in its marked-markets list, for `channels.settings`;
            // see `SharedConfigSequence::note_fav_markets`.
            shared_config_sequence.note_fav_markets(server.id, &config);
            // Only a FULL snapshot lifts the write barrier. The core re-broadcasts both snapshots
            // after applying a write, and the compact one can arrive in an earlier batch: treating
            // it as the echo would replan the whole config against a still-unconfirmed base,
            // provoke another compact echo, and burn the attempt budget on a write already in
            // flight. The other two events only mean the OVERLAY changed, which is why they
            // republish the projection above.
            if events
                .iter()
                .any(|ev| matches!(ev, &Event::Settings(SettingsEvent::SharedConfigUpdated)))
            {
                shared_config_sequence.observe_update();
            }
            if client_settings_sequence.is_idle() {
                shared_config_sequence.drive(&client, server.id, &mut core_config_events);
            } else {
                shared_config_sequence.note_gated(server.id);
            }
            // Provenance travels with the message: only a real `SharedConfigUpdated` full-snapshot
            // echo may advance `core_config_recv_rev` in the store, for the same reason the write
            // barrier above does not lift on a compact-overlay republication.
            let from_full_snapshot = events
                .iter()
                .any(|ev| matches!(ev, &Event::Settings(SettingsEvent::SharedConfigUpdated)));
            if tx
                .send(FeedMsg::CoreConfig {
                    config,
                    from_full_snapshot,
                })
                .is_err()
            {
                break;
            }
        }
        // After the snapshot, never before it — and once per iteration, whichever drive produced
        // them: a `Drained` among these tells a surface that the retained page reflects everything
        // it sent, and that page must already be in the store when the surface reads it. The drive
        // in the command drain compares against the client's snapshot as it stands NOW, which is
        // the one published just above when a settings event arrived in this same iteration.
        if !send_core_config_events(core_config_events) {
            break;
        }
        let profit_state = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::ProfitStateUpdated)),
            |state| {
                state
                    .settings()
                    .profit_state
                    .as_ref()
                    .map(profit_state_from_proto)
            },
        );
        if let Some(profit) = profit_state {
            if tx.send(FeedMsg::ProfitState(profit)).is_err() {
                break;
            }
        }
        // The core's own confirmed diagnostics. Read from the RETAINED snapshot rather than from
        // the event payload, and on BOTH of its events: `ProblemsUpdated` carries a replacement
        // list, while `ProblemConfirmed` carries one row that moonproto has ALREADY folded into
        // that same retained state. Rebuilding from the snapshot on either one means the two paths
        // cannot disagree, and it is what makes removals work at all — a row that vanished from a
        // full list has no event of its own.
        //
        // Nothing is ever requested here: the core sends its list unprompted on connection, and a
        // core too old for the extension simply never sends one. That is the whole compatibility
        // story — see `CoreProblems::supported`.
        //
        // An operator-requested re-read is a SECOND reason to read the same retained state, so it
        // joins the event gate rather than branching beside it. There is no event to wait for: the
        // wire cannot be asked for a list, and moonproto empties this state without one when a
        // ServerToken changes — see `CoreCmd::RefreshProblems`. Either reason reads the same
        // snapshot through the same projection, so one gate is all the difference between them.
        let want_problems = problems_relist
            || events.iter().any(|ev| {
                matches!(
                    ev,
                    &Event::Settings(
                        SettingsEvent::ProblemsUpdated | SettingsEvent::ProblemConfirmed { .. }
                    )
                )
            });
        // Wrapped in `Some` because an EMPTY list is a real answer — the core looked and found
        // nothing — and the helper would otherwise drop it as "nothing to report", which is the one
        // reading this feature must never produce.
        let problems = convert::snapshot_when(want_problems, &client, |state| {
            Some(convert::problems_from_proto(&state.settings().problems))
        });
        if let Some(problems) = problems {
            if tx.send(FeedMsg::Problems(problems)).is_err() {
                break;
            }
        }
        // The core's Telegram reader snapshot. Read from the RETAINED snapshot on
        // `TelegramUpdated`, wrapping the inner `Option` in `Some` because `None` is a real
        // answer — the library holds no snapshot — and the helper would otherwise drop it.
        // No republish flag: a user Refresh is `client.telegram().refresh()` only.
        let telegram = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Settings(SettingsEvent::TelegramUpdated)),
            |state| {
                Some(
                    state
                        .settings()
                        .telegram
                        .as_ref()
                        .map(|t| Arc::new(telegram_from_proto(t))),
                )
            },
        );
        if let Some(telegram) = telegram {
            if tx.send(FeedMsg::Telegram(telegram)).is_err() {
                break;
            }
        }
        // Remaining exchange API request quota (HyperLiquid `THLRequestLimitStateCommand`). Read
        // from the RETAINED snapshot on its own event, like every settings value above: the core
        // republishes it every few minutes, so an event-driven read costs nothing between them.
        let api_quota = settings_event_snapshot(
            &events,
            &client,
            |ev| {
                matches!(
                    ev,
                    &Event::Settings(SettingsEvent::HyperliquidRequestLimitUpdated)
                )
            },
            // Wrapped in `Some` on purpose: the inner `None` is a real answer — the protocol cannot
            // represent a quota it failed to decode — and the helper would drop it as "nothing".
            |state| Some(state.settings().hyperliquid_requests_left),
        );
        if let Some(left) = api_quota {
            if tx.send(FeedMsg::ApiQuota(left)).is_err() {
                break;
            }
        }
        // Diagnostics (`channels.hl_limit`): the same number, written to its own file with the core
        // and the exchange beside it. Unlike the publication above this reports the value the
        // snapshot HOLDS, so switching the channel on mid-session answers immediately instead of
        // waiting for the next command. Gated on the switch FIRST, so the off case costs one atomic
        // load rather than a snapshot per batch.
        if crate::hl_diag::enabled() {
            if let Some(left) = client
                .snapshot()
                .map(|s| s.settings().hyperliquid_requests_left)
            {
                if api_quota.is_some() || hl_last_logged != Some(left) {
                    hl_last_logged = Some(left);
                    let exchange = client
                        .server_info()
                        .and_then(|i| i.exchange_code)
                        .map(|c| c.stable_id().to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let value = match left {
                        Some(v) => v.to_string(),
                        None => "none".to_string(),
                    };
                    crate::hl_diag::line(&format!(
                        "core={} exchange={exchange} requests_left={value} on_command={}",
                        crate::feed::core_label(server.id),
                        api_quota.is_some()
                    ));
                }
            }
        }
        // The global strategy engine's run state (`TStratRuntimeState`), which is NOT part of the
        // runtime state above: the core sends the two over different commands and at different
        // moments. Read from the RETAINED snapshot on its own event, like every settings value
        // here — the event carries the same flag, but the snapshot is the authority the terminal
        // renders from, so nothing can drift if the core repeats or reorders them.
        let strategies_running = settings_event_snapshot(
            &events,
            &client,
            |ev| matches!(ev, &Event::Strat(StratEvent::RuntimeState { .. })),
            |state| state.strats().strategies_running(),
        );
        if let Some(running) = strategies_running {
            run_state_seen.trading = true;
            if tx.send(FeedMsg::StrategiesRunning(running)).is_err() {
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
        // alerts=true. Controlled by `channels.detect`, off by default.
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
        // `want_log` gates only UI/disk PUBLICATION further below. A rejected
        // `request_version_update` is fire-and-forget, so `ServerLog` is its only reply channel,
        // and `feed.log = false` must not be able to hide it from the update state machine: read
        // it here, ahead of and independent from the publication-gated block.
        for ev in &events {
            if let Event::ServerLog(l) = ev {
                // Asked not to send its log, the core still does, well after the lines a core
                // flushes around Ready: it does not understand the request.
                if log_delivery_off
                    && !log_delivery_ignored
                    && ready_since.is_some_and(|t| t.elapsed() >= crate::feed::LOG_DELIVERY_GRACE)
                {
                    log_delivery_ignored = true;
                    log::info!(
                        "[{}] the core keeps sending its log after it was asked to stop",
                        server.name
                    );
                    let _ = tx.send(FeedMsg::LogDeliveryIgnored(true));
                }
                if crate::feed::is_core_update_rejection(&l.msg) {
                    log::warn!(
                        "[{}] core update refused: {}",
                        server.name,
                        crate::applog::redact::addresses(&l.msg)
                    );
                    let _ = tx.send(FeedMsg::CoreUpdateRejected);
                }
            }
        }
        // Alert fires (`DETECT_KIND_ALERT`) arrive as Event::Detect. Also enter this path when
        // feed.alerts is enabled so alerts work without the general detect stream.
        let want_detects = server.feed.detects || server.feed.alerts;
        // A detect may be one its strategy reports to Telegram whatever this feed's flags say —
        // on a station, where `detects` is off, too.
        let has_detect = events.iter().any(|ev| matches!(ev, Event::Detect(_)));
        if want_detects || has_detect || (server.feed.reports && reports.is_some()) || want_log {
            let mut detects: Vec<DetectRow> = Vec::new();
            let mut logs: Vec<CoreLogLine> = Vec::new();
            let mut tg_events: Vec<CoreTgEvent> = Vec::new();
            // Snapshot for fields of the strategy that produced the detect (SoundAlert/KeepAlert/sound).
            let detect_snap = (want_detects || has_detect)
                .then(|| client.snapshot())
                .flatten();
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
                        tg_events.extend(tg_detect(detect_snap.as_deref(), d));
                        let strat = detect_snap
                            .as_ref()
                            .and_then(|s| s.strats().snapshot(d.strategy_id));
                        let params = strat
                            .map(|st| alert_params(st, detect_schema))
                            .unwrap_or_default();
                        // Empty ONLY when no strategy produced this — an alert firing. An unnamed
                        // strategy still names itself by id, so neither the card nor the chart
                        // caption has to guess which of the two it is looking at.
                        let strat_name = detect_strat_name(strat);
                        if crate::detect_diag::enabled() {
                            crate::detect_diag::line(&format!(
                                "[feed] detect market={} strat_id={} strat_found={} strat_name={strat_name:?} sound_alert={} sound={:?} is_alert={}",
                                d.market_name,
                                d.strategy_id,
                                strat.is_some(),
                                params.sound_alert,
                                params.sound_name,
                                d.is_alert_fire(),
                            ));
                        }
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
                            // Core text, and the same redaction the server log gets: a detect line
                            // can name the machine the core runs on, and this one is drawn on a
                            // chart that gets screenshotted.
                            msg: crate::applog::redact::addresses(&d.msg)
                                .chars()
                                .take(crate::feed::DETECT_MSG_KEEP)
                                .collect(),
                            strat_name,
                        });
                    }
                    // Not wanted for the detects feed, but still one its strategy may report.
                    Event::Detect(d) => {
                        tg_events.extend(tg_detect(detect_snap.as_deref(), d));
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
                    // The archived order traces of one closed trade, asked for by a trade window.
                    // Ahead of the replica arm and outside its `feed.reports` gate: the answer is
                    // the window's, not the writer's, and a core whose report feed is off can
                    // still be asked about a trade the terminal read from an older replica.
                    Event::Report(ReportEvent::TraceReady { ticket, traces }) => {
                        log::info!(
                            "core {} report traces uid={}: {} archived line(s)",
                            crate::feed::core_label(server.id),
                            ticket.report_uid,
                            traces.len()
                        );
                        trace_pacer.answered(ticket.report_uid, true);
                        let lines: Arc<[crate::feed::ArchivedOrderTrace]> = Arc::from(
                            crate::feed::report_traces::archived_traces_from_proto(traces),
                        );
                        // Filed locally first — empty included, so the row is not asked about
                        // again on every start — then handed to whoever asked.
                        if let Some(sink) = &trace_sink {
                            sink.send(TraceDbMsg::Answer {
                                core_uid: server.uid,
                                report_uid: ticket.report_uid,
                                lines: lines.clone(),
                            });
                        }
                        let _ = tx.send(FeedMsg::ReportTraces {
                            report_uid: ticket.report_uid,
                            outcome: crate::feed::ReportTracesOutcome::Ready(lines),
                        });
                    }
                    Event::Report(ReportEvent::TraceFailed { ticket, error }) => {
                        log::warn!(
                            "core {} report traces for uid={} failed: {error}",
                            crate::feed::core_label(server.id),
                            ticket.report_uid
                        );
                        trace_pacer.answered(ticket.report_uid, false);
                        let _ = tx.send(FeedMsg::ReportTraces {
                            report_uid: ticket.report_uid,
                            outcome: crate::feed::ReportTracesOutcome::Failed(error.clone()),
                        });
                    }
                    // Typed report-database replica: send schema, rows, catch-up state, and
                    // reconciliation results to the SQLite writer, the sole write-connection owner.
                    Event::Report(rev) if server.feed.reports => {
                        // The trace store's three inputs, read off the same events the replica
                        // writer gets and independent of whether that writer is running: a closed
                        // row's traces are worth keeping even when the replica file is not.
                        match rev {
                            ReportEvent::Schema(schema) => {
                                trace_fields = trace_field_indices(schema);
                                // A revision keeps the rows: what was remembered of them stays.
                                capture = match (
                                    capture.take(),
                                    capture::CaptureFields::from_schema(schema),
                                ) {
                                    (Some(mut tracker), Some(fields)) => {
                                        tracker.refield(fields);
                                        Some(tracker)
                                    }
                                    (None, Some(fields)) => {
                                        Some(capture::CaptureTracker::new(fields))
                                    }
                                    (_, None) => None,
                                };
                            }
                            ReportEvent::RowUpsert(row) => {
                                if let (Some(fields), Some(sink)) = (trace_fields, &trace_sink) {
                                    if let Some(report_uid) = closed_row_uid(row, fields) {
                                        sink.send(TraceDbMsg::RowClosed {
                                            core_uid: server.uid,
                                            report_uid,
                                            ask: trace_ask_sink.clone(),
                                        });
                                    }
                                }
                                match capture.as_mut().and_then(|tracker| tracker.on_row(row)) {
                                    Some(capture::RowEdge::Opened {
                                        rec_id,
                                        coin,
                                        buy,
                                        strategy,
                                        emulator,
                                    }) => {
                                        tg_events.extend(tg_opened(
                                            client.snapshot().as_deref(),
                                            strategy,
                                            emulator,
                                            rec_id,
                                            &coin,
                                            buy,
                                        ));
                                        let _ = tx.send(FeedMsg::TradeOpened {
                                            rec_id,
                                            coin,
                                            quote: server.market.clone(),
                                            buy,
                                        });
                                    }
                                    Some(capture::RowEdge::Closed(rec_id, closed)) => {
                                        let _ = tx.send(FeedMsg::TradeClosed {
                                            rec_id,
                                            coin: closed.coin,
                                            quote: server.market.clone(),
                                            buy: closed.buy,
                                            close: closed.close,
                                        });
                                    }
                                    None => {}
                                }
                            }
                            // A page row carries the core's whole column set: the open rows on
                            // it are what a later partial close completes itself from, and its
                            // waiting buys what a later partial fill is announced from.
                            ReportEvent::SyncPage(page) => {
                                if let Some(tracker) = capture.as_mut() {
                                    for row in page.rows.iter() {
                                        tracker.on_page_row(row);
                                    }
                                }
                            }
                            ReportEvent::SyncComplete(_) => {
                                if let Some(sink) = &trace_sink {
                                    sink.send(TraceDbMsg::Backfill {
                                        core_uid: server.uid,
                                        ask: trace_ask_sink.clone(),
                                    });
                                }
                            }
                            _ => {}
                        }
                        if reports.is_some() {
                            if let Some(progress) =
                                report_sync.observe(rev, crate::util::time::now_unix_ms_i64())
                            {
                                let _ = tx.send(FeedMsg::ReportSync(progress));
                            }
                        }
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
                                ReportEvent::OpenRowsCheckStarted { rec_ids } => {
                                    open_rows_check_started = true;
                                    log::info!(
                                        "отчёты: core={} «{}» проверка открытых строк начата                                          ({} шт)",
                                        server.uid,
                                        server.name,
                                        rec_ids.len(),
                                    );
                                }
                                ReportEvent::OpenRowsCheckComplete { rec_ids } => {
                                    log::info!(
                                        "отчёты: core={} «{}» проверка открытых строк завершена \
                                         ({} шт)",
                                        server.uid,
                                        server.name,
                                        rec_ids.len(),
                                    );
                                    // A full page may have more open rows below it. Each step
                                    // goes strictly below the page it follows, so the walk ends;
                                    // the writer drops the step if a newer walk replaced it.
                                    if open_rows_check_started
                                        && rec_ids.len() >= crate::db::OPEN_ROWS_PAGE
                                    {
                                        open_rows_check_started = false;
                                        sink.send(DbMsg::RecheckOpenRows {
                                            core_uid: server.uid,
                                            after: Some(rec_ids.clone()),
                                            reports: client.reports(),
                                        });
                                    }
                                }
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
                                // Matched by the arms above this one, before the replica gate.
                                ReportEvent::TraceReady { .. }
                                | ReportEvent::TraceFailed { .. } => {}
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
            if !tg_events.is_empty() && tx.send(FeedMsg::TelegramEvents(tg_events)).is_err() {
                break;
            }
        }

        // A snapshot is cheap (an Arc clone), but read it only after an actual domain event. Throttle
        // the UI order table to about 4 Hz while updating the chart/order-line store immediately on
        // OrderEvent. Otherwise a short terminal status (Cancel/Fail with deferred-removal=0) can be
        // missed between two table ticks.
        let orders_now = Instant::now();
        if server.feed.orders && has_orders_table_event {
            orders_table.queue(orders_now);
        }
        if server.feed.orders && (had_domain_event || orders_table.is_queued()) {
            let publish =
                OrdersPublish::decide(orders_table.is_due(orders_now), has_order_line_event);
            // Read the snapshot only for a publication that is actually owed, as above.
            if let Some(publish) = publish {
                match client.snapshot() {
                    Some(snap) => {
                        snapshotless_orders = false;
                        let rows = build_order_rows(server.id, &snap, &events);
                        if publish == OrdersPublish::Table {
                            // Not `orders_now`: that is when the turn DECIDED, and the cooldown
                            // has to run from when the table actually went out, after the rows
                            // were built. Stamping the earlier moment shortens every period by
                            // the cost of building them.
                            orders_table.mark_attempt(Instant::now());
                        }
                        if tx.send(publish.message(rows)).is_err() {
                            break;
                        }
                    }
                    // No state to build rows from — reachable only once MoonProto's runtime is
                    // gone for good, since it publishes the snapshot before the events that carry
                    // it and clears it only at teardown. Both halves here are the TABLE's: it is
                    // the only publication that carries a deadline and the only one that still owes
                    // a send after being declined. `defer` keeps that debt while stopping `wait`
                    // from answering zero every pass — the shape this replaced spun the thread at
                    // 100%, and an unlogged version of it would publish nothing just as quietly,
                    // which is what the once-per-episode warning is for.
                    None if publish == OrdersPublish::Table => {
                        orders_table.defer(orders_now);
                        if !snapshotless_orders {
                            snapshotless_orders = true;
                            log::warn!(
                                "core {} has no state snapshot while orders are due; the table \
                                 waits for one",
                                crate::feed::core_label(server.id)
                            );
                        }
                    }
                    None => {}
                }
            }
        }

        // Strategy-edit carrier: publish `open`/`resolved` on their own ~250 ms cadence, well
        // under the 1 Hz gate below, so a button press gets pending feedback before a user
        // concludes it did nothing. Runs every tick, not only ones with a fresh domain event: the
        // retained latch is what decides whether there is anything to send, so a publish delayed
        // by the rate limit still goes out on the very next tick instead of waiting for another
        // edit event that may never come.
        if server.feed.strategies {
            if let Some(snap) = client.snapshot() {
                let strats = snap.strats();
                let edit_sig = strategy_edit_sig(strats.strategy_edits());
                if edit_sig != last_strat_edit_sig {
                    strat_edit_publish_pending = true;
                }
                if strat_edit_publish_pending
                    && last_strat_edit_pub.elapsed() >= Duration::from_millis(250)
                {
                    let open: Vec<StrategyEditRow> = strats
                        .strategy_edits()
                        .map(|(id, edit)| StrategyEditRow {
                            id,
                            phase: match edit.status() {
                                StrategyEditStatus::Pending => StrategyEditPhase::Pending,
                                StrategyEditStatus::TimedOut => StrategyEditPhase::TimedOut,
                            },
                            submitted_at_ms: edit.submitted_at().unix_millis(),
                            fields: edit
                                .desired()
                                .fields
                                .iter()
                                .map(|(n, v)| (n.to_string(), fmt_field(v)))
                                .collect(),
                        })
                        .collect();
                    last_strat_edit_sig = edit_sig;
                    last_strat_edit_pub = Instant::now();
                    strat_edit_publish_pending = false;
                    let resolved = std::mem::take(&mut pending_strat_edit_notes);
                    let had_resolution = !resolved.is_empty();
                    if tx
                        .send(FeedMsg::StrategyEdits(StrategyEditSnapshot {
                            open,
                            resolved,
                        }))
                        .is_err()
                    {
                        break;
                    }
                    // A terminal resolution changes both the confirmed value and the pending
                    // marker; force the StrategyRow rebuild into THIS drain so the two never
                    // appear one publish apart. Do not hold the edit publish back instead — that
                    // reintroduces the exact drop this block exists to prevent.
                    if had_resolution {
                        last_strats = Instant::now()
                            .checked_sub(Duration::from_secs(1))
                            .unwrap_or(last_strats);
                    }
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
                let sig = convert::strategies_publish_sig(
                    strats
                        .snapshots()
                        .map(|s| (s.strategy_id, s.strategy_ver, s.last_date, s.checked)),
                );
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
                let strategies_changed = sig != last_strat_sig;
                if strategies_changed {
                    last_strat_sig = sig;
                    // The order table's Strat column resolves strat_id to kind through this same
                    // registry in `build_order_row`. The registry is populated AFTER orders, while
                    // the table normally rebuilds only on order events, so raw strat_id values are
                    // visible until then. A strategy-set change must resolve the names again:
                    // queue the table so it rebuilds within its own period even without a new order
                    // event, and `order_wait` below wakes the loop on that deadline.
                    if server.feed.orders {
                        orders_table.queue(Instant::now());
                    }
                    let strategies: Vec<StrategyRow> = strats
                        .snapshots()
                        .map(|s| {
                            let name = strat_display_name(s);
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

                // The core's folder tree, empty folders included. Read on its own cursor rather
                // than inside the block above: a folder created or deleted with no strategy in it
                // moves nothing the strategy signature covers, and that folder is exactly the one
                // this list exists to carry.
                let folders_version = strats.folders_last_modified();
                if folders_version != last_folders_version || strategies_changed {
                    last_folders_version = folders_version;
                    let folders = convert::folders_from_proto(strats);
                    // Digested rather than compared: the tree is republished on every strategy
                    // change, and holding a second copy of every path per core to answer "did it
                    // move" costs more than the fold does.
                    let digest = folders.paths.iter().fold(
                        u64::from(folders.supported) | (u64::from(folders.editable) << 1),
                        |acc, path| {
                            let mut hasher = std::collections::hash_map::DefaultHasher::new();
                            std::hash::Hash::hash(path, &mut hasher);
                            acc.wrapping_mul(1099511628211)
                                .wrapping_add(std::hash::Hasher::finish(&hasher))
                        },
                    );
                    if digest != last_folders_digest {
                        last_folders_digest = digest;
                        if tx.send(FeedMsg::Folders(folders)).is_err() {
                            break;
                        }
                    }
                }
                // The database cursor is separate from the UI cursor: schema defaults can arrive
                // after an unchanged strategy set, and a full writer queue must leave the set due
                // for retry. Dumps are incomplete until defaults are available. The strategy
                // version archive is the terminal's: a station keeps no `strategies.sqlite`, even
                // the one that takes strategies for the Mini App.
                if station.is_none()
                    && pending_strat_db_delivery.is_none()
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
        if account && should_publish_assets(&events, last_assets.elapsed(), assets_every) {
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
        if let Some(snap) = client.snapshot().filter(|_| station.is_none()) {
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
        if !dirty_markets.is_empty() && tx.send(FeedMsg::MarketDataChanged(dirty_markets)).is_err()
        {
            let _ = client.disconnect();
            return Ok(());
        }
        force_market_sample = false;

        // The clock offset, on its own deadline: nothing announces a new Ping, so this runs on
        // every pass and `ping_clock` decides whether a reading is due — and counts one only while
        // Ready, since the getter holds the previous connection's last Ping through a reconnect.
        let recv_ms = now_ms_i64();
        // This PC's own clock error comes first: until its first SNTP round ends (seconds, inside
        // the quarantine) a sample could be corrected while its neighbours are not.
        let pc = pc_clock::read();
        let reading = ping_clock.poll(
            Instant::now(),
            recv_ms,
            client.server_time_delta_ms(),
            pc.error_ms(),
            is_ready && pc.settled(),
        );
        // A confirmation stores nothing — the offset in force is already the newest segment — but
        // tells Core Status the value is live: the samples and the Ping as its source. Its instant
        // stays the segment's own start, so the axis built from this status is the same axis.
        if let (Some(ping_clock::PingOffset::Confirmed(offset_secs)), Some(_)) = (reading, reports)
        {
            let _ = tx.send(FeedMsg::TimeOffset(CoreTimeOffsetStatus {
                offset_secs: Some(offset_secs),
                observed_at_utc: segment_start_ms.unwrap_or(recv_ms),
                samples: ping_clock.samples(),
                source: OffsetSource::Ping,
            }));
        }
        if let Some(ping_clock::PingOffset::Adopted(offset_secs)) = reading {
            // The writer opens the new segment at this instant (`DbMsg::CoreTimeOffset`).
            segment_start_ms = Some(recv_ms);
            // Durability-first ORDERING, not a durability GUARANTEE: this loop cannot observe the
            // writer thread's own commit, so the DbMsg is only QUEUED ahead of the FeedMsg, on the
            // writer's one ordered channel, rather than waited on through an acknowledgement path
            // that does not exist. The writer still applies its messages strictly in send order,
            // so nothing this loop sends afterwards can reach the report table ahead of this
            // segment.
            // The screen only ever learns about an offset the WRITER was told about. With no
            // report sink this core replicates nothing, so the segment would never reach the
            // table, the report reader would keep running on the uncorrected axis, and Core Status
            // would stand there claiming a correction nothing applies — and it would vanish on the
            // next restart, since the seed reads that same table. Staying silent is the honest
            // outcome: the core genuinely has no measured offset as far as anything downstream is
            // concerned.
            if let Some(sink) = reports {
                sink.send(DbMsg::CoreTimeOffset {
                    core_uid: server.uid,
                    offset_secs,
                    observed_at_utc: recv_ms,
                    source: OffsetSource::Ping.label().into(),
                });
                let _ = tx.send(FeedMsg::TimeOffset(CoreTimeOffsetStatus {
                    offset_secs: Some(offset_secs),
                    observed_at_utc: recv_ms,
                    samples: ping_clock.samples(),
                    source: OffsetSource::Ping,
                }));
            }
        }

        if !command_drain.may_wait() {
            continue;
        }

        // The table's deadline is asked against the same reading as the account ones below, so
        // the two describe one moment. The startup poll below still reads its own clock.
        let wait_now = Instant::now();
        let order_wait = server
            .feed
            .orders
            .then(|| orders_table.wait(wait_now))
            .flatten();
        // Without this a deferred strategy-edit publish would sleep until whichever unrelated
        // deadline fires next — the API-key poll, minutes out on a quiet core — instead of the
        // ~250 ms rate-limit window it is actually waiting on. The latch never loses the edit,
        // but a confirmed edit would keep rendering as pending long after the core answered.
        let strat_edit_wait = strat_edit_publish_pending
            .then(|| Duration::from_millis(250).saturating_sub(last_strat_edit_pub.elapsed()));
        let account_wait = account_reconciliation.next_wait(wait_now);
        // The API-key poll is the one deadline that is ALWAYS pending, so the wait now always has a
        // ceiling — which is what makes the poll fire on a core whose event stream went quiet. The
        // loop therefore no longer blocks indefinitely, and the former unbounded `recv()` arm is
        // gone with it.
        let poll_wait = account_reconciliation.api_expiry_wait(wait_now);
        // Without this the feature silently half-works: a core that is still starting but whose
        // event stream went quiet would sleep past its next poll on the API-key deadline (minutes),
        // freezing the progress figure mid-startup — the exact symptom this telemetry exists to
        // explain.
        let startup_settled = startup_poll_settled(is_ready, startup_sent);
        let startup_wait =
            (!startup_settled).then(|| STARTUP_POLL.saturating_sub(last_startup.elapsed()));
        let trace_wait = trace_pacer.next_due(wait_now);
        let wake_wait = [
            order_wait,
            account_wait,
            startup_wait,
            strat_edit_wait,
            trace_wait,
            Some(ping_clock.wait(wait_now)),
        ]
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
