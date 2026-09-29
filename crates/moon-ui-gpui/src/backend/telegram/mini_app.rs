//! Mini App routes: report, core status, balances, open orders, and owner money commands.
//!
//! The session check stays in `telegram.rs` so its authorization text stays byte-identical.
//! Every route here goes through [`Backend::mini_access`]. Only the report read leaves the UI thread.

use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use chrono_tz::Tz;
use gpui::Context;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::db::QuoteBreakdown;
use moon_core::feed::{ConnFaultKind, ConnStatus, CoreSysStatus, OrderRow};
use moon_core::session::{BalanceState, CoreId, CoreRunState};
use moon_core::telegram::report::{Period, ReportRequest};
use moon_core::telegram::web::dto::{
    BalanceStateDto, BalancesDto, CommandErrorDto, CommandResultDto, ConnDto, CoreBalanceDto,
    CoreStatusDto, CoreStrategiesDto, CoreSwitchDto, CoresDto, DayDto, ExchangeBalanceDto,
    MoneyDto, OrderDto, OrdersDto, ReportDto, ReportPeriodDto, RowDto, ScopeResultDto,
    StrategiesDto, StrategyDto, StrategyFolderDto, StrategyPendingDto, TradeDto, TradesDto,
};
use moon_core::telegram::web::{MiniAppApiError, MiniAppApiRequest};
use moon_core::util::{display_time, fmt};

use crate::Backend;
use crate::core_order::{CoreOrder, exchange_sections};
use crate::order_math::{MONEY_DECIMALS, order_pnl, order_pnl_pct, pct_to_entry, position_qty};
use crate::panels::{BalanceFigures, aggregate_balance_figures};

/// How long a finished report may answer the same chat and period without reading again.
const REPORT_CACHE_TTL: Duration = Duration::from_secs(15);

/// How long a strategy toggle stays `Pending` before it is reported as `TimedOut`.
const STRATEGY_CONFIRM_WINDOW: Duration = Duration::from_secs(45);

/// Kind key paired with the existing Core Status short label. No new locale values.
pub(super) const FAULT_LABELS: &[(&str, &str)] = &[
    ("key_empty", "core_status.fault.short.key_empty"),
    ("key_unparsable", "core_status.fault.short.key_unparsable"),
    ("local_bind_failed", "core_status.fault.short.local_port"),
    ("aborted", "core_status.fault.short.aborted"),
    ("connect_timed_out", "core_status.fault.short.no_response"),
    ("not_authenticated", "core_status.fault.short.access"),
    ("init_step_timed_out", "core_status.fault.short.stalled"),
    ("startup_stalled", "core_status.fault.short.stalled"),
    ("init_step_failed", "core_status.fault.short.unknown"),
];

/// Answer one Mini App request. The live session check is handled by the caller.
pub(super) fn dispatch(
    backend: &mut Backend,
    request: MiniAppApiRequest,
    cx: &mut Context<Backend>,
) {
    match request {
        MiniAppApiRequest::Report {
            chat_id,
            period,
            reply,
            ..
        } => backend.mini_report(chat_id, period, reply, cx),
        MiniAppApiRequest::Cores { chat_id, reply, .. } => {
            let _ = reply.try_send(backend.mini_cores(chat_id));
        }
        MiniAppApiRequest::Balances { chat_id, reply, .. } => {
            let _ = reply.try_send(backend.mini_balances(chat_id));
        }
        MiniAppApiRequest::Orders { chat_id, reply, .. } => {
            let _ = reply.try_send(backend.mini_orders(chat_id));
        }
        MiniAppApiRequest::CancelOrder {
            chat_id,
            core,
            uid,
            reply,
            ..
        } => {
            let _ = reply.try_send(backend.mini_cancel_order(chat_id, core, uid));
        }
        MiniAppApiRequest::PanicSell {
            chat_id,
            core,
            market,
            on,
            reply,
            ..
        } => {
            let _ = reply.try_send(backend.mini_panic_sell(chat_id, core, market, on));
        }
        MiniAppApiRequest::CoreSwitch {
            chat_id,
            core,
            switch,
            on,
            reply,
            ..
        } => {
            let _ = reply.try_send(backend.mini_core_switch(chat_id, core, switch, on));
        }
        MiniAppApiRequest::CoresSwitch {
            chat_id,
            cores,
            switch,
            on,
            reply,
            ..
        } => {
            let _ = reply.try_send(backend.mini_cores_switch(chat_id, &cores, switch, on));
        }
        MiniAppApiRequest::CancelAllOrders {
            chat_id,
            core,
            reply,
            ..
        } => {
            let _ = reply.try_send(backend.mini_cancel_all(chat_id, core));
        }
        MiniAppApiRequest::Trades { chat_id, reply, .. } => {
            backend.mini_trades(chat_id, reply, cx);
        }
        MiniAppApiRequest::Strategies { chat_id, reply, .. } => {
            let _ = reply.try_send(backend.mini_strategies(chat_id));
        }
        MiniAppApiRequest::StrategyToggle {
            chat_id,
            core,
            id,
            on,
            reply,
            ..
        } => {
            let _ = reply.try_send(backend.mini_strategy_toggle(chat_id, core, id, on));
        }
        MiniAppApiRequest::CoreReconnect {
            chat_id,
            core,
            reply,
            ..
        } => {
            let _ = reply.try_send(backend.mini_core_reconnect(chat_id, core, cx));
        }
        MiniAppApiRequest::Session { reply, .. } => {
            let _ = reply.try_send(Err(MiniAppApiError::Rejected));
        }
    }
}

/// Stable snake_case kind for one [`ConnFaultKind`].
///
/// `KeyUnparsable` splits on `empty` because the Core Status panel already words those two
/// facts apart. The other variants keep one key each; step and packet-count forks stay in the
/// desktop verdict and are not part of this closed set.
pub(super) fn fault_kind(kind: &ConnFaultKind) -> &'static str {
    match kind {
        ConnFaultKind::KeyUnparsable { empty: true } => "key_empty",
        ConnFaultKind::KeyUnparsable { empty: false } => "key_unparsable",
        ConnFaultKind::LocalBindFailed { .. } => "local_bind_failed",
        ConnFaultKind::Aborted => "aborted",
        ConnFaultKind::ConnectTimedOut { .. } => "connect_timed_out",
        ConnFaultKind::NotAuthenticated => "not_authenticated",
        ConnFaultKind::InitStepTimedOut { .. } => "init_step_timed_out",
        ConnFaultKind::StartupStalled => "startup_stalled",
        ConnFaultKind::InitStepFailed { .. } => "init_step_failed",
    }
}

