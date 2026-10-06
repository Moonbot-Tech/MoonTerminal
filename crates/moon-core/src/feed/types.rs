//! Domain types sent from the backend to the UI. They are independent of moonproto so the UI and
//! rendering layer do not need to know about the transport.

mod core_folders;
mod core_problem;
mod core_settings;
mod core_status;
mod core_telegram;

pub use core_folders::CoreFolders;
pub use core_problem::{CoreProblem, CoreProblemCategory, CoreProblems};
pub use core_settings::{
    AutoBuySettings, AutoStartSettings, BtcBlinkSettings, CORE_FIELDS, CORE_HOTKEY_ACTION_COUNT,
    CoreChangeSet, CoreConfig, CoreConfigArea, CoreConfigEditEvent, CoreConfigEditPhase,
    CoreConfigEditResult, CoreConfigEditRow, CoreConfigRejection, CoreConfigState, CoreField,
    CoreHotkeyAction, CoreHotkeyLayout, CoreStratButtons, FieldValue, GeneralSettings,
    GestureSettings, InterfaceSettings, LeverageSettings, ManualSettings, MoveRow,
    OrderRulesSettings, ProbeBases, ProfitState, SignalsSettings, SpecialSettings,
    TelegramSettings, day_fraction_to_minutes, differing_fields, fav_markets_has, fav_markets_list,
    fav_markets_set, index_of, minutes_to_day_fraction,
};
pub use core_status::{
    ApiKeyExpiry, ConnFault, ConnFaultKind, CoreEndpoint, CoreIdentityFacts, CoreInitStep,
    CoreStartupState, CoreStartupStatus, CoreSysStatus, INIT_STEPS_TOTAL, ReportSyncProgress,
};
pub use core_telegram::{
    AuthStep, CoreTelegramActiveProxy, CoreTelegramAuthDetails, CoreTelegramCodeType,
    CoreTelegramError, CoreTelegramLoginMode, CoreTelegramProxy, CoreTelegramService,
    CoreTelegramState, ResendState, TelegramCmd, auth_controls_visible, auth_step, resend_state,
};

mod client_trade;
mod command_vocab;
mod connection;
mod market_order;
mod signals_log;
mod strategy;
mod wallet;

pub use client_trade::*;
pub use command_vocab::*;
pub use connection::*;
pub use market_order::*;
pub use signals_log::*;
pub use strategy::*;
pub use wallet::*;

#[cfg(test)]
use client_trade::TEMP_BLACKLIST_MAX;

