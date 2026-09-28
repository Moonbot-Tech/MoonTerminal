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
    CoreStatusDto, CoreSwitchDto, CoresDto, DayDto, ExchangeBalanceDto, MoneyDto, OrderDto,
    OrdersDto, ReportDto, ReportPeriodDto, RowDto, ScopeResultDto,
};
use moon_core::telegram::web::{MiniAppApiError, MiniAppApiRequest};
use moon_core::util::{display_time, fmt};

use crate::Backend;
use crate::core_order::{CoreOrder, exchange_sections};
use crate::order_math::{MONEY_DECIMALS, order_pnl, order_pnl_pct};
use crate::panels::{BalanceFigures, aggregate_balance_figures};

/// How long a finished report may answer the same chat and period without reading again.
const REPORT_CACHE_TTL: Duration = Duration::from_secs(15);

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
            .map(|(id, _)| id)
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
        let venues = self.session.core_venues().clone();
        let read_access = access.clone();
        self.telegram.mini_report_pending = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    super::reports::read_mini_report(from, to, zone, order, venues, read_access)
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

    /// Core status in canonical order, limited to the chat's cores.
    fn mini_cores(&self, chat_id: i64) -> Result<CoresDto, MiniAppApiError> {
        let access = self.mini_access(chat_id).ok_or(MiniAppApiError::Rejected)?;
        let can_control = matches!(access, TelegramReportAccess::Owner);
        let listed = visible_cores(self, &access);
        let store = self.session.store();
        let venues = self.session.core_venues();
        let mut cores = Vec::with_capacity(listed.len());
        for (id, name) in listed {
            let core = store.core(id);
            let run = core.map(CoreRunState::from_core).unwrap_or_default();
            let (status, sys, fault) = match core {
                Some(core) => (core.status.clone(), core.sys, core.fault.clone()),
                None => (ConnStatus::Disconnected, CoreSysStatus::default(), None),
            };
            cores.push(CoreStatusDto {
                id,
                name,
                exchange: crate::controls::venue_section_label(venues.get(&id)),
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
        for (id, name) in &cores {
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
                exchange: crate::controls::venue_section_label(venues.get(id)),
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
                .map(|(index, (id, _))| (index, venues.get(id))),
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
            for (id, name) in &cores {
                if let Some(data) = store.core(*id) {
                    for order in &data.orders {
                        staged.push((*id, name.clone(), order.clone()));
                    }
                }
            }
            staged
        };
        let mut orders = Vec::with_capacity(staged.len());
        for (id, name, order) in staged {
            orders.push(order_dto(self, id, name, &order));
        }
        Ok(OrdersDto {
            orders,
            can_control,
        })
    }
}

/// Sessions in canonical order that this grant may see. An empty viewer list keeps none.
fn visible_cores(backend: &Backend, access: &TelegramReportAccess) -> Vec<(u64, String)> {
    let order = CoreOrder::new(&backend.config);
    order
        .from_sessions(backend.session.sessions(), |session| match access {
            TelegramReportAccess::Owner => true,
            TelegramReportAccess::Viewer(ids) => ids.contains(&session.id),
        })
        .into_iter()
        .collect()
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
fn order_dto(backend: &Backend, id: u64, name: String, order: &OrderRow) -> OrderDto {
    let pnl = order_pnl(order).filter(|value| value.is_finite());
    let change = order_pnl_pct(order).filter(|value| value.is_finite());
    OrderDto {
        core: id,
        core_name: name,
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
        panic_armed: backend.is_panic_armed(id, &order.market),
    }
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
        total: money_of(&report.total),
        by_exchange: report
            .by_exchange
            .into_iter()
            .map(|(key, name, total)| RowDto {
                key,
                name,
                money: money_of(&total),
            })
            .collect(),
        by_core: report
            .by_core
            .into_iter()
            .map(|(key, name, total)| RowDto {
                key,
                name,
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
                }
            })
            .collect(),
    })
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