impl Backend {
    /// Live Mini App gate: the feature is on, and `report_access` still pairs this chat.
    /// Pairing is that function's own check.
    ///
    /// `None` means the request is refused. A viewer grant may list no cores; that is still
    /// `Some`, and the reads then return empty data instead of every core.
    fn mini_access(&self, chat_id: i64) -> Option<TelegramReportAccess> {
        if !self.config.telegram.mini_app_enabled {
            return None;
        }
        self.config.telegram.report_access(chat_id)
    }

    /// Owner gate for a money command.
    ///
    /// Args:
    ///     chat_id: Paired chat that sent the command.
    ///
    /// Returns:
    ///     `Ok(())` for the owner. A viewer is `Forbidden`. No grant is `Rejected`.
    fn mini_owner(&self, chat_id: i64) -> Result<(), MiniAppApiError> {
        match self.mini_access(chat_id) {
            Some(TelegramReportAccess::Owner) => Ok(()),
            Some(TelegramReportAccess::Viewer(_)) => Err(MiniAppApiError::Forbidden),
            None => Err(MiniAppApiError::Rejected),
        }
    }

    /// Cancel one open order through the same session call the desktop uses.
    ///
    /// Args:
    ///     chat_id: Paired chat that sent the command.
    ///     core: Core id. `CoreId` is a `u64`.
    ///     uid: Order uid from the open-order list.
    ///
    /// Returns:
    ///     `Ok` with `ok: false` and `NotFound` when the uid is absent, and nothing is sent.
    ///     A refused send is `Unavailable`. `Err` is only the owner gate.
    fn mini_cancel_order(
        &mut self,
        chat_id: i64,
        core: u64,
        uid: u64,
    ) -> Result<CommandResultDto, MiniAppApiError> {
        self.mini_owner(chat_id)?;
        let listed = self
            .session
            .store()
            .core(core)
            .is_some_and(|data| data.orders.iter().any(|order| order.uid == uid));
        if !listed {
            return Ok(command_miss(CommandErrorDto::NotFound));
        }
        match self.session.cancel_order(core, uid) {
            Ok(()) => Ok(command_hit(None)),
            Err(_) => Ok(command_miss(CommandErrorDto::Unavailable)),
        }
    }

    /// Arm or disarm Panic Sell only when the market is not already in the asked state.
    ///
    /// Args:
    ///     chat_id: Paired chat that sent the command.
    ///     core: Core id. `CoreId` is a `u64`.
    ///     market: Market key.
    ///     on: Armed state the page asked for.
    ///
    /// Returns:
    ///     `Ok` with the armed state after an accepted toggle, or the asked state when no toggle
    ///     was needed. An absent core, or a market that is not on that core's open orders, is
    ///     `NotFound` and sends nothing. A refused toggle is `Unavailable`. `Err` is only the
    ///     owner gate.
    fn mini_panic_sell(
        &mut self,
        chat_id: i64,
        core: u64,
        market: String,
        on: bool,
    ) -> Result<CommandResultDto, MiniAppApiError> {
        self.mini_owner(chat_id)?;
        if self.session.store().core(core).is_none() {
            return Ok(command_miss(CommandErrorDto::NotFound));
        }
        let listed = self
            .session
            .store()
            .core(core)
            .is_some_and(|data| data.orders.iter().any(|order| order.market == market));
        if !listed {
            return Ok(command_miss(CommandErrorDto::NotFound));
        }
        if self.is_panic_armed(core, &market) == on {
            return Ok(command_hit(Some(on)));
        }
        if !self.toggle_panic_sell(core, market.clone()) {
            return Ok(command_miss(CommandErrorDto::Unavailable));
        }
        Ok(command_hit(Some(self.is_panic_armed(core, &market))))
    }

    /// Flip one core's trading or auto-detect switch through the desktop's session call.
    ///
    /// Args:
    ///     chat_id: Paired chat that sent the command.
    ///     core: Core id.
    ///     switch: Which switch to flip.
    ///     on: State the page asked for.
    ///
    /// Returns:
    ///     `Ok` with `NotFound` for a core that is not configured, and nothing is sent. A refused
    ///     send is `Unavailable`. `Err` is only the owner gate.
    fn mini_core_switch(
        &mut self,
        chat_id: i64,
        core: u64,
        switch: CoreSwitchDto,
        on: bool,
    ) -> Result<CommandResultDto, MiniAppApiError> {
        self.mini_owner(chat_id)?;
        if !self.mini_core_known(core) {
            return Ok(command_miss(CommandErrorDto::NotFound));
        }
        let sent = match switch {
            CoreSwitchDto::Trading => self.session.set_trading(core, on),
            CoreSwitchDto::AutoDetect => self.session.set_auto_detect(core, on),
        };
        match sent {
            Ok(()) => Ok(command_hit(None)),
            Err(_) => Ok(command_miss(CommandErrorDto::Unavailable)),
        }
    }

