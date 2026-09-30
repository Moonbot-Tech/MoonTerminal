//! The Mini App's reads: report, trades, strategies, core status, balances, open orders.

use std::sync::mpsc::SyncSender;
use std::time::Instant;

use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::db::CoreNames;
use moon_core::feed::{ConnStatus, CoreSysStatus, fault_keys};
use moon_core::session::balances::{BalanceFigures, aggregate_account_figures};
use moon_core::session::core_order::{CoreOrder, exchange_sections};
use moon_core::session::{BalanceState, CoreRunState};
use moon_core::telegram::report::ReportRequest;
use moon_core::telegram::web::MiniAppApiError;
use moon_core::telegram::web::dto::{
    BalancesDto, CoreBalanceDto, CoreStatusDto, CoreStrategiesDto, CoresDto, ExchangeBalanceDto,
    OrdersDto, ReportDto, ReportPeriodDto, StrategiesDto, StrategyDto, StrategyFolderDto,
    TradesDto,
};
use moon_core::util::fmt;

use super::dto::{
    balance_state_dto, balance_text, conn_of, order_dto, report_dto, report_period,
    strategy_pending, trade_dto,
};
use super::{mini_access, visible_cores};

use super::cache::{CachedReport, CachedTrades, ReportInputs, Reuse, TradesInputs, Window, reuse};
use crate::TgHost;
use crate::labels::section_label;

