//! The Mini App's reads: report, trades, strategies, core status and its balances, open orders.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::SyncSender;
use std::time::Instant;

use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::db::CoreNames;
use moon_core::feed::{
    AssetsSnapshot, ConnStatus, CoreSysStatus, TransferAssetsSnapshot, fault_keys,
};
use moon_core::session::balances::{BalanceFigures, aggregate_account_figures};
use moon_core::session::core_order::{CoreOrder, exchange_sections};
use moon_core::session::store::CoreData;
use moon_core::session::{BalanceState, CoreRunState};
use moon_core::telegram::report::ReportRequest;
use moon_core::telegram::web::MiniAppApiError;
use moon_core::telegram::web::dto::{
    CoinBalanceDto, CoreBalanceFigureDto, CoreStatusDto, CoreStrategiesDto, CoresDto,
    ExchangeBalanceDto, OrdersDto, ReportDto, ReportPeriodDto, StrategiesDto, StrategyDto,
    StrategyFolderDto, TradesDto,
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

/// The core-list version cell: the dotted build, plus a non-empty letter.
///
/// Args:
///     core: The store record for one core.
///
/// Returns:
///     `Some("7.71 R3")` when the core reported a number, else `None`. A release (`Some("")`)
///     and an older core (`None`) both print the bare number.
pub(super) fn core_list_version(core: &CoreData) -> Option<String> {
    core.server_version
        .map(|v| fmt::core_build_named(v, core.server_version_suffix.as_deref()))
}

/// Core status in canonical order, limited to the chat's cores, with each core's balance.
///
/// The grand total and each exchange section use [`aggregate_account_figures`] over those same
/// cores, so a stale figure counts and an awaiting or unpriced one does not. `total_text` is
/// empty when nothing was counted.
pub(super) fn mini_cores(host: &dyn TgHost, chat_id: i64) -> Result<CoresDto, MiniAppApiError> {
    let access = mini_access(host, chat_id).ok_or(MiniAppApiError::Rejected)?;
    let can_control = matches!(access, TelegramReportAccess::Owner);
    let listed = visible_cores(host, &access);
    let store = host.session().store();
    let venues = host.session().core_venues();
    let mut cores = Vec::with_capacity(listed.len());
    let mut figures = Vec::with_capacity(listed.len());
    for (id, name, exchange) in &listed {
        let data = store.core(*id);
        let run = data.map(CoreRunState::from_core).unwrap_or_default();
        let (status, sys, fault, reading, coins) = match data {
            Some(core) => {
                let reading = BalanceFigures {
                    state: core.balance_state(),
                    free: core.assets.global.free_usdt,
                    total: core.assets.global.total_usdt,
                };
                let coins = coin_rows(
                    &core.assets,
                    &core.transfer_assets,
                    &account_quote(host, *id),
                    reading,
                );
                (
                    core.status.clone(),
                    core.sys,
                    core.fault.clone(),
                    reading,
                    coins,
                )
            }
            None => (
                ConnStatus::Disconnected,
                CoreSysStatus::default(),
                None,
                BalanceFigures {
                    state: BalanceState::Awaiting,
                    free: 0.0,
                    total: 0.0,
                },
                Vec::new(),
            ),
        };
        cores.push(CoreStatusDto {
            id: *id,
            name: name.clone(),
            exchange: exchange.clone(),
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
            version: data.and_then(core_list_version),
            mem_mb: sys.used_memory_mb.map(u32::from),
            free_mem_mb: sys.free_physical_memory_mb.map(u32::from),
            balance: core_balance_figure(reading),
            coins,
        });
        figures.push((*id, name.clone(), reading));
    }
    // Folds cores sharing one exchange account, exactly as the Assets footer does.
    let grand = aggregate_account_figures(host.session(), &host.config().servers, &figures).sum;
    let mut per_exchange = Vec::new();
    for (venue, members) in exchange_sections(
        listed
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
    Ok(CoresDto {
        cores,
        can_control,
        total: grand.total,
        total_text: grand.total.map(balance_text).unwrap_or_default(),
        counted: grand.counted,
        stale: grand.stale,
        excluded: grand.excluded,
        per_exchange,
    })
}

/// Free and total for one core. Empty text when the reading cannot be shown or summed.
///
/// Args:
///     reading: The core's balance figures, including the store's trust state.
///
/// Returns:
///     The figure a Cores row carries. Numbers are absent when [`BalanceFigures::usable`] is false.
fn core_balance_figure(reading: BalanceFigures) -> CoreBalanceFigureDto {
    let show = reading.usable();
    CoreBalanceFigureDto {
        state: balance_state_dto(reading.state),
        free: show.then_some(reading.free),
        total: show.then_some(reading.total),
        free_text: show
            .then_some(reading.free)
            .map(balance_text)
            .unwrap_or_default(),
        total_text: show
            .then_some(reading.total)
            .map(balance_text)
            .unwrap_or_default(),
    }
}

/// One coin before it becomes a DTO. Duplicate wallets of the same coin collapse into this.
struct CoinDraft {
    /// Display spelling, taken from the chosen source row.
    coin: String,
    /// Held quantity. Non-finite fields are zero before the max. For a market row this is
    /// the max of the absolute finite `qty_full` and `qty`. For a spot transfer wallet it is
    /// the max of the absolute finite `total` and `amount`. A market row whose result is zero
    /// is not a coin.
    qty: f64,
    /// USDT value of the chosen row. `0` means the rate is unknown.
    value_usdt: f64,
    /// Dust floor from the market row. A transfer wallet has none, so this stays `0`.
    min_lot_usd: f64,
}

/// Absolute held quantity. A non-finite field is zero, so it cannot win a compare.
///
/// Args:
///     value: One quantity field from a market row or a transfer wallet.
///
/// Returns:
///     `value.abs()` when `value` is finite, otherwise `0`.
fn finite_abs(value: f64) -> f64 {
    if value.is_finite() { value.abs() } else { 0.0 }
}

/// Positive finite USDT value used when comparing and showing a coin.
///
/// Args:
///     value_usdt: Row value in USDT.
///
/// Returns:
///     `value_usdt` when it is finite and positive, otherwise `0` (unpriced).
fn priced_usdt(value_usdt: f64) -> f64 {
    if value_usdt.is_finite() && value_usdt > 0.0 {
        value_usdt
    } else {
        0.0
    }
}

/// A priced holding smaller than its lot. A zero or non-finite value is an unknown rate, not dust.
///
/// Args:
///     value_usdt: Row value in USDT. Non-finite and non-positive values are unpriced.
///     min_lot_usd: Minimum sellable lot. `0` means unknown, so nothing is dust.
fn priced_dust(value_usdt: f64, min_lot_usd: f64) -> bool {
    let value = priced_usdt(value_usdt);
    value > 0.0 && value < min_lot_usd
}

/// Quote currency used when the asset snapshot has no `base_currency`.
///
/// The desktop assets table uses the core's configured market quote for that fallback, and
/// `USDT` when the core is not in the server list. An unrecognized market yields an empty
/// string, which hides no wallet.
fn account_quote(host: &dyn TgHost, id: u64) -> String {
    host.config()
        .servers
        .iter()
        .find(|server| server.id == id)
        .map(|server| moon_core::symbol::resolve_quote(&server.market))
        .unwrap_or_else(|| "USDT".to_string())
}

/// Coin rows for one core. One row per coin. A positive finite value is a number only when
/// `reading` is usable.
///
/// Coin-margined wallets repeat the same coin once per contract. Those rows collapse to the
/// one with the larger held quantity, then the larger finite value. Values are not summed.
/// A non-finite quantity is zero before that compare. A market row whose held quantity is
/// zero is skipped, so a position-only futures market is not a coin. A row is dust only when
/// its value is finite, positive, and strictly below `min_lot_usd`. A zero or non-finite
/// value stays and is unpriced. The coin set is recorded before dust is dropped, so a priced
/// dust market coin is not added again from a spot transfer wallet. Quote-currency market
/// rows stay: this list is the core's coins, not the desktop assets table. Spot transfer
/// wallets are added only for a spot account, and only for coins that had no positive-qty
/// market row. The account quote wallet is collateral and is not added. Unpriced rows keep
/// input order and follow every priced row. Priced rows of equal value keep input order.
///
/// Args:
///     assets: The core's asset snapshot.
///     transfer: The core's transfer wallets. Only `spot` is read, and only for a spot account.
///     quote_fallback: Quote used when `assets.base_currency` is empty. See [`account_quote`].
///     reading: The same core's balance figures. [`BalanceFigures::usable`] gates coin values.
///
/// Returns:
///     Coins sorted by USDT value descending, with unpriced rows last.
pub(super) fn coin_rows(
    assets: &AssetsSnapshot,
    transfer: &TransferAssetsSnapshot,
    quote_fallback: &str,
    reading: BalanceFigures,
) -> Vec<CoinBalanceDto> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut drafts: Vec<CoinDraft> = Vec::new();
    for row in &assets.rows {
        let key = row.coin.trim().to_ascii_uppercase();
        if key.is_empty() {
            continue;
        }
        let qty = finite_abs(row.qty_full).max(finite_abs(row.qty));
        if qty == 0.0 {
            continue;
        }
        if let Some(&at) = index.get(&key) {
            let draft = &mut drafts[at];
            let richer = qty > draft.qty
                || (qty == draft.qty
                    && priced_usdt(row.value_usdt) > priced_usdt(draft.value_usdt));
            if richer {
                draft.coin = row.coin.clone();
                draft.qty = qty;
                draft.value_usdt = row.value_usdt;
                draft.min_lot_usd = row.min_lot_usd;
            }
        } else {
            index.insert(key, drafts.len());
            drafts.push(CoinDraft {
                coin: row.coin.clone(),
                qty,
                value_usdt: row.value_usdt,
                min_lot_usd: row.min_lot_usd,
            });
        }
    }
    let mut seen: HashSet<String> = drafts
        .iter()
        .map(|draft| draft.coin.trim().to_ascii_uppercase())
        .collect();
    drafts.retain(|draft| !priced_dust(draft.value_usdt, draft.min_lot_usd));
    if !assets.futures_account {
        let base = assets.base_currency.trim();
        let quote = if base.is_empty() {
            quote_fallback.trim().to_ascii_uppercase()
        } else {
            base.to_ascii_uppercase()
        };
        for wallet in &transfer.spot {
            let key = wallet.currency.trim().to_ascii_uppercase();
            if key.is_empty() || seen.contains(&key) {
                continue;
            }
            if !quote.is_empty() && key == quote {
                continue;
            }
            seen.insert(key);
            drafts.push(CoinDraft {
                coin: wallet.currency.clone(),
                qty: finite_abs(wallet.total).max(finite_abs(wallet.amount)),
                value_usdt: wallet.value_usdt,
                min_lot_usd: 0.0,
            });
        }
    }
    let priced = reading.usable();
    let unpriced_word = crate::t!("telegram.mini_coin_unpriced").to_string();
    let mut rows: Vec<CoinBalanceDto> = drafts
        .into_iter()
        .map(|draft| {
            let show = priced && priced_usdt(draft.value_usdt) > 0.0;
            CoinBalanceDto {
                coin: draft.coin,
                qty_text: fmt::qty(draft.qty),
                value: show.then_some(draft.value_usdt),
                value_text: if show {
                    balance_text(draft.value_usdt)
                } else {
                    unpriced_word.clone()
                },
            }
        })
        .collect();
    rows.sort_by(|left, right| match (left.value, right.value) {
        (Some(left_value), Some(right_value)) => right_value.total_cmp(&left_value),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    rows
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