    /// Flip one switch on several cores with one scope call.
    ///
    /// Args:
    ///     chat_id: Paired chat that sent the command.
    ///     cores: Requested core ids; unknown ids and repeats are dropped before sending.
    ///     switch: Which switch to flip.
    ///     on: State the page asked for.
    ///
    /// Returns:
    ///     `Ok` with `sent` of `requested` known cores accepted; `ok` only when all were.
    ///     No known core is `NotFound` and sends nothing. `Err` is only the owner gate.
    fn mini_cores_switch(
        &mut self,
        chat_id: i64,
        cores: &[u64],
        switch: CoreSwitchDto,
        on: bool,
    ) -> Result<ScopeResultDto, MiniAppApiError> {
        self.mini_owner(chat_id)?;
        let visible = self.mini_owner_core_ids();
        let targets = scope_targets(cores, &visible);
        if targets.is_empty() {
            return Ok(ScopeResultDto {
                ok: false,
                sent: 0,
                requested: 0,
                error: Some(CommandErrorDto::NotFound),
            });
        }
        let accepted = match switch {
            CoreSwitchDto::Trading => self.session.set_trading_many(&targets, on),
            CoreSwitchDto::AutoDetect => self.session.set_auto_detect_many(&targets, on),
        };
        let sent = u32::try_from(accepted.len()).unwrap_or(u32::MAX);
        let requested = u32::try_from(targets.len()).unwrap_or(u32::MAX);
        let ok = sent == requested;
        Ok(ScopeResultDto {
            ok,
            sent,
            requested,
            error: (!ok).then_some(CommandErrorDto::Unavailable),
        })
    }

    /// Cancel every open order of one core through the desktop's session call.
    ///
    /// Args:
    ///     chat_id: Paired chat that sent the command.
    ///     core: Core id.
    ///
    /// Returns:
    ///     `Ok` with `NotFound` for a core that is not configured, and nothing is sent. A refused
    ///     send is `Unavailable`. `Err` is only the owner gate.
    fn mini_cancel_all(
        &mut self,
        chat_id: i64,
        core: u64,
    ) -> Result<CommandResultDto, MiniAppApiError> {
        self.mini_owner(chat_id)?;
        if !self.mini_core_known(core) {
            return Ok(command_miss(CommandErrorDto::NotFound));
        }
        match self.session.cancel_all_orders(core) {
            Ok(()) => Ok(command_hit(None)),
            Err(_) => Ok(command_miss(CommandErrorDto::Unavailable)),
        }
    }

    /// Whether `core` is one of the configured sessions the owner sees.
    fn mini_core_known(&self, core: u64) -> bool {
        self.mini_owner_core_ids().contains(&core)
    }

    /// Ids of the configured sessions the owner sees.
    fn mini_owner_core_ids(&self) -> Vec<CoreId> {
        visible_cores(self, &TelegramReportAccess::Owner)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect()
    }