/// Account updates and transient notifications delivered from one core's feed.
// `CoreConfig` is delivered by value on the feed. Boxing it allocates on every such message.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum FeedMsg {
    /// Already deduplicated actual trade edges; independent of order-table publication.
    TradeSounds(Vec<super::trade_sound::TradeSound>),
    Status(ConnStatus),
    /// A core's clock offset was measured and its durable write has already been HANDED to the
    /// report writer.
    ///
    /// Emission is durability-ORDERED, which is a weaker promise than durability-confirmed and is
    /// stated that way on purpose: no acknowledgement path back from the writer exists. The feed
    /// sends `DbMsg::CoreTimeOffset` first and this message second, and it sends this one ONLY
    /// when a report sink exists at all — a core replicating nothing would otherwise have the
    /// panel claim a correction that reaches no table and vanishes on the next restart. The
    /// writer applies its queue strictly in send order, so nothing sent afterwards can reach the
    /// report table ahead of the segment. What remains is a window of the writer's own queue
    /// latency in which the panel names an offset that is not yet on disk, and a failed writer
    /// transaction leaves the panel briefly ahead of the data until the next restart re-seeds it
    /// from that same table. Bounded and self-correcting; do not read this as a commit receipt.
    TimeOffset(CoreTimeOffsetStatus),
    /// Network endpoint selected from the exported MoonBot key before the connection attempt.
    ///
    /// The live feed publishes this domain value after parsing the key so UI consumers never need
    /// access to plaintext credentials. Several cores may use different ports on one host.
    Endpoint(CoreEndpoint),
    /// Core exchange from `server_info` after BaseCheck, sent once.
    ///
    /// Everything the terminal knows about a core's venue arrives here, once per connection, and
    /// is retained by `SessionManager`. Consumers therefore read it from memory: nothing needs to
    /// re-derive a venue from a client snapshot while rendering.
    Identity {
        /// Grouping and provider-election key: platform code plus HIP-3 DEX discriminator.
        id: ExchangeId,
        /// HIP-3 DEX name as the core reported it, empty for every regular exchange.
        ///
        /// [`ExchangeId`] keeps only a hash of this, which distinguishes two DEXes but cannot name
        /// either. The caption needs the name itself, so it travels alongside the key rather than
        /// being reconstructed from it.
        dex: String,
        /// Free-form venue caption from `server_info`, such as `Binance Quarterly`.
        ///
        /// Carried for the one case the venue directory cannot answer — an ordinal newer than this
        /// build. Never an identity: its spelling belongs to the core build that sent it.
        reported: String,
    },
    /// Core account base currency such as USDT or BTC from `server_info`, sent once alongside
    /// `Identity`. The UI uses it to convert a group-local USD-equivalent size before placement.
    CoreBase {
        base: String,
    },
    /// MoonBot build number the core reported in its `BaseCheck` payload, sent at most once per
    /// connection independently of `Identity` and `CoreBase`.
    ///
    /// Unlike those venue-scoped messages, this is published even when the core reported no
    /// exchange code: a build identifies the MoonBot process, not its venue.
    ///
    /// Sent ONLY when the core actually reported one, exactly like `CoreBase`: absence travels as
    /// SILENCE — no message, so no entry — rather than as a `None` inside an otherwise-populated
    /// payload. Nothing downstream may read that silence as a fault. MoonProto publishes the
    /// snapshot behind it only once init reaches Ready, and an unpublished snapshot is
    /// byte-identical to the empty payload a genuinely ancient core answers with, so the two are
    /// indistinguishable here and neither may be claimed — see [`CoreIdentityFacts`].
    ///
    /// Deliberately its own message rather than a field on [`FeedMsg::Identity`]: that variant is
    /// scoped to a core's VENUE and is published only when the core also reported an exchange
    /// code, so riding it would silently withhold the build number from every core that reports no
    /// venue.
    CoreVersion {
        version: u32,
    },
    /// Notify that the market read model changed. This lightweight wake-up makes
    /// `SessionManager` mark particular markets dirty while visible charts pull the snapshots they
    /// need. The ticks and order book themselves do not travel through the UI channel.
    MarketDataChanged(Vec<MarketDirty>),
    /// The core answered this client's chart-archive request for the market — merged it into the
    /// retained rings (`Ready`) or failed it. Sent by EVERY core, provider or not, chart open or
    /// not: the tuner's tape stage asks any connected core of the venue and waits for exactly this
    /// (`market::source::archive`), while [`Self::MarketDataChanged`] only ever comes from an
    /// elected provider with a wanted market. `epoch` is the client slot's epoch the answering
    /// client was installed under: a network reconnect keeps the slot and bumps the epoch, and an
    /// answer still in this channel from the previous connection must not settle the new one's
    /// request for the same market.
    ChartArchiveAnswered {
        market: String,
        epoch: u64,
    },
    /// Open core orders across all markets.
    Orders(Vec<OrderRow>),
    /// Fast order snapshot only for the chart/order-line store. The Orders table remains gated by
    /// `Orders`, while the chart retains a brief terminal status between `OrderEvent::Updated` and
    /// deferred removal.
    OrderLines(Vec<OrderRow>),
    /// Batch of new detects accumulated during one event-drain tick.
    Detects(Vec<DetectRow>),
    /// What this core's strategies asked to report to Telegram during one event-drain tick, for
    /// the bot to relay. Produced whatever the feed's `detects` flag says, on a station too.
    TelegramEvents(Vec<CoreTgEvent>),
    /// Batch of new core server-log lines accumulated during one event-drain tick.
    ServerLog(Vec<CoreLogLine>),
    /// Core strategy snapshot sent when its signature changes.
    Strategies(Vec<StrategyRow>),
    /// The core acknowledged a checkbox delta this terminal sent (`TStratCheckedEcho` /
    /// `TStratCheckedSync`).
    ///
    /// This is the ONLY evidence that a checkbox change was committed by the core.
    /// `Strategies` cannot serve: the protocol library flips its own snapshot the moment
    /// `set_checked` is called — before a single byte is sent — so a `checked` flag read back from
    /// `Strategies` only proves the terminal asked, never that the core agreed.
    StrategiesAck,
    /// In-flight strategy-edit state: open pending/timed-out edits plus newly resolved ones.
    ///
    /// Published on its own faster cadence than [`Self::Strategies`] — see [`StrategyEditSnapshot`]
    /// for why `open` and `resolved` travel together.
    StrategyEdits(StrategyEditSnapshot),
    /// Core strategy schema with sections and fields by kind, sent when its revision changes.
    StrategySchema(StrategySchemaModel),
    /// Core asset and position snapshot for the Assets window, sent after domain events at most
    /// once per second while the window is active and once every five seconds otherwise.
    Assets(AssetsSnapshot),
    /// Snapshot of transferable core assets by wallet for the transfer tree, sent when its revision
    /// changes after a `RefreshTransferAssets` request.
    TransferAssets(TransferAssetsSnapshot),
    /// Core License, Free-PRO, and MoonCredits state.
    License(LicenseState),
    /// Core client-settings snapshot for TP, SL, sell, iceberg, and related settings, sent on
    /// `ClientSettingsUpdated`.
    ClientSettings(ClientSettings),
    /// The core's temporary-blacklist rows, sent on `ClientSettingsUpdated` — but only when they
    /// are NEWS: a row appearing, going away, being respelled, or being re-timed in either
    /// direction.
    ///
    /// What is not news is a remainder counting down by roughly the time that passed, which is
    /// what these rows do; publishing that would wake every reader once per snapshot to say the
    /// same thing. A reader that wants a live countdown subtracts the elapsed time from what it
    /// was handed. See `feed::live::temp_blacklist`.
    TempBlacklist(Vec<TempBlacklistRow>),
    /// Core runtime and passive-mode state sent on `RuntimeStateUpdated`.
    RuntimeState(RuntimeState),
    /// Projection of the core's full safe-share configuration, sent on `SharedConfigUpdated`,
    /// `ClientSettingsUpdated`, or `LevManageUpdated` — the projection overlays the compact
    /// snapshots, so any of the three can change it even with no new full snapshot.
    ///
    /// The runtime requests that snapshot on its own after `Ready` and retries until it arrives, so
    /// this costs no extra request; it carries the settings the compact `ClientSettings` snapshot
    /// has no room for, such as the whole AutoStart page.
    CoreConfig {
        config: CoreConfig,
        /// Whether this arrival came from a real `SharedConfigUpdated` full-snapshot echo, rather
        /// than a compact-overlay republication (`ClientSettingsUpdated` / `LevManageUpdated`).
        /// `session::store::CoreData::core_config_recv_rev` must advance ONLY when this is `true`:
        /// a one-shot pull acknowledging the LATTER would let an unrelated compact event confirm a
        /// refresh of the manual block that never actually happened.
        from_full_snapshot: bool,
    },
    /// Lifecycle event for a queued core-config write: submitted-and-awaiting-echo, or the verdict
    /// one echo reached. Published beside [`Self::CoreConfig`] by
    /// `feed::live::shared_config::SharedConfigSequence`, for the toolbar and popup's per-cell
    /// notices.
    CoreConfigEdit(CoreConfigEditEvent),
    /// Core report profit counters sent on `ProfitStateUpdated`, shown beside the AutoStart loss
    /// caps and reset through [`CoreCmd::ResetProfit`].
    ProfitState(ProfitState),
    /// Forget everything known about the core's run state: a DIFFERENT MoonBot process now answers
    /// on this connection (`LifecycleEvent::ServerRestart`, a changed `PeerAppToken`).
    ///
    /// MoonProto keeps its own retained settings/strategy state across that event — it clears only
    /// news and session profits — so the values behind it describe the process that just went away.
    /// Sent for the same reason the store drops `server_version`: a replacement instance has to
    /// speak for itself.
    RunStateForgotten,
    /// The exchange identity this client published may no longer describe the core, and only a
    /// fresh client can learn the new one (#734).
    ///
    /// MoonProto reads `ServerInfo` once, in its init (BaseCheck), and neither an internal
    /// reconnect nor a `ServerRestart` repeats it, so the feed cannot re-read the venue itself.
    /// The session answers with the same respawn the Settings Reconnect button issues, debounced
    /// per core; see `session::identity_respawn`.
    IdentityStale(IdentityStaleCause),
    /// Whether the core's global strategy engine is running, sent on
    /// `Event::Strat(StratEvent::RuntimeState)`.
    ///
    /// Deliberately NOT a field of [`RuntimeState`]: the core reports the two over different
    /// commands (`TRuntimeStateCommand` and `TStratRuntimeState`) and at different moments, so
    /// merging them would force one arrival to invent a value for the other half.
    StrategiesRunning(bool),
    /// Core account hedge mode for dual-side positions, sent on `HedgeModeUpdated`.
    HedgeMode(bool),
    /// Exchange API-key expiration for this core, sent on a successful
    /// `ApiExpirationUpdated`. A failed check publishes nothing, so the store keeps the last
    /// known answer instead of falling back to "unknown" on one dropped request.
    ApiExpiry(ApiKeyExpiry),
    /// Remaining exchange API request quota for this core's account, or `None` when the core
    /// publishes none.
    ///
    /// Today only HyperLiquid cores report it (`THLRequestLimitStateCommand`), and the counter is
    /// address-level: two cores on the same address report the same number. `None` is both "this
    /// exchange does not publish a quota" and "the core has not answered yet" — the protocol draws
    /// no distinction between them.
    ApiQuota(Option<u64>),
    /// Batch of Engine-action results such as leverage, hedge, cancel-all, or transfer accumulated
    /// during one event-drain tick. The UI displays them as toasts in the active window.
    EngineActions(Vec<EngineActionResult>),
    /// Batch of core chart-alert changes from one drain tick, gated by `feed.alerts`.
    ChartAlerts(Vec<ChartAlertUpdate>),
    /// The core's answer to [`super::CoreCmd::RequestReportTraces`] for one report row.
    ///
    /// Keyed by the row's `ReportUID` rather than by MoonProto's per-request ticket: every
    /// surface wanting the same trade wants the same answer, and the store holds one entry per row.
    /// Sent for the backfill's answers too, which nothing in the UI waits for. A `Ready` answer
    /// is also queued to the local archive's writer before this message is sent — queued, not
    /// yet on disk — and a `Failed` one is never filed there: it is not evidence of anything.
    ReportTraces {
        report_uid: i64,
        outcome: super::report_traces::ReportTracesOutcome,
    },
    /// A report row of this core just OPENED a trade: the first live `RowUpsert` that carries its
    /// coin and entry (`live::capture`). Not sent for rows a catch-up page carried; but the
    /// open-row check after a (re)connect resends open rows as upserts, and those ARE sent again —
    /// the listener tells a real entry by its stamp. Read by the tape recorder alone
    /// (`market::tape_recorder`), which asks the core's archive for the run-up while it still
    /// holds it.
    TradeOpened {
        /// The report row — with the core, what tells two trades of one coin apart.
        rec_id: i64,
        coin: String,
        quote: String,
        buy: crate::db::ReportStamp,
    },
    /// A report row of this core just CLOSED, read off the same `RowUpsert` the replica gets.
    ///
    /// Carries what the session needs to file the trade's prints from the core's retained
    /// archive while the archive still holds them (`market::trade_replay::worker::capture`):
    /// the row's own coin token and the core's quote setting — the two inputs of the
    /// coin-to-market rule — and both stamps as the core wrote them, core-local, for the
    /// session's time axis to lift. A closing upsert is partial and rarely carries the coin or
    /// the entry itself; the feed completes it from the open row it saw earlier
    /// (`live::capture`), and announces each row once. A close it cannot complete is not a
    /// trade this can locate and is not sent.
    TradeClosed {
        /// The report row — with the core, what tells two trades of one coin apart.
        rec_id: i64,
        coin: String,
        quote: String,
        buy: crate::db::ReportStamp,
        close: crate::db::ReportStamp,
    },
    /// Core-built strategy-filter rows for one or more markets, from `Event::ChartText`.
    ///
    /// These are ready strings, not typed skip codes. The UI paints them; it does not recompute
    /// volume/EMA/tag matches. A later snapshot for the same market replaces the previous rows.
    ChartText(Vec<ChartTextRows>),
    /// Core resource telemetry from protocol-v4 `Event::KernelHealth`.
    /// Emitted for every health event; the store gates the Core Status panel with
    /// `sys_rev` only when metric values change.
    SysStatus(CoreSysStatus),
    /// The core's own confirmed diagnostics, rebuilt from the retained snapshot whenever the core
    /// republishes them.
    ///
    /// A FULL replace, never a delta, because that is what the protocol delivers: a new list
    /// removes rows it no longer contains, and there is no per-row "resolved". `problems_rev` moves
    /// only when the projection actually differs, and `session::lifecycle` wakes the UI on that
    /// counter rather than on arrival — the core republishes the same list on reconnect and on
    /// every newly confirmed row alike.
    Problems(CoreProblems),
    /// A Telegram snapshot RECEIVED from the core. `None` means the library holds no snapshot.
    Telegram(Option<std::sync::Arc<CoreTelegramState>>),
    /// The retained Telegram snapshot is no longer current: keep showing it, muted, but treat
    /// no part of it as actionable until a real `Telegram(..)` arrives.
    TelegramStale,
    /// The core's folder tree, empty folders included, whenever it or the folders the strategies
    /// imply have changed.
    ///
    /// A FULL replace like `Problems`, for the same reason: the protocol delivers the whole tree
    /// and a folder that vanished from it has no event of its own. `folders_rev` moves only when
    /// the projection actually differs, so a core republishing an identical tree on reconnect
    /// wakes nothing.
    Folders(CoreFolders),
    /// Core startup progress and channel measurements, POLLED from the moonproto client rather
    /// than pushed by an event — MoonProto publishes it as a passive snapshot at its own bounded
    /// rate. Sent only while the core is starting, plus once when it settles, so an already-started
    /// core costs nothing. Moonproto-free — the projection lives in `feed::live::convert`, and the
    /// store gates the Core Status panel with `startup_rev` only when progress changes.
    StartupStatus(CoreStartupStatus),
    /// Report catch-up progress: `Some` while one runs, `None` once it completes.
    ///
    /// Sent per page, which is the rate catch-up itself runs at. Carries no revision: its only
    /// reader, the status bar, renders on every backend notify anyway.
    ReportSync(Option<ReportSyncProgress>),
    /// Why the current connection attempt ended, TYPED, so the UI can localize it.
    ///
    /// Emitted exactly once per terminal failure, immediately BEFORE the `Status(Failed)` that
    /// accompanies it, and never for a healthy core. It exists because the accompanying
    /// `ConnStatus::Failed(String)` cannot survive the trip: the application-level reconnect loop
    /// in `feed::run` overwrites that payload with its own text on every retry, and a string built
    /// in this crate could not be translated anyway.
    ConnFault(ConnFault),
    /// Core news snapshot: logical news items plus the tags catalog, rebuilt from the retained
    /// moonproto `NewsState` when any `Event::News` arrives. Moonproto-free — the reduction lives in
    /// `feed::news` and the projection in `feed::live::convert`. The store gates the News panel with
    /// `news_rev` only when the reduced snapshot changes.
    News(super::news::NewsSnapshot),
    /// The core answered a `request_version_update` with [`CORE_UPDATE_REJECT_CODE`].
    ///
    /// `request_version_update` is fire-and-forget (moonproto `send_no_reply`, no ack exists), so
    /// a `ServerLog` line naming the code is the ONLY reply channel for a refused target. Published
    /// from the feed's UNCONDITIONAL `ServerLog` ingestion loop — ahead of `want_log` — so a core
    /// with logging disabled in the UI still surfaces the refusal. Carries NO payload and
    /// deliberately no timestamp: a consumer comparing it against a `sent_at_ms` would be mixing
    /// two different clocks (see `session::core_update`'s counter-not-timestamp design), so this
    /// message exists purely as a tick for a monotonic counter on `CoreData`.
    CoreUpdateRejected,
}

#[cfg(test)]
mod tests;