/// Read one period off the owner thread, then re-check the grant before replying.
///
/// A finished read answers again within the cache TTL, and past it while its inputs are
/// unchanged (`super::cache`), so a request that already received `504` can still pick the result
/// up — but only while the stored admission grant still equals this chat's current one. A
/// mismatch drops the entry instead of serving it. A request without a valid cache hit while a
/// read is in flight is `Busy`, even for the same period.
pub(super) fn mini_report(
    host: &mut dyn TgHost,
    chat_id: i64,
    period: ReportPeriodDto,
    reply: SyncSender<Result<ReportDto, MiniAppApiError>>,
) {
    let Some(access) = mini_access(host, chat_id) else {
        let _ = reply.try_send(Err(MiniAppApiError::Rejected));
        return;
    };
    // Taken before anything is read: a commit that lands while the read runs moves the revision
    // past the one the answer is filed under, so that answer is read again rather than kept.
    let revision = host.report_revision();
    let zone = host.report_zone();
    let now = moon_core::util::time::now_unix_secs() as i64;
    // Same `Period` values the Today / Yesterday / Month / Last month buttons construct.
    let request = ReportRequest::new(report_period(period), false);
    let bounds = request.bounds(now, zone);
    let order = CoreOrder::new(host.config());
    let names = CoreNames::from_servers(&host.config().servers);
    let venues = host.session().core_venues().clone();
    let window = bounds.and_then(|(from, to)| Window::of(from, to, now, zone));
    let inputs = revision.zip(window).map(|(revision, window)| ReportInputs {
        revision,
        window,
        zone,
        locale: rust_i18n::locale().to_string(),
        names: names.clone(),
        venues: venues.clone(),
        order: order.clone(),
    });
    let verdict = match &host.state().mini_report_last {
        Some(cached) => reuse(
            cached.chat == chat_id && cached.period == period,
            cached.at.elapsed(),
            cached.access == access,
            cached.inputs.as_ref(),
            inputs.as_ref(),
        ),
        None => Reuse::Read,
    };
    match verdict {
        Reuse::Serve => {
            if let Some(cached) = &host.state().mini_report_last {
                let _ = reply.try_send(Ok(cached.dto.clone()));
            }
            return;
        }
        Reuse::Rebuild => {
            if let (Some(cached), Some((_, to))) = (&host.state().mini_report_last, bounds) {
                // The rows as read, answered with the window's end as a read now would carry it.
                let mut report = cached.report.clone();
                report.to = to;
                let _ = reply.try_send(report_dto(report));
            }
            return;
        }
        Reuse::Drop => host.state_mut().mini_report_last = None,
        Reuse::Read => {}
    }
    if host.state().mini_report_pending {
        let _ = reply.try_send(Err(MiniAppApiError::Busy));
        return;
    }
    let Some((from, to)) = bounds else {
        let _ = reply.try_send(Err(MiniAppApiError::ReadFailed));
        return;
    };
    let read_access = access.clone();
    let ends_now = window.is_some_and(Window::ends_now);
    host.state_mut().mini_report_pending = true;
    host.spawn(Box::new(move || {
        let result = crate::report::read_mini_report(
            from,
            to,
            zone,
            order,
            &names,
            venues,
            read_access,
            ends_now,
        );
        Box::new(move |host: &mut dyn TgHost| {
            host.state_mut().mini_report_pending = false;
            if mini_access(host, chat_id).as_ref() != Some(&access) {
                host.state_mut().mini_report_last = None;
                let _ = reply.try_send(Err(MiniAppApiError::Rejected));
                return;
            }
            match result {
                Ok(report) => match report_dto(report.clone()) {
                    Ok(dto) => {
                        let rows_after_to = report.rows_after_to;
                        // The captions were drawn during the read: a language switched meanwhile
                        // leaves them in neither language the key could name.
                        let locale = rust_i18n::locale().to_string();
                        let inputs = inputs.filter(|inputs| {
                            inputs.window.holds(rows_after_to) && inputs.locale == locale
                        });
                        host.state_mut().mini_report_last = Some(CachedReport {
                            chat: chat_id,
                            period,
                            access,
                            at: Instant::now(),
                            inputs,
                            report,
                            dto: dto.clone(),
                        });
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
        })
    }));
}

/// Read the latest closed trades off the owner thread, then re-check the grant before replying.
///
/// Same cache and busy rules as [`mini_report`]: a finished read answers the same chat and grant
/// within the cache TTL, and past it while its inputs are unchanged, and a request without a cache
/// hit while a read is in flight is `Busy`. A read failure is `ReadFailed`, never an empty list.
pub(super) fn mini_trades(
    host: &mut dyn TgHost,
    chat_id: i64,
    reply: SyncSender<Result<TradesDto, MiniAppApiError>>,
) {
    let Some(access) = mini_access(host, chat_id) else {
        let _ = reply.try_send(Err(MiniAppApiError::Rejected));
        return;
    };
    // Before the read, for the same reason as in `mini_report`.
    let revision = host.report_revision();
    let zone = host.report_zone();
    let names = CoreNames::from_servers(&host.config().servers);
    let inputs = revision.map(|revision| TradesInputs {
        revision,
        zone,
        names: names.clone(),
    });
    let verdict = match &host.state().mini_trades_last {
        Some(cached) => reuse(
            cached.chat == chat_id,
            cached.at.elapsed(),
            cached.access == access,
            cached.inputs.as_ref(),
            inputs.as_ref(),
        ),
        None => Reuse::Read,
    };
    match verdict {
        Reuse::Serve => {
            if let Some(cached) = &host.state().mini_trades_last {
                let _ = reply.try_send(Ok(cached.dto.clone()));
            }
            return;
        }
        Reuse::Rebuild => {
            if let Some(cached) = &host.state().mini_trades_last {
                // The same rows built again: their strategy names, exchanges and "today" are read
                // from live state, as a new read would read them.
                let _ = reply.try_send(Ok(trades_dto(&*host, zone, &cached.trades)));
            }
            return;
        }
        Reuse::Drop => host.state_mut().mini_trades_last = None,
        Reuse::Read => {}
    }
    if host.state().mini_trades_pending {
        let _ = reply.try_send(Err(MiniAppApiError::Busy));
        return;
    }
    let read_access = access.clone();
    host.state_mut().mini_trades_pending = true;
    host.spawn(Box::new(move || {
        let result = crate::report::read_mini_trades(
            zone,
            read_access,
            names,
            crate::report::MINI_TRADES_LIMIT,
        );
        Box::new(move |host: &mut dyn TgHost| {
            host.state_mut().mini_trades_pending = false;
            if mini_access(host, chat_id).as_ref() != Some(&access) {
                host.state_mut().mini_trades_last = None;
                let _ = reply.try_send(Err(MiniAppApiError::Rejected));
                return;
            }
            match result {
                Ok(trades) => {
                    let dto = trades_dto(&*host, zone, &trades);
                    host.state_mut().mini_trades_last = Some(CachedTrades {
                        chat: chat_id,
                        access,
                        at: Instant::now(),
                        inputs,
                        trades,
                        dto: dto.clone(),
                    });
                    let _ = reply.try_send(Ok(dto));
                }
                Err(_) => {
                    let _ = reply.try_send(Err(MiniAppApiError::ReadFailed));
                }
            }
        })
    }));
}

/// The Trades tab's answer for rows read from the report, with the page's fields from live state.
fn trades_dto(
    host: &dyn TgHost,
    zone: chrono_tz::Tz,
    trades: &[crate::report::MiniTrade],
) -> TradesDto {
    let now_secs = moon_core::util::now_unix_ms_i64() / 1000;
    TradesDto {
        trades: trades
            .iter()
            .map(|trade| trade_dto(host, zone, now_secs, trade))
            .collect(),
        limit: u32::try_from(crate::report::MINI_TRADES_LIMIT).unwrap_or(u32::MAX),
    }
}

/// Strategies of the chat's cores, grouped by folder, with unconfirmed toggles marked.
///
/// Confirmed, vanished and settled toggle entries are dropped from the pending map first.
pub(super) fn mini_strategies(
    host: &mut dyn TgHost,
    chat_id: i64,
) -> Result<StrategiesDto, MiniAppApiError> {
    let access = mini_access(host, chat_id).ok_or(MiniAppApiError::Rejected)?;
    let can_control = matches!(access, TelegramReportAccess::Owner);
    prune_mini_strategy_wanted(host);
    let listed = visible_cores(host, &access);
    let store = host.session().store();
    let now = Instant::now();
    let mut cores = Vec::with_capacity(listed.len());
    for (id, name, exchange) in listed {
        let mut folders: Vec<StrategyFolderDto> = Vec::new();
        if let Some(data) = store.core(id) {
            for row in &data.strategies {
                let path = moon_core::feed::strategy_path::split_path(&row.folder_path).join("/");
                let entry = host.state().mini_strategy_wanted.get(&(id, row.id));
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
fn prune_mini_strategy_wanted(host: &mut dyn TgHost) {
    // Taken out and put back: the store and the map both live behind `host`.
    let mut wanted = std::mem::take(&mut host.state_mut().mini_strategy_wanted);
    {
        let store = host.session().store();
        let now = Instant::now();
        wanted.retain(|&(core, id), entry| {
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
    host.state_mut().mini_strategy_wanted = wanted;
}

/// Core status in canonical order, limited to the chat's cores.
pub(super) fn mini_cores(host: &dyn TgHost, chat_id: i64) -> Result<CoresDto, MiniAppApiError> {
    let access = mini_access(host, chat_id).ok_or(MiniAppApiError::Rejected)?;
    let can_control = matches!(access, TelegramReportAccess::Owner);
    let listed = visible_cores(host, &access);
    let store = host.session().store();
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
                .map(|fault| fault_keys::fault_kind(&fault.kind).to_string()),
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
pub(super) fn mini_balances(
    host: &dyn TgHost,
    chat_id: i64,
) -> Result<BalancesDto, MiniAppApiError> {
    let access = mini_access(host, chat_id).ok_or(MiniAppApiError::Rejected)?;
    let cores = visible_cores(host, &access);
    let store = host.session().store();
    let venues = host.session().core_venues();
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
        let reading = BalanceFigures { state, free, total };
        let show = reading.usable();
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
        figures.push((*id, name.clone(), reading));
    }
    // Folds cores sharing one exchange account, exactly as the Assets footer does.
    let grand = aggregate_account_figures(host.session(), &host.config().servers, &figures).sum;
    let mut per_exchange = Vec::new();
    for (venue, members) in exchange_sections(
        cores
            .iter()
            .enumerate()
            .map(|(index, (id, _, _))| (index, venues.get(id))),
    ) {
        let rows: Vec<_> = members
            .iter()
            .map(|&index| figures[index].clone())
            .collect();
        let summed = aggregate_account_figures(host.session(), &host.config().servers, &rows).sum;
        per_exchange.push(ExchangeBalanceDto {
            exchange: section_label(venue),
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
pub(super) fn mini_orders(host: &dyn TgHost, chat_id: i64) -> Result<OrdersDto, MiniAppApiError> {
    let access = mini_access(host, chat_id).ok_or(MiniAppApiError::Rejected)?;
    let can_control = matches!(access, TelegramReportAccess::Owner);
    let cores = visible_cores(host, &access);
    let staged = {
        let store = host.session().store();
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
        orders.push(order_dto(host, id, name, exchange, &order));
    }
    Ok(OrdersDto {
        orders,
        can_control,
    })
}