    /// Read one period off the UI thread, then re-check the grant before replying.
    ///
    /// A finished read is kept for [`REPORT_CACHE_TTL`] so a request that already received
    /// `504` can still pick the result up, but only while the stored admission grant still
    /// equals this chat's current one. A mismatch drops the entry instead of serving it. A request
    /// without a valid cache hit while a read is in flight is `Busy`, even for the same period.
    fn mini_report(
        &mut self,
        chat_id: i64,
        period: ReportPeriodDto,
        reply: SyncSender<Result<ReportDto, MiniAppApiError>>,
        cx: &mut Context<Self>,
    ) {
        let Some(access) = self.mini_access(chat_id) else {
            let _ = reply.try_send(Err(MiniAppApiError::Rejected));
            return;
        };
        let (serve_cached, drop_cached) = match &self.telegram.mini_report_last {
            Some((cached_chat, cached_period, cached_access, at, _))
                if *cached_chat == chat_id
                    && *cached_period == period
                    && at.elapsed() < REPORT_CACHE_TTL =>
            {
                (cached_access == &access, cached_access != &access)
            }
            _ => (false, false),
        };
        if serve_cached {
            if let Some((_, _, _, _, dto)) = &self.telegram.mini_report_last {
                let _ = reply.try_send(Ok(dto.clone()));
            }
            return;
        }
        if drop_cached {
            self.telegram.mini_report_last = None;
        }
        if self.telegram.mini_report_pending {
            let _ = reply.try_send(Err(MiniAppApiError::Busy));
            return;
        }
        let zone = crate::chrome::clock::resolved_header_clock_zone(self.header_clock_zone());
        let now = moon_core::util::time::now_unix_secs() as i64;
        // Same `Period` values the Today / Yesterday / Month / Last month buttons construct.
        let request = ReportRequest::new(report_period(period), false);
        let Some((from, to)) = request.bounds(now, zone) else {
            let _ = reply.try_send(Err(MiniAppApiError::ReadFailed));
            return;
        };
        let order = CoreOrder::new(&self.config);
        let names = self.report_core_names();
        let venues = self.session.core_venues().clone();
        let read_access = access.clone();
        self.telegram.mini_report_pending = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    super::reports::read_mini_report(
                        from,
                        to,
                        zone,
                        order,
                        &names,
                        venues,
                        read_access,
                    )
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, _| {
                    this.telegram.mini_report_pending = false;
                    if this.mini_access(chat_id).as_ref() != Some(&access) {
                        this.telegram.mini_report_last = None;
                        let _ = reply.try_send(Err(MiniAppApiError::Rejected));
                        return;
                    }
                    match result {
                        Ok(report) => match report_dto(report) {
                            Ok(dto) => {
                                this.telegram.mini_report_last =
                                    Some((chat_id, period, access, Instant::now(), dto.clone()));
                                let _ = reply.try_send(Ok(dto));
                            }
                            Err(error) => {
                                let _ = reply.try_send(Err(error));
                            }
                        },
                        Err(_) => {
                            let _ = reply.try_send(Err(MiniAppApiError::ReadFailed));
                        }
                    }
                });
            });
        })
        .detach();
    }

    /// Read the latest closed trades off the UI thread, then re-check the grant before replying.
    ///
    /// Same cache and busy rules as [`Self::mini_report`]: a finished read answers the same chat
    /// and grant for [`REPORT_CACHE_TTL`], and a request without a cache hit while a read is in
    /// flight is `Busy`. A read failure is `ReadFailed`, never an empty list.
    fn mini_trades(
        &mut self,
        chat_id: i64,
        reply: SyncSender<Result<TradesDto, MiniAppApiError>>,
        cx: &mut Context<Self>,
    ) {
        let Some(access) = self.mini_access(chat_id) else {
            let _ = reply.try_send(Err(MiniAppApiError::Rejected));
            return;
        };
        let (serve_cached, drop_cached) = match &self.telegram.mini_trades_last {
            Some((cached_chat, cached_access, at, _))
                if *cached_chat == chat_id && at.elapsed() < REPORT_CACHE_TTL =>
            {
                (cached_access == &access, cached_access != &access)
            }
            _ => (false, false),
        };
        if serve_cached {
            if let Some((_, _, _, dto)) = &self.telegram.mini_trades_last {
                let _ = reply.try_send(Ok(dto.clone()));
            }
            return;
        }
        if drop_cached {
            self.telegram.mini_trades_last = None;
        }
        if self.telegram.mini_trades_pending {
            let _ = reply.try_send(Err(MiniAppApiError::Busy));
            return;
        }
        let zone = crate::chrome::clock::resolved_header_clock_zone(self.header_clock_zone());
        let names = self.report_core_names();
        let read_access = access.clone();
        self.telegram.mini_trades_pending = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    super::reports::read_mini_trades(
                        zone,
                        read_access,
                        names,
                        super::reports::MINI_TRADES_LIMIT,
                    )
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, _| {
                    this.telegram.mini_trades_pending = false;
                    if this.mini_access(chat_id).as_ref() != Some(&access) {
                        this.telegram.mini_trades_last = None;
                        let _ = reply.try_send(Err(MiniAppApiError::Rejected));
                        return;
                    }
                    match result {
                        Ok(trades) => {
                            let now_secs = moon_core::util::now_unix_ms_i64() / 1000;
                            let dto = TradesDto {
                                trades: trades
                                    .iter()
                                    .map(|trade| trade_dto(this, zone, now_secs, trade))
                                    .collect(),
                                limit: u32::try_from(super::reports::MINI_TRADES_LIMIT)
                                    .unwrap_or(u32::MAX),
                            };
                            this.telegram.mini_trades_last =
                                Some((chat_id, access, Instant::now(), dto.clone()));
                            let _ = reply.try_send(Ok(dto));
                        }
                        Err(_) => {
                            let _ = reply.try_send(Err(MiniAppApiError::ReadFailed));
                        }
                    }
                });
            });
        })
        .detach();
    }

    /// Strategies of the chat's cores, grouped by folder, with unconfirmed toggles marked.
    ///
    /// Confirmed, vanished and settled toggle entries are dropped from the pending map first.
    fn mini_strategies(&mut self, chat_id: i64) -> Result<StrategiesDto, MiniAppApiError> {
        let access = self.mini_access(chat_id).ok_or(MiniAppApiError::Rejected)?;
        let can_control = matches!(access, TelegramReportAccess::Owner);
        self.prune_mini_strategy_wanted();
        let listed = visible_cores(self, &access);
        let store = self.session.store();
        let now = Instant::now();
        let mut cores = Vec::with_capacity(listed.len());
        for (id, name, exchange) in listed {
            let mut folders: Vec<StrategyFolderDto> = Vec::new();
            if let Some(data) = store.core(id) {
                for row in &data.strategies {
                    let path = crate::strategies::tree::ops::split_path(&row.folder_path).join("/");
                    let entry = self.telegram.mini_strategy_wanted.get(&(id, row.id));
                    let pending = entry.and_then(|entry| {
                        strategy_pending(
                            *entry,
                            data.strategies_ack_rev,
                            data.strategies_rev,
                            row.checked,
                            now,
                        )
                    });
                    let strategy = StrategyDto {
                        id: row.id,
                        name: row.name.clone(),
                        checked: row.checked,
                        wanted: pending.and(entry.map(|(wanted, ..)| *wanted)),
                        pending,
                    };
                    match folders.iter_mut().find(|folder| folder.path == path) {
                        Some(folder) => folder.strategies.push(strategy),
                        None => folders.push(StrategyFolderDto {
                            path,
                            strategies: vec![strategy],
                        }),
                    }
                }
            }
            cores.push(CoreStrategiesDto {
                core: id,
                core_name: name,
                exchange,
                folders,
            });
        }
        Ok(StrategiesDto { cores, can_control })
    }

    /// Drop toggle entries whose strategy is gone or whose state [`strategy_pending`] settled.
    fn prune_mini_strategy_wanted(&mut self) {
        let store = self.session.store();
        let now = Instant::now();
        self.telegram
            .mini_strategy_wanted
            .retain(|&(core, id), entry| {
                let Some(data) = store.core(core) else {
                    return false;
                };
                let Some(row) = data.strategies.iter().find(|row| row.id == id) else {
                    return false;
                };
                strategy_pending(
                    *entry,
                    data.strategies_ack_rev,
                    data.strategies_rev,
                    row.checked,
                    now,
                )
                .is_some()
            });
    }

    /// Turn one strategy on or off without touching the core's strategy engine.
    ///
    /// Args:
    ///     chat_id: Paired chat that sent the command.
    ///     core: Core id.
    ///     id: Strategy id on that core.
    ///     on: Checked state the page asked for.
    ///
    /// Returns:
    ///     `Ok` with `NotFound` for an unknown core or strategy, and nothing is sent. A toggle
    ///     already pending for the same state is a hit without a second send. A refused send is
    ///     `Unavailable`. `Err` is only the owner gate.
    fn mini_strategy_toggle(
        &mut self,
        chat_id: i64,
        core: u64,
        id: u64,
        on: bool,
    ) -> Result<CommandResultDto, MiniAppApiError> {
        let result = self.mini_strategy_toggle_inner(chat_id, core, id, on);
        let outcome = match &result {
            Ok(dto) if dto.ok => "sent",
            Ok(dto) => match dto.error {
                Some(CommandErrorDto::NotFound) => "not found",
                _ => "unavailable",
            },
            Err(_) => "rejected",
        };
        log::info!("mini app strategy toggle: core={core} strategy={id} on={on} -> {outcome}");
        result
    }

    /// Body of [`Self::mini_strategy_toggle`], which logs the outcome.
    fn mini_strategy_toggle_inner(
        &mut self,
        chat_id: i64,
        core: u64,
        id: u64,
        on: bool,
    ) -> Result<CommandResultDto, MiniAppApiError> {
        self.mini_owner(chat_id)?;
        if !self.mini_core_known(core) {
            return Ok(command_miss(CommandErrorDto::NotFound));
        }
        let now = Instant::now();
        let listed = self.session.store().core(core).and_then(|data| {
            data.strategies
                .iter()
                .find(|row| row.id == id)
                .map(|row| (data.strategies_ack_rev, data.strategies_rev, row.checked))
        });
        let Some((ack_before, rev_before, checked)) = listed else {
            return Ok(command_miss(CommandErrorDto::NotFound));
        };
        let already = self
            .telegram
            .mini_strategy_wanted
            .get(&(core, id))
            .is_some_and(|entry| {
                entry.0 == on
                    && strategy_pending(*entry, ack_before, rev_before, checked, now)
                        == Some(StrategyPendingDto::Pending)
            });
        if already {
            return Ok(command_hit(None));
        }
        match self.session.apply_strategies(core, vec![(id, on)], None) {
            Ok(()) => {
                self.telegram
                    .mini_strategy_wanted
                    .insert((core, id), (on, now, ack_before, rev_before));
                Ok(command_hit(None))
            }
            Err(_) => Ok(command_miss(CommandErrorDto::Unavailable)),
        }
    }

    /// Queue one core for the desktop's reconnect path.
    ///
    /// Args:
    ///     chat_id: Paired chat that sent the command.
    ///     core: Core id.
    ///     cx: Backend context, notified so the queue drains.
    ///
    /// Returns:
    ///     `Ok` with `NotFound` for a core that is not configured, and nothing is queued. A hit
    ///     only means the request was queued; the next core status read shows the outcome.
    ///     `Err` is only the owner gate.
    fn mini_core_reconnect(
        &mut self,
        chat_id: i64,
        core: u64,
        cx: &mut Context<Self>,
    ) -> Result<CommandResultDto, MiniAppApiError> {
        self.mini_owner(chat_id)?;
        if !self.mini_core_known(core) {
            return Ok(command_miss(CommandErrorDto::NotFound));
        }
        if !self.reconnect_request.contains(&core) {
            self.reconnect_request.push(core);
        }
        cx.notify();
        Ok(command_hit(None))
    }

    /// Core status in canonical order, limited to the chat's cores.
    fn mini_cores(&self, chat_id: i64) -> Result<CoresDto, MiniAppApiError> {
        let access = self.mini_access(chat_id).ok_or(MiniAppApiError::Rejected)?;
        let can_control = matches!(access, TelegramReportAccess::Owner);
        let listed = visible_cores(self, &access);
        let store = self.session.store();
        let mut cores = Vec::with_capacity(listed.len());
        for (id, name, exchange) in listed {
            let core = store.core(id);
            let run = core.map(CoreRunState::from_core).unwrap_or_default();
            let (status, sys, fault) = match core {
                Some(core) => (core.status.clone(), core.sys, core.fault.clone()),
                None => (ConnStatus::Disconnected, CoreSysStatus::default(), None),
            };
            cores.push(CoreStatusDto {
                id,
                name,
                exchange,
                conn: conn_of(&status),
                ping_ms: sys.round_trip_ms,
                exch_ping_ms: sys.order_api_latency_ms.map(u32::from),
                cpu_proc: sys.process_cpu_percent.map(f32::from),
                cpu_sys: sys.system_cpu_percent.map(f32::from),
                fault: fault
                    .as_ref()
                    .map(|fault| fault_kind(&fault.kind).to_string()),
                trading: run.trading,
                auto_detect: run.auto_detect,
                version: core
                    .and_then(|core| core.server_version)
                    .map(fmt::core_build),
                mem_mb: sys.used_memory_mb.map(u32::from),
                free_mem_mb: sys.free_physical_memory_mb.map(u32::from),
            });
        }
        Ok(CoresDto { cores, can_control })
    }

    /// Balances for the chat's cores. The viewer filter is applied before the sum.
    fn mini_balances(&self, chat_id: i64) -> Result<BalancesDto, MiniAppApiError> {
        let access = self.mini_access(chat_id).ok_or(MiniAppApiError::Rejected)?;
        let cores = visible_cores(self, &access);
        let store = self.session.store();
        let venues = self.session.core_venues();
        let mut per_core = Vec::with_capacity(cores.len());
        let mut figures = Vec::with_capacity(cores.len());
        for (id, name, exchange) in &cores {
            let (state, free, total) = match store.core(*id) {
                Some(data) => (
                    data.balance_state(),
                    data.assets.global.free_usdt,
                    data.assets.global.total_usdt,
                ),
                None => (BalanceState::Awaiting, 0.0, 0.0),
            };
            let show = state.has_value() && free.is_finite() && total.is_finite();
            per_core.push(CoreBalanceDto {
                id: *id,
                name: name.clone(),
                exchange: exchange.clone(),
                state: balance_state_dto(state),
                free: show.then_some(free),
                total: show.then_some(total),
                free_text: show.then(|| balance_text(free)),
                total_text: show.then(|| balance_text(total)),
            });
            figures.push(BalanceFigures { state, free, total });
        }
        let grand = aggregate_balance_figures(&figures);
        let mut per_exchange = Vec::new();
        for (venue, members) in exchange_sections(
            cores
                .iter()
                .enumerate()
                .map(|(index, (id, _, _))| (index, venues.get(id))),
        ) {
            let rows: Vec<BalanceFigures> = members.iter().map(|&index| figures[index]).collect();
            let summed = aggregate_balance_figures(&rows);
            per_exchange.push(ExchangeBalanceDto {
                exchange: crate::controls::venue_section_label(venue),
                total: summed.total,
                total_text: summed.total.map(balance_text),
                counted: summed.counted,
                stale: summed.stale,
                excluded: summed.excluded,
            });
        }
        Ok(BalancesDto {
            total: grand.total,
            total_text: grand.total.map(balance_text),
            per_core,
            per_exchange,
            excluded: grand.excluded,
            counted: grand.counted,
            stale: grand.stale,
        })
    }

    /// Open orders for the chat's cores. `can_control` is true only for the owner.
    fn mini_orders(&self, chat_id: i64) -> Result<OrdersDto, MiniAppApiError> {
        let access = self.mini_access(chat_id).ok_or(MiniAppApiError::Rejected)?;
        let can_control = matches!(access, TelegramReportAccess::Owner);
        let cores = visible_cores(self, &access);
        let staged = {
            let store = self.session.store();
            let mut staged = Vec::new();
            for (id, name, exchange) in &cores {
                if let Some(data) = store.core(*id) {
                    for order in &data.orders {
                        staged.push((*id, name.clone(), exchange.clone(), order.clone()));
                    }
                }
            }
            staged
        };
        let mut orders = Vec::with_capacity(staged.len());
        for (id, name, exchange, order) in staged {
            orders.push(order_dto(self, id, name, exchange, &order));
        }
        Ok(OrdersDto {
            orders,
            can_control,
        })
    }
}

/// Sessions this grant may see, in Mini App order. An empty viewer list keeps none.
///
/// Returns `(id, name, exchange section caption)` ordered by [`by_section`].
fn visible_cores(backend: &Backend, access: &TelegramReportAccess) -> Vec<(u64, String, String)> {
    let order = CoreOrder::new(&backend.config);
    let cores: Vec<(u64, String)> = order
        .from_sessions(backend.session.sessions(), |session| match access {
            TelegramReportAccess::Owner => true,
            TelegramReportAccess::Viewer(ids) => ids.contains(&session.id),
        })
        .into_iter()
        .collect();
    by_section(
        cores,
        backend.session.core_venues(),
        |(id, _)| *id,
        |(_, name)| name,
    )
    .into_iter()
    .map(|(section, (id, name))| (id, name, section))
    .collect()
}

/// Order per-core rows the way every Mini App list shows them.
///
/// Rows are grouped into the terminal's exchange sections, in the terminal's section order
/// ([`exchange_sections`]), and sorted by name inside each section with numbers compared as
/// numbers ([`natural_cmp`]), so "Account 9" comes before "Account 10".
///
/// Args:
///     rows: Per-core rows in any order.
///     venues: Live venue of each core id.
///     id: Core id of one row.
///     name: Display name of one row.
///
/// Returns:
///     Each row paired with its section caption, in display order.
pub(super) fn by_section<T>(
    rows: Vec<T>,
    venues: &std::collections::HashMap<CoreId, moon_core::venue::CoreVenue>,
    id: impl Fn(&T) -> u64,
    name: impl Fn(&T) -> &str,
) -> Vec<(String, T)> {
    let sections: Vec<(String, Vec<usize>)> = exchange_sections(
        rows.iter()
            .enumerate()
            .map(|(index, row)| (index, venues.get(&id(row)))),
    )
    .into_iter()
    .map(|(venue, mut members)| {
        members.sort_by(|&a, &b| natural_cmp(name(&rows[a]), name(&rows[b])));
        (crate::controls::venue_section_label(venue), members)
    })
    .collect();
    let mut slots: Vec<Option<T>> = rows.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    for (label, members) in sections {
        for index in members {
            if let Some(row) = slots[index].take() {
                out.push((label.clone(), row));
            }
        }
    }
    out
}

/// Compare two names case-insensitively, reading each run of ASCII digits as one number.
///
/// Leading zeros do not change a number's value; names equal under that reading fall back to
/// the raw text, so the order is total.
///
/// Args:
///     a: First name.
///     b: Second name.
///
/// Returns:
///     The natural order of `a` against `b`.
pub(super) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    use std::iter::Peekable;
    use std::str::Chars;

    /// Consume one digit run and return it without leading zeros.
    fn number(chars: &mut Peekable<Chars<'_>>) -> String {
        let mut digits = String::new();
        while let Some(c) = chars.next_if(char::is_ascii_digit) {
            digits.push(c);
        }
        digits.trim_start_matches('0').to_string()
    }

    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let (m, n) = (number(&mut x), number(&mut y));
                let by_value = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if by_value != Ordering::Equal {
                    return by_value;
                }
            }
            (Some(p), Some(q)) => {
                let by_char = p.to_lowercase().cmp(q.to_lowercase());
                if by_char != Ordering::Equal {
                    return by_char;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Requested cores that are visible, without repeats, in `visible` order.
///
/// Args:
///     requested: Core ids the page asked for.
///     visible: Cores the chat may command, in canonical order.
///
/// Returns:
///     The ids to send; unknown ids never appear.
fn scope_targets(requested: &[u64], visible: &[CoreId]) -> Vec<CoreId> {
    let mut targets: Vec<CoreId> = Vec::new();
    for id in visible {
        if requested.contains(id) && !targets.contains(id) {
            targets.push(*id);
        }
    }
    targets
}

/// Map a stored order into the Mini App row.
///
/// The change percent uses the desktop orders table's arithmetic and precision.
/// Adaptive price text cannot be parsed back into that percent.
fn order_dto(
    backend: &Backend,
    id: u64,
    name: String,
    exchange: String,
    order: &OrderRow,
) -> OrderDto {
    let pnl = order_pnl(order).filter(|value| value.is_finite());
    let change = order_pnl_pct(order).filter(|value| value.is_finite());
    let to_entry = order_to_entry_pct(order);
    OrderDto {
        core: id,
        core_name: name,
        exchange,
        uid: order.uid,
        coin: order.coin.clone(),
        market: order.market.clone(),
        side: if order.is_short { "sell" } else { "buy" }.to_string(),
        qty_text: fmt::qty(order.size),
        entry_text: price_text(order.buy_price),
        mark_text: price_text(f64::from(order.price)),
        pnl,
        pnl_text: pnl
            .and_then(|value| fmt::signed_fixed(value, MONEY_DECIMALS).map(|(text, _)| text)),
        change_pct: change,
        change_text: change
            .and_then(|value| fmt::signed_pct(value, MONEY_DECIMALS).map(|(text, _)| text)),
        to_entry_pct: to_entry,
        to_entry_text: to_entry.and_then(distance_text),
        panic_armed: backend.is_panic_armed(id, &order.market),
    }
}

/// Unsigned text of a distance to entry: a "+" beside a resting order reads as profit, and which
/// way the mark has to travel is the side's business, not the figure's.
fn distance_text(pct: f64) -> Option<String> {
    fmt::pct(pct.abs(), MONEY_DECIMALS).map(|(text, _)| text)
}

/// Distance from the current mark to the entry of an order that holds no position yet.
///
/// A resting entry has no PnL, so the row states how far the price still has to travel to fill
/// it, with the arithmetic Moonbot's price-approach alert uses ([`pct_to_entry`]). Negative once
/// the mark has passed the entry.
///
/// Args:
///     order: Stored order row.
///
/// Returns:
///     The percent, or `None` once the order holds a position or a price is unusable.
fn order_to_entry_pct(order: &OrderRow) -> Option<f64> {
    if position_qty(order).is_some() {
        return None;
    }
    pct_to_entry(order, f64::from(order.price)).filter(|value| value.is_finite())
}

/// Map a read closed trade into the Mini App row.
///
/// Args:
///     backend: Source of venues and live strategy names.
///     zone: Display zone for the close time.
///     now_secs: Current UTC Unix seconds; decides whether the short close time drops the date.
///     trade: Trade read by [`super::reports::read_mini_trades`].
///
/// Returns:
///     The row, its strategy attributed by [`trade_strategy`].
fn trade_dto(
    backend: &Backend,
    zone: Tz,
    now_secs: i64,
    trade: &super::reports::MiniTrade,
) -> TradeDto {
    let profit = trade.profit_usdt.filter(|value| value.is_finite());
    let pct = trade.pct.filter(|value| value.is_finite());
    let (strategy, manual) = trade_strategy(trade.strategy_id, &trade.channel_name, |sid| {
        backend
            .session
            .store()
            .core(trade.core_uid)
            .and_then(|data| data.strategies.iter().find(|row| row.id == sid))
            .map(|row| row.name.clone())
    });
    TradeDto {
        core: trade.core_uid,
        core_name: trade.core_name.clone(),
        exchange: crate::controls::venue_section_label(
            backend.session.core_venues().get(&trade.core_uid),
        ),
        rec_id: trade.rec_id,
        coin: trade.coin.clone(),
        side: if trade.is_short { "sell" } else { "buy" }.to_string(),
        profit,
        profit_text: profit.map(signed_dollars),
        profit_pct: pct,
        profit_pct_text: pct
            .and_then(|value| fmt::signed_pct(value, MONEY_DECIMALS).map(|(text, _)| text)),
        closed_at: trade.close_utc,
        closed_text: display_time::format_minute(trade.close_utc, zone),
        closed_short_text: display_time::format_short_minute(trade.close_utc, zone, now_secs),
        entry_text: price_text(trade.buy_price),
        exit_text: price_text(trade.sell_price),
        qty_text: fmt::qty(trade.quantity),
        duration_secs: trade
            .buy_utc
            .map(|buy| trade.close_utc - buy)
            .filter(|secs| *secs >= 0),
        strategy,
        manual,
    }
}

/// Attribute a closed trade to its strategy the way the desktop Report does.
///
/// The stored id is Delphi-signed: a strategy whose id has the top bit set is stored negative, so
/// it is reinterpreted `as u64` exactly like the Report's own lookup rather than dropped.
///
/// Args:
///     strategy_id: The row's `strategyid`; `None` when the row carries none.
///     stored: The row's own `channelname`, the name the Report shows for a strategy the core no
///         longer lists.
///     name: Live name of a strategy id on the trade's core, `None` when the core does not list it.
///
/// Returns:
///     `(name, manual)`: the live name, else the stored one, else `#<id>`; `(None, true)` for `0`
///     (manual), and `(None, false)` when the id is unknown.
fn trade_strategy(
    strategy_id: Option<i64>,
    stored: &str,
    name: impl FnOnce(u64) -> Option<String>,
) -> (Option<String>, bool) {
    match strategy_id {
        None => (None, false),
        Some(0) => (None, true),
        Some(sid) => {
            let sid = sid as u64;
            let stored = stored.trim();
            let shown = name(sid)
                .or_else(|| (!stored.is_empty()).then(|| stored.to_string()))
                .unwrap_or_else(|| format!("#{sid}"));
            (Some(shown), false)
        }
    }
}

/// Unconfirmed state of one strategy toggle, or `None` once it is settled.
///
/// Settled means the core acknowledged a checkbox delta, the strategy list was rebuilt, and the
/// row shows the asked state. After [`STRATEGY_CONFIRM_WINDOW`] an unconfirmed toggle is
/// `TimedOut` until the row shows the asked state or the core sends a fresh strategy list
/// (`strategies_rev` moved), whose state is then the truth.
///
/// Args:
///     entry: `(wanted, sent_at, ack_rev before, strategies_rev before)` recorded at send.
///     ack_now: Current `strategies_ack_rev` of the core.
///     rev_now: Current `strategies_rev` of the core.
///     checked: Current checked state of the row.
///     now: Current instant.
///
/// Returns:
///     `Pending`, `TimedOut`, or `None` when the entry should be dropped.
pub(super) fn strategy_pending(
    entry: (bool, Instant, u64, u64),
    ack_now: u64,
    rev_now: u64,
    checked: bool,
    now: Instant,
) -> Option<StrategyPendingDto> {
    let (wanted, sent_at, ack_before, rev_before) = entry;
    if ack_now != ack_before && rev_now != rev_before && checked == wanted {
        return None;
    }
    if now.saturating_duration_since(sent_at) < STRATEGY_CONFIRM_WINDOW {
        return Some(StrategyPendingDto::Pending);
    }
    if checked == wanted || rev_now != rev_before {
        return None;
    }
    Some(StrategyPendingDto::TimedOut)
}

/// `Period` values the chat buttons Today, Yesterday, Month, and Last month already use.
fn report_period(period: ReportPeriodDto) -> Period {
    match period {
        ReportPeriodDto::Today => Period::Today,
        ReportPeriodDto::Yesterday => Period::Yesterday,
        ReportPeriodDto::Month => Period::Month,
        ReportPeriodDto::LastMonth => Period::LastMonth,
    }
}

/// A money command the core accepted. `armed` is set only for Panic Sell.
///
/// Args:
///     armed: Panic Sell state after the call, or `None` for cancel.
///
/// Returns:
///     `ok: true` and no error.
fn command_hit(armed: Option<bool>) -> CommandResultDto {
    CommandResultDto {
        ok: true,
        armed,
        error: None,
    }
}

/// A money command that did not run or was not accepted. Nothing about `armed` is claimed.
///
/// Args:
///     error: Why the command stopped.
///
/// Returns:
///     `ok: false` with that error and `armed: None`.
fn command_miss(error: CommandErrorDto) -> CommandResultDto {
    CommandResultDto {
        ok: false,
        armed: None,
        error: Some(error),
    }
}

/// JSON report. Money text follows the chat report's signed dollars, with `None` when unvalued.
///
/// Returns:
///     The report, or `ReadFailed` when a window edge has no civil date.
fn report_dto(report: super::reports::MiniReport) -> Result<ReportDto, MiniAppApiError> {
    let zone = report.zone;
    let from = iso_day(report.from, zone).ok_or(MiniAppApiError::ReadFailed)?;
    let to = iso_day(report.to, zone).ok_or(MiniAppApiError::ReadFailed)?;
    Ok(ReportDto {
        from,
        to,
        from_text: stamp(report.from, zone),
        to_text: stamp(report.to, zone),
        total: money_of(&report.total),
        by_exchange: report
            .by_exchange
            .into_iter()
            .map(|(key, name, total)| RowDto {
                key,
                name,
                section: None,
                money: money_of(&total),
            })
            .collect(),
        by_core: report
            .by_core
            .into_iter()
            .map(|(key, name, section, total)| RowDto {
                key,
                name,
                section: Some(section),
                money: money_of(&total),
            })
            .collect(),
        days: report
            .days
            .into_iter()
            .map(|(start, total)| {
                let money = money_of(&total);
                DayDto {
                    start,
                    usdt: money.usdt,
                    text: money.text,
                    trades: money.orders,
                }
            })
            .collect(),
    })
}

/// Window edge as the chat report stamps it, `DD.MM.YYYY HH:MM` in the report zone.
///
/// Args:
///     secs: UTC unix seconds of the window edge.
///     zone: Display zone.
///
/// Returns:
///     The stamp, or an empty string outside chrono's range.
fn stamp(secs: i64, zone: Tz) -> String {
    display_time::at(secs, zone)
        .map(|value| value.format("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_default()
}

/// Inclusive window edge as `YYYY-MM-DD` in the report zone.
///
/// Args:
///     secs: UTC unix seconds of the window edge.
///     zone: Display zone.
///
/// Returns:
///     The civil date, or `None` outside chrono's range.
fn iso_day(secs: i64, zone: Tz) -> Option<String> {
    display_time::date(secs, zone).map(|date| date.to_string())
}

/// Chat-report money: `0.00$` when there are no closed trades, `None` when unvalued.
///
/// `total.orders` is that closed-trade count, the figure the chat report labels
/// `telegram.report_trades`. It is not a count of open orders.
///
/// Args:
///     total: Quote breakdown whose `orders` field counts closed trades.
///
/// Returns:
///     Signed dollar text plus those counts, or no text when the total is unvalued.
fn money_of(total: &QuoteBreakdown) -> MoneyDto {
    let orders = u64::try_from(total.orders).unwrap_or(0);
    let unknown_orders = u64::try_from(total.unknown_orders).unwrap_or(0);
    if total.orders == 0 {
        return MoneyDto {
            usdt: Some(0.0),
            text: Some(signed_dollars(0.0)),
            orders: 0,
            unknown_orders: 0,
        };
    }
    match total.unified_usdt() {
        Some(amount) => MoneyDto {
            usdt: Some(amount.profit),
            text: Some(signed_dollars(amount.profit)),
            orders,
            unknown_orders,
        },
        None => MoneyDto {
            usdt: None,
            text: None,
            orders,
            unknown_orders,
        },
    }
}

/// Signed fixed dollars, the same digits the chat report appends `$` to.
fn signed_dollars(value: f64) -> String {
    let digits = fmt::signed_fixed(value, 2)
        .map(|(text, _)| text)
        .unwrap_or_else(|| "0.00".to_string());
    format!("{digits}$")
}

/// Mini App balance figure: grouped thousands, exactly two decimals, then `$`.
///
/// The hero total, each exchange, and each core share this text. The desktop
/// Assets panel keeps [`fmt::usd_grouped`], which drops a trailing zero.
///
/// Args:
///     value: USDT amount. Callers pass a finite value; a non-finite one prints `0.00$`.
///
/// Returns:
///     Display text such as `10 000.00$`.
fn balance_text(value: f64) -> String {
    let mut text = fmt::usd_grouped_cents(value);
    text.push('$');
    text
}

/// Positive finite price as adaptive text. Zero and non-finite prices are absent.
fn price_text(value: f64) -> Option<String> {
    if value.is_finite() && value > 0.0 {
        Some(fmt::adaptive(value))
    } else {
        None
    }
}

/// Connection phase, dropping the stage and failure payloads.
fn conn_of(status: &ConnStatus) -> ConnDto {
    match status {
        ConnStatus::Ready => ConnDto::Ready,
        ConnStatus::Connecting => ConnDto::Connecting,
        ConnStatus::Stage(_) => ConnDto::Stage,
        ConnStatus::Failed(_) => ConnDto::Failed,
        ConnStatus::Disconnected => ConnDto::Disconnected,
    }
}

/// Store trust state onto the Mini App enum.
fn balance_state_dto(state: BalanceState) -> BalanceStateDto {
    match state {
        BalanceState::Live => BalanceStateDto::Live,
        BalanceState::Stale => BalanceStateDto::Stale,
        BalanceState::Awaiting => BalanceStateDto::Awaiting,
        BalanceState::Unpriced => BalanceStateDto::Unpriced,
    }
}

#[cfg(test)]
mod tests;
